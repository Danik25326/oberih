//! HTTP-сервер для `serve(port, handler)`.
//!
//! Архітектура така сама, як у `http_client` + планувальника VM: мережевий
//! ввід-вивід виконується у ЗВИЧАЙНИХ потоках ОС над простими рядками, а
//! значення VM (`Value`) НІКОЛИ не перетинають межу потоків.
//!
//!   потік-приймач ──► потік з'єднання: читає й розбирає запит (з лімітами)
//!        │                   │ Incoming { запит, канал відповіді }
//!        │                   ▼
//!        │            планувальник VM: створює кооперативну задачу, що
//!        │            виконує обробник (може `sleep`, `httpGet`, ...)
//!        │                   │ ServerResponse (звичайні рядки)
//!        │                   ▼
//!        └────────── потік з'єднання: пише відповідь і закриває з'єднання
//!
//! Обмеження цієї версії (чесно): одне з'єднання = один запит (`Connection:
//! close`, без keep-alive); тіло запиту й відповіді — лише текст (UTF-8);
//! `Transfer-Encoding: chunked` у запитах не підтримується (відповідь 501).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::gc::{GcMap, MapKey, OMap};
use crate::vm::{OberihStruct, Value};

/// Максимальний сумарний розмір рядка запиту й заголовків.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Максимальна кількість заголовків у запиті.
pub const MAX_HEADERS: usize = 100;
/// Максимальний розмір тіла запиту.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Максимальна кількість одночасно відкритих з'єднань; решта отримує 503.
pub const MAX_CONNECTIONS: usize = 512;
/// Максимальна кількість параметрів рядка запиту (`?a=1&b=2`).
const MAX_QUERY_PARAMS: usize = 200;

/// Таймаут одного read/write на сокеті.
const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// Загальний ліміт часу на отримання заголовків (захист від повільних клієнтів).
const HEADER_DEADLINE: Duration = Duration::from_secs(10);
/// Загальний ліміт часу на отримання всього запиту разом з тілом.
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
/// Скільки чекати відповіді від обробника, перш ніж віддати 504.
const HANDLER_TIMEOUT: Duration = Duration::from_secs(60);

/// Розібраний HTTP-запит (лише прості рядки — без `Value`).
#[derive(Debug, Clone)]
pub struct ParsedRequest {
    pub method:  String,
    pub path:    String,
    pub query:   Vec<(String, String)>,
    /// Імена заголовків — у нижньому регістрі.
    pub headers: Vec<(String, String)>,
    pub body:    String,
    pub remote:  String,
}

/// Відповідь, яку потрібно записати клієнтові.
#[derive(Debug, Clone)]
pub struct ServerResponse {
    pub status:  u16,
    pub headers: Vec<(String, String)>,
    pub body:    String,
}

impl ServerResponse {
    pub fn plain(status: u16, body: &str) -> Self {
        ServerResponse { status, headers: Vec::new(), body: body.to_string() }
    }
}

/// Запит, переданий планувальнику, разом із каналом для відповіді.
pub struct Incoming {
    pub req:   ParsedRequest,
    pub reply: mpsc::Sender<ServerResponse>,
}

/// Помилка розбору запиту — перетворюється на відповідь із цим статусом.
#[derive(Debug, PartialEq)]
pub struct HttpError {
    pub status:  u16,
    pub message: &'static str,
}

fn err(status: u16, message: &'static str) -> HttpError {
    HttpError { status, message }
}

// ---------------------------------------------------------------------------
// Розбір запиту
// ---------------------------------------------------------------------------

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Декодує `%XX` (і, за потреби, `+` як пробіл). Некоректні послідовності
/// лишаються як є; не-UTF-8 байти замінюються на U+FFFD.
pub fn percent_decode(s: &str, plus_as_space: bool) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                    (Some(h), Some(l)) => { out.push((h << 4) | l); i += 3; }
                    _ => { out.push(b'%'); i += 1; }
                }
            }
            b'+' if plus_as_space => { out.push(b' '); i += 1; }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in q.split('&') {
        if pair.is_empty() { continue; }
        if out.len() >= MAX_QUERY_PARAMS { break; }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.push((percent_decode(k, true), percent_decode(v, true)));
    }
    out
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

