//! Наскрізні тести LSP: справжній бінарник `oberih lsp` як окремий процес, спілкування по
//! stdin/stdout із фреймінгом `Content-Length` — так само, як це робить редактор.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_oberih");

fn frame(body: &str) -> Vec<u8> { format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes() }

/// Розбирає ВЕСЬ stdout на кадри; будь-яке сміття поза кадрами — помилка (stdout належить протоколу).
fn parse_frames(mut data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let s = std::str::from_utf8(data).expect("stdout містить не-UTF-8");
        let head_end = s.find("\r\n\r\n").unwrap_or_else(|| panic!("сміття поза кадрами: {:?}", &s[..s.len().min(80)]));
        let head = &s[..head_end];
        let len: usize = head.strip_prefix("Content-Length: ").unwrap_or_else(|| panic!("поганий заголовок: {head:?}")).trim().parse().unwrap();
        let body_start = head_end + 4;
        out.push(s[body_start..body_start + len].to_string());
        data = &data[body_start + len..];
    }
    out
}

struct Session { frames: Vec<String>, code: Option<i32> }

fn run_session(input: Vec<u8>) -> Session {
    let mut child = Command::new(BIN).arg("lsp").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || { let mut v = Vec::new(); stdout.read_to_end(&mut v).unwrap(); v });
    // запис у окремому потоці: якщо сервер вийшов раніше, запис просто обірветься
    let writer = std::thread::spawn(move || { let _ = stdin.write_all(&input); drop(stdin); });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() { break s; }
        if start.elapsed().as_secs() > 20 { child.kill().ok(); panic!("сервер завис (>20 с)"); }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    writer.join().ok();
    let data = reader.join().unwrap();
    Session { frames: parse_frames(&data), code: status.code() }
}

const URI: &str = "file:///proj/main.obh";
fn open_msg(text: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/didOpen\",\"params\":{{\"textDocument\":{{\"uri\":\"{URI}\",\"languageId\":\"oberih\",\"version\":1,\"text\":{}}}}}}}", jstr(text))
}
fn jstr(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() { match c { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), c => o.push(c) } }
    o.push('"'); o
}
const INIT: &str = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}";
const SHUTDOWN: &str = "{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"shutdown\"}";
const EXIT: &str = "{\"jsonrpc\":\"2.0\",\"method\":\"exit\"}";

#[test]
fn full_editor_session_ends_with_exit_code_zero() {
    let bad = "fn main() -> Number {\n    let x: Number = \"a\"\n    return x\n}\n";
    let mut input = Vec::new();
    for m in [INIT, "{\"jsonrpc\":\"2.0\",\"method\":\"initialized\",\"params\":{}}", &open_msg(bad), SHUTDOWN, EXIT] { input.extend(frame(m)); }
    let s = run_session(input);
    assert_eq!(s.code, Some(0), "shutdown + exit => код 0");
    assert!(s.frames[0].contains("\"capabilities\"") && s.frames[0].contains("documentFormattingProvider"), "{}", s.frames[0]);
    let diag = s.frames.iter().find(|f| f.contains("publishDiagnostics")).expect("немає діагностики");
    assert!(diag.contains(URI) && diag.contains("\"line\":1") && diag.contains("\"severity\":1"), "{diag}");
    assert!(s.frames.last().unwrap().contains("\"id\":99") && s.frames.last().unwrap().contains("\"result\":null"));
}

#[test]
fn exit_without_shutdown_is_code_one_and_eof_does_not_hang() {
    let mut input = frame(INIT);
    input.extend(frame(EXIT));
    assert_eq!(run_session(input).code, Some(1), "exit без shutdown => код 1 за специфікацією");
    // клієнт просто закрив stdin
    let s = run_session(frame(INIT));
    assert_eq!(s.code, Some(1));
    assert_eq!(s.frames.len(), 1);
}

#[test]
fn server_survives_garbage_messages_and_keeps_stdout_clean() {
    let mut input = Vec::new();
    input.extend(frame(INIT));
    input.extend(frame("це не json"));
    input.extend(frame("{\"id\":5,\"method\":\"textDocument/hover\",\"params\":42}"));
    input.extend(frame("{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"unknown/thing\"}"));
    input.extend(frame(&open_msg("fn main() -> Number {\n    return 0\n}\n")));
    input.extend(frame(SHUTDOWN));
    input.extend(frame(EXIT));
    let s = run_session(input); // parse_frames уже перевірив, що в stdout немає сміття
    assert_eq!(s.code, Some(0));
    assert!(s.frames.iter().any(|f| f.contains("-32700")), "парсинг-помилка на сміття");
    assert!(s.frames.iter().any(|f| f.contains("\"id\":6") && f.contains("-32601")), "метод не знайдено");
    assert!(s.frames.iter().any(|f| f.contains("publishDiagnostics") && f.contains("\"diagnostics\":[]")), "після сміття сервер ще працює");
}

#[test]
fn oversized_declared_length_terminates_cleanly_instead_of_allocating() {
    let s = run_session(b"Content-Length: 99999999999\r\n\r\n".to_vec());
    assert_eq!(s.code, Some(1));
}