/// Читає й розбирає один HTTP-запит із потоку, дотримуючись усіх лімітів.
pub fn read_request<R: Read>(stream: &mut R, remote: String) -> Result<ParsedRequest, HttpError> {
    let started = Instant::now();
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];

    // 1) рядок запиту й заголовки — до порожнього рядка
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            if pos > MAX_HEADER_BYTES { return Err(err(431, "Request Header Fields Too Large")); }
            break pos;
        }
        if buf.len() > MAX_HEADER_BYTES { return Err(err(431, "Request Header Fields Too Large")); }
        if started.elapsed() > HEADER_DEADLINE { return Err(err(408, "Request Timeout")); }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(err(400, "Bad Request")),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if is_timeout(&e) => return Err(err(408, "Request Timeout")),
            Err(_) => return Err(err(400, "Bad Request")),
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let method  = parts.next().unwrap_or("");
    let target  = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    if parts.next().is_some() || method.is_empty() || target.is_empty() || version.is_empty() {
        return Err(err(400, "Bad Request"));
    }
    if !version.starts_with("HTTP/1.") {
        return Err(if version.starts_with("HTTP/") { err(505, "HTTP Version Not Supported") } else { err(400, "Bad Request") });
    }
    if method.len() > 16 || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(err(400, "Bad Request"));
    }

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() { continue; }
        if headers.len() >= MAX_HEADERS { return Err(err(431, "Request Header Fields Too Large")); }
        let Some((name, value)) = line.split_once(':') else { return Err(err(400, "Bad Request")); };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name.contains(' ') { return Err(err(400, "Bad Request")); }
        headers.push((name, value.trim().to_string()));
    }

    // Захист від request smuggling: chunked не підтримуємо взагалі, а
    // суперечливі Content-Length відхиляємо.
    if headers.iter().any(|(k, _)| k == "transfer-encoding") {
        return Err(err(501, "Not Implemented"));
    }
    let mut content_length: Option<usize> = None;
    for (k, v) in &headers {
        if k != "content-length" { continue; }
        if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) { return Err(err(400, "Bad Request")); }
        let n: usize = v.parse().map_err(|_| err(400, "Bad Request"))?;
        if let Some(prev) = content_length {
            if prev != n { return Err(err(400, "Bad Request")); }
        }
        content_length = Some(n);
    }
    let want = content_length.unwrap_or(0);
    if want > MAX_BODY_BYTES { return Err(err(413, "Payload Too Large")); }

    // 2) тіло
    let body_start = header_end + 4;
    let mut body: Vec<u8> = if buf.len() > body_start { buf[body_start..].to_vec() } else { Vec::new() };
    if body.len() > want { body.truncate(want); }
    while body.len() < want {
        if started.elapsed() > REQUEST_DEADLINE { return Err(err(408, "Request Timeout")); }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(err(400, "Bad Request")),
            Ok(n) => {
                let take = n.min(want - body.len());
                body.extend_from_slice(&chunk[..take]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if is_timeout(&e) => return Err(err(408, "Request Timeout")),
            Err(_) => return Err(err(400, "Bad Request")),
        }
    }

    // 3) ціль запиту -> шлях і параметри
    let target = target.split('#').next().unwrap_or("");
    if !target.starts_with('/') { return Err(err(400, "Bad Request")); }
    let (raw_path, raw_query) = target.split_once('?').unwrap_or((target, ""));

    Ok(ParsedRequest {
        method:  method.to_string(),
        path:    percent_decode(raw_path, false),
        query:   parse_query(raw_query),
        headers,
        body:    String::from_utf8_lossy(&body).into_owned(),
        remote,
    })
}

// ---------------------------------------------------------------------------
// Запис відповіді
// ---------------------------------------------------------------------------

pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue", 101 => "Switching Protocols",
        200 => "OK", 201 => "Created", 202 => "Accepted", 204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently", 302 => "Found", 303 => "See Other",
        304 => "Not Modified", 307 => "Temporary Redirect", 308 => "Permanent Redirect",
        400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found",
        405 => "Method Not Allowed", 408 => "Request Timeout", 409 => "Conflict",
        410 => "Gone", 411 => "Length Required", 413 => "Payload Too Large",
        415 => "Unsupported Media Type", 422 => "Unprocessable Entity",
        429 => "Too Many Requests", 431 => "Request Header Fields Too Large",
        500 => "Internal Server Error", 501 => "Not Implemented", 502 => "Bad Gateway",
        503 => "Service Unavailable", 504 => "Gateway Timeout", 505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

/// Записує відповідь. Заголовки від користувача очищаються: імена — лише
/// `[A-Za-z0-9_-]`, у значеннях вилучаються CR/LF (захист від response splitting).
pub fn write_response<W: Write>(w: &mut W, resp: &ServerResponse, head_only: bool) -> std::io::Result<()> {
    let status = if (100..=599).contains(&resp.status) { resp.status } else { 500 };
    let no_body = head_only || status == 204 || status == 304;
    let body_bytes = resp.body.as_bytes();

    let mut head = String::with_capacity(256);
    head.push_str(&format!("HTTP/1.1 {} {}\r\n", status, reason(status)));
    let mut has_content_type = false;
    for (k, v) in &resp.headers {
        let lk = k.to_ascii_lowercase();
        if matches!(lk.as_str(), "content-length" | "connection" | "transfer-encoding") { continue; }
        if k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') { continue; }
        let clean: String = v.chars().filter(|c| *c != '\r' && *c != '\n').collect();
        if lk == "content-type" { has_content_type = true; }
        head.push_str(k);
        head.push_str(": ");
        head.push_str(&clean);
        head.push_str("\r\n");
    }
    if !has_content_type {
        head.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    }
    let declared_len = if status == 204 || status == 304 { 0 } else { body_bytes.len() };
    head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", declared_len));

    w.write_all(head.as_bytes())?;
    if !no_body {
        w.write_all(body_bytes)?;
    }
    w.flush()
}

// ---------------------------------------------------------------------------
// Сервер: потік-приймач і потоки з'єднань
// ---------------------------------------------------------------------------

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Керування запущеним сервером.
pub struct ServerHandle {
    stop:   Arc<AtomicBool>,
    addr:   SocketAddr,
    active: Arc<AtomicUsize>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr { self.addr }

    /// Припиняє приймати нові з'єднання (уже прийняті доробляються).
    pub fn stop(&self) {
        if self.stop.swap(true, Ordering::SeqCst) { return; }
        // Блокуючий accept() можна розбудити лише з'єднанням — підключаємось самі до себе.
        let mut wake = self.addr;
        if wake.ip().is_unspecified() {
            wake.set_ip(if wake.is_ipv4() { Ipv4Addr::LOCALHOST.into() } else { Ipv6Addr::LOCALHOST.into() });
        }
        let _ = TcpStream::connect_timeout(&wake, Duration::from_secs(1));
    }

    /// Чекає (не довше `max`), поки всі потоки з'єднань допишуть відповіді.
    pub fn wait_idle(&self, max: Duration) {
        let start = Instant::now();
        while self.active.load(Ordering::SeqCst) > 0 && start.elapsed() < max {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn serve_connection<F>(mut stream: TcpStream, handler: &F)
where
    F: Fn(Incoming) -> bool,
{
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let remote = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();

    let req = match read_request(&mut stream, remote) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_response(&mut stream, &ServerResponse::plain(e.status, e.message), false);
            return;
        }
    };
    let head_only = req.method == "HEAD";

    let (tx, rx) = mpsc::channel();
    if !handler(Incoming { req, reply: tx }) {
        let _ = write_response(&mut stream, &ServerResponse::plain(503, "Service Unavailable"), head_only);
        return;
    }
    let resp = match rx.recv_timeout(HANDLER_TIMEOUT) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => ServerResponse::plain(504, "Gateway Timeout"),
        Err(mpsc::RecvTimeoutError::Disconnected) => ServerResponse::plain(500, "Internal Server Error"),
    };
    let _ = write_response(&mut stream, &resp, head_only);
}

/// Запускає сервер. `on_request` передає запит планувальнику й повертає
/// `false`, якщо планувальник уже недоступний (тоді клієнт отримує 503).
pub fn start<F>(host: &str, port: u16, on_request: F) -> Result<ServerHandle, String>
where
    F: Fn(Incoming) -> bool + Send + Sync + 'static,
{
    let listener = TcpListener::bind((host, port))
        .map_err(|e| format!("не вдалось відкрити {}:{}: {}", host, port, e))?;
    let addr = listener.local_addr().map_err(|e| format!("local_addr: {}", e))?;
    let stop   = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(on_request);

    let stop_for_loop   = Arc::clone(&stop);
    let active_for_loop = Arc::clone(&active);
    std::thread::Builder::new()
        .name("oberih-accept".to_string())
        .spawn(move || {
            for conn in listener.incoming() {
                if stop_for_loop.load(Ordering::SeqCst) { break; }
                let stream = match conn {
                    Ok(s) => s,
                    Err(_) => {
                        // Тимчасова помилка (наприклад, вичерпано дескриптори) — не крутимось у порожню.
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                };
                if active_for_loop.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    let mut s = stream;
                    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                    let _ = write_response(&mut s, &ServerResponse::plain(503, "Service Unavailable"), false);
                    continue;
                }
                active_for_loop.fetch_add(1, Ordering::SeqCst);
                let handler = Arc::clone(&handler);
                let active  = Arc::clone(&active_for_loop);
                let spawned = std::thread::Builder::new()
                    .stack_size(512 * 1024)
                    .spawn(move || {
                        let _guard = ActiveGuard(active);
                        // Паніка в одному з'єднанні не має зачіпати решту.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            serve_connection(stream, &*handler);
                        }));
                    });
                if spawned.is_err() {
                    // Потік не створено — замикання (і ActiveGuard) не виконалось.
                    active_for_loop.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })
        .map_err(|e| format!("не вдалось запустити потік-приймач: {}", e))?;

    Ok(ServerHandle { stop, addr, active })
}

// ---------------------------------------------------------------------------
// Перетворення між HTTP і значеннями мови
// ---------------------------------------------------------------------------

fn pairs_to_map(pairs: &[(String, String)]) -> Value {
    let mut m = OMap::new();
    for (k, v) in pairs {
        m.insert(MapKey::Str(k.clone()), Value::Str(v.clone()));
    }
    Value::Map(GcMap::new(m))
}

/// Запит -> `Request`-struct з полями `method`, `path`, `query` (Map),
/// `headers` (Map, імена в нижньому регістрі), `body`, `remote`.
pub fn request_to_value(req: &ParsedRequest) -> Value {
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("method".to_string(),  Value::Str(req.method.clone()));
    fields.insert("path".to_string(),    Value::Str(req.path.clone()));
    fields.insert("query".to_string(),   pairs_to_map(&req.query));
    fields.insert("headers".to_string(), pairs_to_map(&req.headers));
    fields.insert("body".to_string(),    Value::Str(req.body.clone()));
    fields.insert("remote".to_string(),  Value::Str(req.remote.clone()));
    Value::Struct(OberihStruct::new("Request".to_string(), fields))
}

fn internal_error(why: &str) -> ServerResponse {
    // Деталі — лише в stderr: клієнтові внутрішні помилки не розкриваємо.
    eprintln!("[oberih] помилка обробника запиту: {}", why);
    ServerResponse::plain(500, "Internal Server Error")
}

fn from_getter(get: &dyn Fn(&str) -> Option<Value>) -> ServerResponse {
    let status: u16 = match get("status") {
        None | Some(Value::Nil) => 200,
        Some(Value::Num(n)) if n >= 100.0 && n <= 599.0 && n.fract() == 0.0 => n as u16,
        Some(other) => return internal_error(&format!("status має бути цілим числом 100..599, отримано {}", other)),
    };
    let body = match get("body") {
        None | Some(Value::Nil) => String::new(),
        Some(Value::Str(s)) => s,
        Some(other) => other.to_string(),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    match get("headers") {
        None | Some(Value::Nil) => {}
        Some(Value::Map(m)) => {
            for (k, v) in m.snapshot() {
                if let MapKey::Str(name) = k {
                    let value = match v { Value::Str(s) => s, other => other.to_string() };
                    headers.push((name, value));
                }
            }
        }
        Some(other) => return internal_error(&format!("headers має бути Map, отримано {}", other)),
    }
    ServerResponse { status, headers, body }
}

/// Значення, яке повернув обробник -> відповідь.
///   * `String`            -> 200 із цим тілом
///   * `Map`/struct        -> поля `status` (за замовчуванням 200), `body`, `headers`
///   * `nil`               -> 204 без тіла
///   * `Ok(x)`             -> як `x`; `Err(..)` та інше -> 500
pub fn value_to_response(v: &Value) -> ServerResponse {
    match v {
        Value::Str(s) => ServerResponse::plain(200, s),
        Value::Nil    => ServerResponse::plain(204, ""),
        Value::Ok(inner) => value_to_response(inner),
        Value::Err(e) => internal_error(&format!("обробник повернув помилку: {}", e)),
        Value::Map(m) => from_getter(&|k: &str| m.get(&MapKey::Str(k.to_string()))),
        Value::Struct(s) => from_getter(&|k: &str| s.get(k)),
        other => internal_error(&format!("обробник має повертати String, Map або struct, отримано {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(raw: &str) -> Result<ParsedRequest, HttpError> {
        read_request(&mut Cursor::new(raw.as_bytes().to_vec()), "1.2.3.4:5".to_string())
    }

    #[test]
    fn percent_decoding_and_plus() {
        assert_eq!(percent_decode("a%20b%2Fc", false), "a b/c");
        assert_eq!(percent_decode("a+b", false), "a+b");
        assert_eq!(percent_decode("a+b", true), "a b");
        assert_eq!(percent_decode("%D1%8F", false), "я");
        // некоректні послідовності лишаються як є
        assert_eq!(percent_decode("100%", false), "100%");
        assert_eq!(percent_decode("%zz", false), "%zz");
    }

    #[test]
    fn query_parsing() {
        let q = parse_query("a=1&b=&c&d=%D1%8F+x");
        assert_eq!(q, vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "".to_string()),
            ("c".to_string(), "".to_string()),
            ("d".to_string(), "я x".to_string()),
        ]);
    }

    #[test]
    fn parses_simple_get() {
        let r = parse("GET /a%20b?x=1&y=2 HTTP/1.1\r\nHost: h\r\nX-Token: Abc\r\n\r\n").unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/a b");
        assert_eq!(r.query.len(), 2);
        assert!(r.headers.contains(&("x-token".to_string(), "Abc".to_string())));
        assert_eq!(r.body, "");
        assert_eq!(r.remote, "1.2.3.4:5");
    }

    #[test]
    fn parses_post_body() {
        let r = parse("POST /p HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.body, "hello");
    }

    #[test]
    fn extra_bytes_after_body_are_ignored() {
        let r = parse("POST /p HTTP/1.1\r\nContent-Length: 2\r\n\r\nhiEXTRA").unwrap();
        assert_eq!(r.body, "hi");
    }

    #[test]
    fn rejects_malformed_and_dangerous_requests() {
        assert_eq!(parse("GARBAGE\r\n\r\n").unwrap_err().status, 400);
        assert_eq!(parse("get / HTTP/1.1\r\n\r\n").unwrap_err().status, 400);
        assert_eq!(parse("GET / HTTP/2\r\n\r\n").unwrap_err().status, 505);
        assert_eq!(parse("GET x HTTP/1.1\r\n\r\n").unwrap_err().status, 400);
        // chunked — не підтримується (і захист від smuggling)
        assert_eq!(parse("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap_err().status, 501);
        // суперечливі Content-Length
        assert_eq!(parse("POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab").unwrap_err().status, 400);
        assert_eq!(parse("POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n").unwrap_err().status, 400);
        // тіло більше за ліміт
        let big = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
        assert_eq!(parse(&big).unwrap_err().status, 413);
        // обірване тіло
        assert_eq!(parse("POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nab").unwrap_err().status, 400);
    }

    #[test]
    fn rejects_oversized_headers() {
        let huge = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEADER_BYTES + 100));
        assert_eq!(parse(&huge).unwrap_err().status, 431);
        let mut many = String::from("GET / HTTP/1.1\r\n");
        for i in 0..(MAX_HEADERS + 5) { many.push_str(&format!("H{}: v\r\n", i)); }
        many.push_str("\r\n");
        assert_eq!(parse(&many).unwrap_err().status, 431);
    }

    #[test]
    fn writes_well_formed_response_and_sanitizes_headers() {
        let resp = ServerResponse {
            status: 201,
            headers: vec![
                ("X-Ok".to_string(), "fine".to_string()),
                // спроба впровадити ще один заголовок через CRLF у значенні
                ("X-Evil".to_string(), "a\r\nSet-Cookie: pwned=1".to_string()),
                // некоректне ім'я заголовка — відкидається
                ("Bad Name".to_string(), "x".to_string()),
                // ці заголовки встановлює лише сервер
                ("Content-Length".to_string(), "999".to_string()),
                ("Connection".to_string(), "keep-alive".to_string()),
            ],
            body: "hello".to_string(),
        };
        let mut out: Vec<u8> = Vec::new();
        write_response(&mut out, &resp, false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 201 Created\r\n"));
        assert!(text.contains("X-Ok: fine\r\n"));
        assert!(text.contains("X-Evil: aSet-Cookie: pwned=1\r\n"));
        assert!(!text.contains("\r\nSet-Cookie:"));
        assert!(!text.contains("Bad Name"));
        assert!(text.contains("Content-Length: 5\r\n"));
        assert!(!text.contains("999"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(!text.contains("keep-alive"));
        assert!(text.contains("Content-Type: text/plain; charset=utf-8\r\n"));
        assert!(text.ends_with("\r\n\r\nhello"));
    }

    #[test]
    fn head_and_204_have_no_body() {
        let mut out: Vec<u8> = Vec::new();
        write_response(&mut out, &ServerResponse::plain(200, "abc"), true).unwrap();
        let t = String::from_utf8(out).unwrap();
        assert!(t.contains("Content-Length: 3\r\n"));
        assert!(t.ends_with("\r\n\r\n"));

        let mut out2: Vec<u8> = Vec::new();
        write_response(&mut out2, &ServerResponse::plain(204, "ignored"), false).unwrap();
        let t2 = String::from_utf8(out2).unwrap();
        assert!(t2.contains("Content-Length: 0\r\n"));
        assert!(t2.ends_with("\r\n\r\n"));
    }

    #[test]
    fn value_to_response_variants() {
        let r = value_to_response(&Value::Str("hi".to_string()));
        assert_eq!((r.status, r.body.as_str()), (200, "hi"));

        let r = value_to_response(&Value::Nil);
        assert_eq!(r.status, 204);

        let mut m = OMap::new();
        m.insert(MapKey::Str("status".to_string()), Value::Num(404.0));
        m.insert(MapKey::Str("body".to_string()), Value::Str("nope".to_string()));
        let mut h = OMap::new();
        h.insert(MapKey::Str("Content-Type".to_string()), Value::Str("application/json".to_string()));
        m.insert(MapKey::Str("headers".to_string()), Value::Map(GcMap::new(h)));
        let r = value_to_response(&Value::Map(GcMap::new(m)));
        assert_eq!(r.status, 404);
        assert_eq!(r.body, "nope");
        assert_eq!(r.headers, vec![("Content-Type".to_string(), "application/json".to_string())]);

        // за замовчуванням status = 200
        let mut m2 = OMap::new();
        m2.insert(MapKey::Str("body".to_string()), Value::Str("x".to_string()));
        assert_eq!(value_to_response(&Value::Map(GcMap::new(m2))).status, 200);

        // некоректний status і помилка обробника -> 500
        let mut bad = OMap::new();
        bad.insert(MapKey::Str("status".to_string()), Value::Num(9999.0));
        assert_eq!(value_to_response(&Value::Map(GcMap::new(bad))).status, 500);
        assert_eq!(value_to_response(&Value::Err(Box::new(Value::Str("boom".to_string())))).status, 500);
        assert_eq!(value_to_response(&Value::Num(1.0)).status, 500);
    }
}
