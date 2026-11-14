//! Фазинг на stable Rust, без зовнішніх залежностей: працює в звичайному `cargo test` і в CI.
//!
//! Що робить: бере корпус (усі `examples/*.obh` + кілька власних зерен), мутує його (заміни
//! символів, видалення/дублювання/перестановки шматків, вставка токенів, обрізання, склеювання,
//! багатократне повторення) і годує кожну точку входу, яка приймає дані ззовні: лексер, парсер,
//! форматер, typechecker, компілятор, JSON, regex, розбір HTTP-запиту сервера, розбір відповіді
//! клієнта, manifest (oberih.toml), форматування логів.
//!
//! Що вважається збоєм: паніка, зависання (>HANG), порушення властивості (наприклад, JSON, що
//! вдалось розібрати, після stringify уже не розбирається; рядок логу містить «сирий» перенос
//! рядка; відформатований код не парситься знову). Кожен збій автоматично ЗМЕНШУЄТЬСЯ до
//! мінімального відтворювача й друкується в повідомленні тесту та у `target/fuzz-findings/`.
//!
//! Керування: `OBERIH_FUZZ_TRACE=<файл>` — перед кожним випадком записувати вхід у `<файл>.<тест>`
//! (для крахів процесу, після яких немає ні мінімізатора, ні повідомлення); `OBERIH_FUZZ_SEED=<число>` — інше зерно; `OBERIH_FUZZ_ITERS=<число>` — кількість
//! випадків на ціль (за замовчуванням малі, щоб CI був швидким; для нічного прогону ставте 200000).
//!
//! Це НЕ coverage-guided фазер (libFuzzer/AFL): він не бачить покриття, тому гірше знаходить
//! глибокі шляхи. Зате воно детерміноване, ловить паніки/зависання/зламані властивості на
//! кожному коміті та не потребує nightly.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::time::Duration;

/// Стек потоку для фронтенду (lexer/parser/typechecker/compiler/formatter).
pub const FRONTEND_STACK: usize = crate::FRONTEND_STACK_SIZE;
const HANG: Duration = Duration::from_secs(10);

// ------------------------------------------------------------------ ГПВЧ
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self { Rng(seed | 1) }
    pub fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn below(&mut self, n: usize) -> usize { if n == 0 { 0 } else { (self.next() % n as u64) as usize } }
    pub fn chance(&mut self, pct: usize) -> bool { self.below(100) < pct }
}

fn env_u64(name: &str) -> Option<u64> { std::env::var(name).ok().and_then(|v| v.parse().ok()) }
fn seed() -> u64 { env_u64("OBERIH_FUZZ_SEED").unwrap_or(0x0BE2_1F00_D5EE_D001) }
fn iters(default: usize) -> usize { env_u64("OBERIH_FUZZ_ITERS").map(|v| v as usize).unwrap_or(default) }

// ------------------------------------------------------------------ корпус і мутації
const TOKENS: &[&str] = &[
    "fn ", "resilient ", "deadline(2s) ", "retries(3) ", "fallback(x) ", "circuit_breaker(3, 10s) ",
    "rate_limit(5, 1s) ", "struct ", "enum ", "match ", "let ", "return ", "if ", "else ", "while ",
    "for ", "in ", "spawn ", "await ", "import ", "private ", "traced ", "true", "false", "null",
    "Ok(", "Err(", "Some(", "None", "=> ", "-> ", "? ", "{", "}", "(", ")", "[", "]", "<", ">", ">>",
    ",", ";", ":", ".", "..", "=", "==", "!=", "<=", ">=", "+", "-", "*", "/", "%", "&", "|", "^", "~", "!",
    "\"", "\"\\", "\\n", "'", "//", "/*", "*/", "0", "1e999", "-0", "0.5", "99999999999999999999", "NaN",
    "Number", "String", "Bool", "List<", "Map<String, ", "Fn(", "self", "main", "\n", "\r\n", "\t", " ",
    "\u{0}", "ї", "😀", "\u{202e}", "\u{feff}", "\u{7f}",
];

fn corpus() -> Vec<String> {
    let mut v: Vec<String> = vec![
        "fn main() -> Number {\n    return 0\n}\n".into(),
        "struct P { x: Number, y: Number }\nfn P.d(self: P) -> Number { return self.x + self.y }\n".into(),
        "fn f(a: Number) -> Number {\n    let r = match a { 0 => 1, _ => a * 2 }\n    return r\n}\n".into(),
        "resilient fn g(u: String) -> String\n    deadline(2s)\n    retries(3)\n    fallback(h)\n{ return u }\n".into(),
        "let m = {\"a\": [1, 2, {\"b\": null}], \"c\": \"x\"}\n".into(),
        "let r = regexMatch(\"a+b\", \"aaab\")\n".into(),
        "fn main() -> Number {\n    let t = 0\n    let v = match t {\n        0 => {\n            let a = 1\n            t = a + 2\n            println(\"x\")\n            a\n        }\n        _ => { return 3 }\n    }\n    let w = if (v == 1) { t = 5\n 6 } else { 7 }\n    return v + w\n}\n".into(),
    ];
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |x| x == "obh")).collect();
        paths.sort();
        for p in paths {
            if let Ok(s) = std::fs::read_to_string(&p) { v.push(s); }
        }
    }
    v
}

fn mutate(rng: &mut Rng, corpus: &[String]) -> String {
    let mut s: Vec<char> = corpus[rng.below(corpus.len())].chars().collect();
    for _ in 0..1 + rng.below(4) {
        if s.is_empty() { s.push('x'); }
        let n = s.len();
        match rng.below(9) {
            0 => { let i = rng.below(n); let t = TOKENS[rng.below(TOKENS.len())]; s[i] = t.chars().next().unwrap_or('?'); }
            1 => { let a = rng.below(n); let b = (a + 1 + rng.below(40)).min(n); s.drain(a..b); }
            2 => { let a = rng.below(n); let b = (a + 1 + rng.below(60)).min(n); let seg: Vec<char> = s[a..b].to_vec(); let at = rng.below(n); for (k, c) in seg.into_iter().enumerate() { s.insert((at + k).min(s.len()), c); } }
            3 => { let at = rng.below(n + 1); for (k, c) in TOKENS[rng.below(TOKENS.len())].chars().enumerate() { s.insert((at + k).min(s.len()), c); } }
            4 => { let cut = rng.below(n); s.truncate(cut); }
            5 => { let other: Vec<char> = corpus[rng.below(corpus.len())].chars().collect(); if !other.is_empty() { let a = rng.below(other.len()); let b = (a + 1 + rng.below(200)).min(other.len()); let at = rng.below(n + 1); for (k, c) in other[a..b].iter().enumerate() { s.insert((at + k).min(s.len()), *c); } } }
            6 => { let a = rng.below(n); let b = (a + 1 + rng.below(8)).min(n); let seg: Vec<char> = s[a..b].to_vec(); let times = 2 + rng.below(40); let mut ins = Vec::new(); for _ in 0..times { ins.extend_from_slice(&seg); } let at = rng.below(s.len() + 1); for (k, c) in ins.into_iter().enumerate() { s.insert((at + k).min(s.len()), c); } }
            7 => { if n > 4 { let (i, j) = (rng.below(n), rng.below(n)); s.swap(i, j); } }
            _ => { let i = rng.below(n); s[i] = char::from_u32(rng.below(0x3000) as u32).unwrap_or('?'); }
        }
        if s.len() > 20_000 { s.truncate(20_000); }
    }
    s.into_iter().collect()
}

fn random_bytes(rng: &mut Rng, max: usize) -> Vec<u8> {
    (0..rng.below(max + 1)).map(|_| match rng.below(4) {
        0 => b"{}[]\":,\\ \r\n".to_vec()[rng.below(11)],
        1 => (rng.below(128)) as u8,
        _ => rng.below(256) as u8,
    }).collect()
}

// ------------------------------------------------------------------ виконання зі сторожем
#[derive(Debug, Clone, PartialEq)]
enum Fail { Panic(String), Hang, Property(String) }

fn panic_msg(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() { s.to_string() }
    else if let Some(s) = p.downcast_ref::<String>() { s.clone() }
    else { "<паніка без повідомлення>".into() }
}

/// Виконує `f` у потоці з великим стеком, ловить паніку, обмежує час.
fn guarded<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Fail> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().stack_size(FRONTEND_STACK).spawn(move || {
        let r = catch_unwind(AssertUnwindSafe(f)).map_err(panic_msg);
        let _ = tx.send(r);
    });
    if spawned.is_err() { return Err(Fail::Property("не вдалось створити потік".into())); }
    match rx.recv_timeout(HANG) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(m)) => Err(Fail::Panic(m)),
        Err(_) => Err(Fail::Hang),
    }
}

/// Тип збою без «шумних» подробностей (щоб дедуплікувати за суттю).
fn fail_key(f: &Fail) -> String {
    match f {
        Fail::Panic(m) => format!("panic: {}", m.chars().take(120).collect::<String>()),
        Fail::Hang => "hang".into(),
        Fail::Property(m) => format!("property: {}", m.chars().take(120).collect::<String>()),
    }
}

/// Greedy-мінімізація: видаляє шматки, поки збій (того самого ключа) відтворюється.
fn shrink(input: &str, key: &str, check: &dyn Fn(&str) -> Option<Fail>) -> String {
    let mut cur: Vec<char> = input.chars().collect();
    let mut budget = 400usize;
    let mut chunk = (cur.len() / 2).max(1);
    while chunk >= 1 && budget > 0 {
        let mut i = 0;
        let mut progressed = false;
        while i < cur.len() && budget > 0 {
            let end = (i + chunk).min(cur.len());
            let mut cand = cur.clone();
            cand.drain(i..end);
            let s: String = cand.iter().collect();
            budget -= 1;
            if check(&s).map_or(false, |f| fail_key(&f) == key) { cur = cand; progressed = true; } else { i += chunk; }
        }
        if !progressed { if chunk == 1 { break; } chunk = (chunk / 2).max(1); }
    }
    cur.into_iter().collect()
}

struct Campaign {
    name: &'static str,
    found: BTreeMap<String, String>, // ключ збою -> найменший відтворювач
}

impl Campaign {
    fn new(name: &'static str) -> Self { Campaign { name, found: BTreeMap::new() } }

    fn run(&mut self, n: usize, mut gen: impl FnMut(&mut Rng) -> String, check: impl Fn(&str) -> Option<Fail> + 'static) {
        let mut rng = Rng::new(seed() ^ (self.name.len() as u64 * 0x9E37_79B9));
        let check: Box<dyn Fn(&str) -> Option<Fail>> = Box::new(check);
        for _ in 0..n {
            let input = gen(&mut rng);
            if let Some(f) = check(&input) {
                let key = fail_key(&f);
                if !self.found.contains_key(&key) || self.found[&key].len() > input.len() {
                    let small = shrink(&input, &key, &*check);
                    self.found.insert(key, small);
                }
            }
        }
    }

    fn finish(self) {
        if self.found.is_empty() { return; }
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/target/fuzz-findings");
        let _ = std::fs::create_dir_all(dir);
        let mut msg = format!("\nфаззинг `{}`: {} різних збоїв (зерно OBERIH_FUZZ_SEED={}; щоб повторити: OBERIH_FUZZ_SEED={} cargo test --release fuzz_)\n",
            self.name, self.found.len(), seed(), seed());
        for (i, (k, input)) in self.found.iter().enumerate() {
            let file = format!("{}/{}-{}.txt", dir, self.name, i);
            let _ = std::fs::write(&file, input);
            let shown: String = input.chars().take(300).collect();
            msg.push_str(&format!("  [{}] {}\n      відтворювач ({} символів, файл {}):\n      {:?}\n", i, k, input.chars().count(), file, shown));
        }
        panic!("{}", msg);
    }
}

/// Обгортка: виконати `body` на вході під сторожем; паніка/зависання → Fail.
fn guard_str<T: Send + 'static>(input: &str, body: impl FnOnce(String) -> Result<T, Fail> + Send + 'static) -> Option<Fail> {
    // Режим трасування для КРАХІВ процесу (SIGSEGV/переповнення стека/abort), після яких не
    // лишається ні мінімізатора, ні повідомлення: перед кожним випадком записуємо вхід у
    // `$OBERIH_FUZZ_TRACE.<назва тесту>`; після краху цей файл і є відтворювач.
    if let Ok(base) = std::env::var("OBERIH_FUZZ_TRACE") {
        let name = std::thread::current().name().unwrap_or("main").replace("::", "_");
        let _ = std::fs::write(format!("{}.{}", base, name), input);
    }
    let owned = input.to_string();
    match guarded(move || body(owned)) {
        Ok(Ok(_)) => None,
        Ok(Err(f)) => Some(f),
        Err(f) => Some(f),
    }
}

// ------------------------------------------------------------------ цілі: мова (фронтенд)
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
static ST_TOTAL: AtomicUsize = AtomicUsize::new(0);
static ST_LEX: AtomicUsize = AtomicUsize::new(0);
static ST_PARSE: AtomicUsize = AtomicUsize::new(0);
static ST_COMPILE: AtomicUsize = AtomicUsize::new(0);
static ST_FMT: AtomicUsize = AtomicUsize::new(0);

fn front(src: String) -> Result<(), Fail> {
    use crate::{lexer, parser, typechecker, compiler, formatter};
    ST_TOTAL.fetch_add(1, Relaxed);
    let (tokens, comments) = match lexer::Lexer::new(&src).tokenize_with_comments() { Ok(t) => t, Err(_) => return Ok(()) };
    ST_LEX.fetch_add(1, Relaxed);
    if !matches!(tokens.last().map(|t| &t.kind), Some(lexer::Token::Eof)) {
        return Err(Fail::Property("лексер повернув Ok без завершального Eof (парсер це припускає)".into()));
    }
    let program = match parser::Parser::new(tokens).parse_program() { Ok(p) => p, Err(_) => return Ok(()) };
    ST_PARSE.fetch_add(1, Relaxed);
    let _ = typechecker::Typechecker::new().check(&program);
    if compiler::Compiler::new().compile_program(&program).is_ok() { ST_COMPILE.fetch_add(1, Relaxed); }
    // Форматер: результат має знову парситись, а повторне форматування — нічого не міняти.
    let once = formatter::Formatter::new().format_program_with_comments(&program, comments);
    // Форматер не має міняти ЗНАЧЕННЯ літералів (числа за модулем: знак `-` може злитись із
    // числом, рядки — точно). Ідемпотентність цього не бачить: `1e20 -> i64::MAX` стабільне.
    let lits = |t: &str| -> Option<Vec<String>> {
        lexer::Lexer::new(t).tokenize().ok().map(|toks| toks.into_iter().filter_map(|x| match x.kind {
            lexer::Token::Number(n) => Some(format!("n{}", n.abs().to_bits())),
            lexer::Token::StringLit(s) => Some(format!("s{}", s)),
            _ => None,
        }).collect())
    };
    if let (Some(a), Some(b)) = (lits(&src), lits(&once)) {
        if a != b { return Err(Fail::Property("форматер змінив значення літералів (числа чи рядки)".into())); }
    }
    let reparsed = lexer::Lexer::new(&once).tokenize_with_comments().ok()
        .and_then(|(t, c)| parser::Parser::new(t).parse_program().ok().map(|p| (p, c)));
    match reparsed {
        None => Err(Fail::Property("форматер видав код, що не парситься знову".into())),
        Some((p2, c2)) => {
            let twice = formatter::Formatter::new().format_program_with_comments(&p2, c2);
            if twice != once { Err(Fail::Property("форматер не ідемпотентний: format(format(x)) != format(x)".into())) } else { ST_FMT.fetch_add(1, Relaxed); Ok(()) }
        }
    }
}

#[test]
fn fuzz_language_frontend() {
    let corpus = corpus();
    let mut c = Campaign::new("frontend");
    c.run(iters(4000), |rng| mutate(rng, &corpus), |s| guard_str(s, front));
    // Глибина охоплення: скільки мутацій дійшло до кожної стадії (видно з `--nocapture`).
    let t = ST_TOTAL.load(Relaxed).max(1);
    let pct = |v: &AtomicUsize| 100.0 * v.load(Relaxed) as f64 / t as f64;
    println!("фронтенд-фазинг: {} випадків; лексер ok {:.0}%, парсер ok {:.0}%, компіляція ok {:.0}%, форматер-ідемпотентність {:.0}%",
        t, pct(&ST_LEX), pct(&ST_PARSE), pct(&ST_COMPILE), pct(&ST_FMT));
    c.finish();
}

// ------------------------------------------------------------------ цілі: JSON
fn json_prop(s: String) -> Result<(), Fail> {
    use crate::json::{json_parse, json_stringify};
    if let Ok(v) = json_parse(&s) {
        let out = json_stringify(&v);
        match json_parse(&out) {
            Err(e) => return Err(Fail::Property(format!("stringify дав JSON, що не розбирається: {}", e))),
            Ok(v2) => {
                let out2 = json_stringify(&v2);
                if out2 != out { return Err(Fail::Property("stringify(parse(stringify(x))) != stringify(x)".into())); }
            }
        }
    }
    Ok(())
}

#[test]
fn fuzz_json() {
    let seeds: Vec<String> = vec![
        r#"{"a":[1,2.5,-3e2,{"b":null,"c":true}],"d":"x\n\u0041\ud83d\ude00"}"#.into(),
        "[]".into(), "{}".into(), "\"\"".into(), "[[[[1]]]]".into(), "-0".into(), "1e400".into(), "\"\\u0000\"".into(),
    ];
    let mut c = Campaign::new("json");
    c.run(iters(6000), |rng| if rng.chance(30) { String::from_utf8_lossy(&random_bytes(rng, 60)).into_owned() } else { mutate(rng, &seeds) },
          |s| guard_str(s, json_prop));
    c.finish();
}

// ------------------------------------------------------------------ цілі: regex
fn regex_prop(s: String) -> Result<(), Fail> {
    // Перший рядок — шаблон, решта — текст.
    let (pat, text) = match s.split_once('\n') { Some((p, t)) => (p.to_string(), t.to_string()), None => (s.clone(), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab".to_string()) };
    if let Ok(re) = crate::regex::Regex::new(&pat) {
        let chars: Vec<char> = text.chars().collect();
        let _ = re.find_all(&chars);
        let _ = re.is_match(&chars);
    }
    Ok(())
}

#[test]
fn fuzz_regex() {
    let seeds: Vec<String> = vec![
        "a+b\naaab".into(), "(a+)+b\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(), "(a|aa)+$\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab".into(),
        "(x+x+)+y\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx".into(), "[a-z]{1,10}\\d*\\s?\nabc123 def".into(), "^(\\w+)@(\\w+)\\.com$\nme@x.com".into(),
        "(?:a*)*b\naaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(), "a{1000}{1000}\na".into(), "((((((((a))))))))\na".into(),
    ];
    let mut c = Campaign::new("regex");
    c.run(iters(5000), |rng| mutate(rng, &seeds), |s| guard_str(s, regex_prop));
    c.finish();
}

// ------------------------------------------------------------------ цілі: HTTP-сервер (розбір запиту)
/// Читач, що віддає дані довільними шматками (імітація повільної мережі).
struct Chunky { data: Vec<u8>, pos: usize, sizes: Vec<usize>, i: usize }
impl std::io::Read for Chunky {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.data.len() { return Ok(0); }
        let want = self.sizes[self.i % self.sizes.len()].max(1);
        self.i += 1;
        let n = want.min(buf.len()).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn req_outcome(r: Result<crate::http_server::ParsedRequest, crate::http_server::HttpError>) -> String {
    match r {
        Ok(p) => format!("OK {:?} {:?} {:?} {:?} {:?}", p.method, p.path, p.query, p.headers, p.body),
        Err(e) => format!("ERR {}", e.status),
    }
}

fn server_prop(s: String) -> Result<(), Fail> {
    // Вхід — latin-1-подібне подання байтів: кожен char < 256 → один байт.
    let bytes: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect();
    let whole = req_outcome(crate::http_server::read_request(&mut std::io::Cursor::new(bytes.clone()), "r".into()));
    let sizes = vec![1, 3, 7, 2, 64, 5];
    let chunked = req_outcome(crate::http_server::read_request(&mut Chunky { data: bytes.clone(), pos: 0, sizes, i: 0 }, "r".into()));
    if whole != chunked {
        return Err(Fail::Property(format!("результат залежить від розбиття на шматки: цілим={} / шматками={}", &whole[..whole.len().min(60)], &chunked[..chunked.len().min(60)])));
    }
    let _ = crate::http_server::percent_decode(&s, true);
    let _ = crate::http_server::parse_query(&s);
    Ok(())
}

#[test]
fn fuzz_http_server_request() {
    let seeds: Vec<String> = vec![
        "GET /a/b?x=1&y=%20z HTTP/1.1\r\nHost: h\r\nUser-Agent: t\r\n\r\n".into(),
        "POST /p HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello".into(),
        "POST /p HTTP/1.1\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabc".into(),
        "POST /p HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n".into(),
        "GET / HTTP/1.0\r\n\r\n".into(), "HEAD /%zz?%=&=%41 HTTP/1.1\r\nX: y\r\n\r\n".into(),
    ];
    let mut c = Campaign::new("http_server_request");
    c.run(iters(6000), |rng| if rng.chance(25) { random_bytes(rng, 200).into_iter().map(|b| b as char).collect() } else { mutate(rng, &seeds) },
          |s| guard_str(s, server_prop));
    c.finish();
}

// ------------------------------------------------------------------ цілі: HTTP-клієнт (розбір відповіді)
#[test]
fn fuzz_http_client_response() {
    let seeds: Vec<String> = vec![
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello".into(),
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n".into(),
        "HTTP/1.1 404 Not Found\r\n\r\n".into(), "HTTP/1.1 301 Moved\r\nLocation: /x\r\n\r\n".into(),
    ];
    let mut c = Campaign::new("http_client_response");
    c.run(iters(6000), |rng| mutate(rng, &seeds), |s| guard_str(s, |s| {
        let _ = crate::http_client::parse_response(s.as_bytes());
        let _ = crate::http_client::decode_chunked(s.as_bytes());
        let _ = crate::http_client::parse_url(&s);
        Ok(())
    }));
    c.finish();
}

// ------------------------------------------------------------------ цілі: manifest (oberih.toml)
#[test]
fn fuzz_manifest() {
    let seeds: Vec<String> = vec![
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nmathutils = { path = \"../m\" }\nweb = { git = \"https://x/y.git\", branch = \"main\" }\n".into(),
        "[package]\nname=\"a\"\n".into(), "[dependencies]\nx = { git = \"u\", tag = \"v1\" }\n".into(),
    ];
    let mut c = Campaign::new("manifest");
    c.run(iters(6000), |rng| mutate(rng, &seeds), |s| guard_str(s, |s| {
        if let Ok(m) = crate::manifest::parse_str(&s, std::path::PathBuf::from(".")) {
            let out = m.to_toml_string();
            if crate::manifest::parse_str(&out, std::path::PathBuf::from(".")).is_err() {
                return Err(Fail::Property("to_toml_string дав маніфест, що не розбирається".into()));
            }
        }
        Ok(())
    }));
    c.finish();
}

// ------------------------------------------------------------------ цілі: логи (ін'єкція рядків)
#[test]
fn fuzz_log_line_injection() {
    use crate::logging::{format_line, Field, Level};
    let seeds: Vec<String> = vec!["замовлення створено".into(), "a\nb".into(), "k=v".into(), "\"q\"".into(), "x\r\ny".into()];
    let mut c = Campaign::new("log_line");
    c.run(iters(6000), |rng| mutate(rng, &seeds), |s| guard_str(s, move |s| {
        for json in [false, true] {
            let fields = vec![Field::str(&s, &s), Field::num(&s, 1.5), Field::str("level", &s), Field::str("ts", &s), Field::str("msg", &s)];
            let line = format_line("2026-01-01T00:00:00.000Z", Level::parse("info").unwrap(), &s, &fields, json);
            let body = line.strip_suffix('\n').unwrap_or(&line);
            if body.contains('\n') || body.contains('\r') {
                return Err(Fail::Property(format!("{}-рядок логу містить сирий перенос рядка (підроблений запис)", if json { "JSON" } else { "текстовий" })));
            }
            if json && crate::json::json_parse(body).is_err() {
                return Err(Fail::Property("JSON-рядок логу не є коректним JSON".into()));
            }
        }
        Ok(())
    }));
    c.finish();
}

// ------------------------------------------------------------------ глибока вкладеність
// Раніше `oberih check` на файлі з 200 000 вкладених дужок завершувався аварійно
// (`thread 'main' has overflowed its stack`), бо лексер/парсер/typechecker/компілятор
// рекурсивні. Тепер парсер має ліміт `MAX_AST_DEPTH`, а фронтенд працює на великому стеку.

type Shape = (&'static str, fn(usize) -> String);

fn wrap_main(expr: String) -> String { format!("fn main() -> Number {{\n    let x = {}\n    return 0\n}}\n", expr) }

fn binop_chain(op: &'static str) -> impl Fn(usize) -> String { move |n| wrap_main(format!("1{}", format!(" {} 1", op).repeat(n))) }

fn shapes() -> Vec<Shape> {
    fn paren(n: usize) -> String { wrap_main(format!("{}1{}", "(".repeat(n), ")".repeat(n))) }
    fn neg(n: usize) -> String { wrap_main(format!("{}1", "-".repeat(n))) }
    fn not(n: usize) -> String { wrap_main(format!("{}true", "!".repeat(n))) }
    fn tilde(n: usize) -> String { wrap_main(format!("{}1", "~".repeat(n))) }
    fn array(n: usize) -> String { wrap_main(format!("{}1{}", "[".repeat(n), "]".repeat(n))) }
    fn call(n: usize) -> String { format!("fn f(a: Number) -> Number {{ return a }}\n{}", wrap_main(format!("{}1{}", "f(".repeat(n), ")".repeat(n)))) }
    fn blocks(n: usize) -> String { format!("fn main() -> Number {{\n{}{}    return 0\n}}\n", "if (true) {\n".repeat(n), "}\n".repeat(n)) }
    fn whiles(n: usize) -> String { format!("fn main() -> Number {{\n{}{}    return 0\n}}\n", "while (false) {\n".repeat(n), "}\n".repeat(n)) }
    fn lambdas(n: usize) -> String { wrap_main(format!("{}1", "fn(a) => ".repeat(n))) }
    fn field(n: usize) -> String { wrap_main(format!("a{}", ".b".repeat(n))) }
    fn method(n: usize) -> String { wrap_main(format!("a{}", ".m()".repeat(n))) }
    fn index(n: usize) -> String { wrap_main(format!("a{}", "[0]".repeat(n))) }
    fn question(n: usize) -> String { wrap_main(format!("a{}", "?".repeat(n))) }
    fn types(n: usize) -> String { format!("fn main() -> Number {{\n    let x: {}Number{} = 0\n    return 0\n}}\n", "List<".repeat(n), ">".repeat(n)) }
    fn matches_(n: usize) -> String { wrap_main(format!("{}1{}", "match 1 { _ => ".repeat(n), " }".repeat(n))) }
    fn add(n: usize) -> String { binop_chain("+")(n) }
    fn sub(n: usize) -> String { binop_chain("-")(n) }
    fn mul(n: usize) -> String { binop_chain("*")(n) }
    fn div(n: usize) -> String { binop_chain("/")(n) }
    fn or_(n: usize) -> String { binop_chain("||")(n) }
    fn and_(n: usize) -> String { binop_chain("&&")(n) }
    fn eq(n: usize) -> String { binop_chain("==")(n) }
    fn lt(n: usize) -> String { binop_chain("<")(n) }
    fn bor(n: usize) -> String { binop_chain("|")(n) }
    fn bxor(n: usize) -> String { binop_chain("^")(n) }
    fn band(n: usize) -> String { binop_chain("&")(n) }
    fn shl(n: usize) -> String { binop_chain("<<")(n) }
    vec![("paren", paren), ("neg", neg), ("not", not), ("tilde", tilde), ("array", array), ("call", call),
         ("blocks", blocks), ("whiles", whiles), ("lambdas", lambdas), ("field", field), ("method", method),
         ("index", index), ("question", question), ("types", types), ("match", matches_),
         ("add", add), ("sub", sub), ("mul", mul), ("div", div), ("or", or_), ("and", and_), ("eq", eq),
         ("lt", lt), ("bor", bor), ("bxor", bxor), ("band", band), ("shl", shl)]
}

fn parses(src: &str) -> Result<(), String> {
    let toks = crate::lexer::Lexer::new(src).tokenize().map_err(|e| e.to_string())?;
    crate::parser::Parser::new(toks).parse_program().map(|_| ()).map_err(|e| e.message)
}

#[test]
fn deep_nesting_of_every_shape_is_a_clean_error_not_a_crash() {
    // Раніше кожна з цих форм на великій глибині аварійно завершувала увесь процес.
    for (name, gen) in shapes() {
        assert!(parses(&gen(5)).is_ok(), "генератор `{name}` дає некоректний код навіть на малій глибині: {:?}", parses(&gen(5)));
        for n in [crate::parser::MAX_AST_DEPTH + 10, 50_000, 300_000] {
            let src = gen(n);
            let r = guarded(move || parses(&src));
            match r {
                Ok(Err(m)) => assert!(m.contains("Надто глибоке вкладення"), "{name} n={n}: інша помилка: {m}"),
                Ok(Ok(())) => panic!("{name} n={n}: глибина понад ліміт мала відхилятись"),
                Err(f) => panic!("{name} n={n}: {f:?}"),
            }
        }
    }
}

#[test]
fn deepest_allowed_nesting_is_safe_even_on_an_eight_times_smaller_stack() {
    // Для кожної форми знаходимо найбільшу глибину, яку парсер ще приймає, і проганяємо ВЕСЬ
    // фронтенд (парсер + typechecker + компілятор + форматер) у потоці зі стеком у 8 разів
    // меншим за робочий. Переповнення тут аварійно завершило б процес тесту: тобто ліміт
    // підібраний із запасом, а не «на межі».
    let small_stack = crate::FRONTEND_STACK_SIZE / 8;
    for (name, gen) in shapes() {
        // Пошук межі теж на штатному стеку: потік самого тесту має лише 2 МіБ.
        let lo = guarded(move || {
            let (mut lo, mut hi) = (1usize, crate::parser::MAX_AST_DEPTH + 10);
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                if parses(&gen(mid)).is_ok() { lo = mid } else { hi = mid - 1 }
            }
            lo
        }).unwrap_or_else(|f| panic!("{name}: {f:?}"));
        assert!(lo >= 100, "{name}: ліміт надто тісний, дозволено лише {lo} рівнів");
        let src = gen(lo);
        let handle = std::thread::Builder::new().stack_size(small_stack)
            .spawn(move || front(src)).unwrap();
        let res = handle.join().unwrap_or_else(|_| panic!("{name}: паніка на глибині {lo}"));
        // Форматер/typechecker можуть мати власні зауваження; тут важливо лише, що не впало.
        if let Err(f) = res { assert!(!matches!(f, Fail::Panic(_) | Fail::Hang), "{name} (глибина {lo}): {f:?}"); }
    }
}


// ------------------------------------------------------------------ цілі: LSP-сервер
// Вхід — довільне повідомлення JSON-RPC (мутації справжніх запитів) і довільні байти кадрів.
// Сервер не повинен ні панікувати, ні зависати, ні віддавати некоректний JSON у відповідь.
fn lsp_prop(s: String) -> Result<(), Fail> {
    use crate::lsp::Server;
    let mut srv = Server::new();
    let doc = "struct Point { x: Number, y: Number }\nfn dist(a: Point) -> Number {\n    return a.x\n}\nfn main() -> Number {\n    let p = Point(1, 2)\n    return dist(p)\n}\n";
    let open = format!("{{\"method\":\"textDocument/didOpen\",\"params\":{{\"textDocument\":{{\"uri\":\"file:///f.obh\",\"text\":{}}}}}}}",
        crate::json::json_stringify(&crate::vm::Value::Str(doc.to_string())));
    let _ = srv.handle_body(&open);
    for out in srv.handle_body(&s) {
        if crate::json::json_parse(&out).is_err() {
            return Err(Fail::Property(format!("сервер віддав некоректний JSON: {}", out.chars().take(80).collect::<String>())));
        }
    }
    Ok(())
}

#[test]
fn fuzz_lsp_messages() {
    let u = "\"textDocument\":{\"uri\":\"file:///f.obh\"}";
    let seeds: Vec<String> = vec![
        format!("{{\"id\":1,\"method\":\"textDocument/hover\",\"params\":{{{u},\"position\":{{\"line\":6,\"character\":12}}}}}}"),
        format!("{{\"id\":2,\"method\":\"textDocument/definition\",\"params\":{{{u},\"position\":{{\"line\":6,\"character\":12}}}}}}"),
        format!("{{\"id\":3,\"method\":\"textDocument/completion\",\"params\":{{{u},\"position\":{{\"line\":2,\"character\":13}}}}}}"),
        format!("{{\"id\":4,\"method\":\"textDocument/formatting\",\"params\":{{{u}}}}}"),
        format!("{{\"id\":5,\"method\":\"textDocument/documentSymbol\",\"params\":{{{u}}}}}"),
        format!("{{\"method\":\"textDocument/didChange\",\"params\":{{{u},\"contentChanges\":[{{\"text\":\"fn main() -> Number {{ return (1 + }}\"}}]}}}}"),
        "{\"id\":6,\"method\":\"initialize\",\"params\":{}}".to_string(),
        "{\"id\":7,\"method\":\"shutdown\"}".to_string(),
    ];
    let mut c = Campaign::new("lsp_messages");
    c.run(iters(8000), |rng| if rng.chance(15) { String::from_utf8_lossy(&random_bytes(rng, 120)).into_owned() } else { mutate(rng, &seeds) },
          |s| guard_str(s, lsp_prop));
    c.finish();
}

#[test]
fn fuzz_lsp_framing() {
    let seeds: Vec<String> = vec![
        "Content-Length: 5\r\n\r\nhello".into(), "Content-Length: 2\r\nX: y\r\n\r\nok".into(),
        "content-length: 0\r\n\r\n".into(), "Content-Length: 99999999999\r\n\r\n".into(),
    ];
    let mut c = Campaign::new("lsp_framing");
    c.run(iters(8000), |rng| if rng.chance(25) { random_bytes(rng, 100).into_iter().map(|b| b as char).collect() } else { mutate(rng, &seeds) },
          |s| guard_str(s, |s| {
              // кілька кадрів поспіль із довільних байтів: помилка — нормально, паніка/зависання — ні
              let mut r = std::io::Cursor::new(s.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect::<Vec<u8>>());
              for _ in 0..4 { match crate::lsp::read_message(&mut r) { Ok(Some(_)) => {}, _ => break } }
              Ok(())
          }));
    c.finish();
}

/// Відтворювачі, знайдені нічним фазингом (workflow Fuzz), лишаються тут назавжди.
#[test]
fn fuzz_regressions_from_nightly_runs() {
    let cases: &[(&str, &str)] = &[
        ("форматер: шлях import з лапкою", "import\"\\\"\"fn n()->r{}"),
        ("форматер: коментар після багаторядкового match", "fn f(a:r)->r{\nmatch a{0=>1_=>2}//\n}"),
        ("форматер: `- 0` і знак нуля", "fn f()->Number{\n- 0\nf()\n}"),
        ("форматер: велике число не має ставати i64::MAX", "fn f()->Number{return 99999999999999999999}"),
        ("парсер: retries(-3) не має мовчки ставати 0", "resilient fn g(u:g)->S deadline(2s)retries(-3){}"),
        ("парсер: дробовий retries", "resilient fn g(u:g)->S deadline(2s)retries(0.5)fallback(h){}"),
        ("форматер: гігантська тривалість", "resilient fn r(c:g)->t<>deadline(9999999999999999991s)retryBudget(2){(y)}"),
        ("парсер: невідомий параметр budget", "resilient fn aA(q:g) -> ult<Str>\n deadline(10s)\n budget(ens: 0.5)\n{l\n}"),
    ];
    for (name, src) in cases {
        let r = guard_str(src, front);
        assert!(r.is_none(), "регресія `{name}`: {r:?}");
    }
}

// ------------------------------------------------------------------ цілі: хеші й кодування
// Довільний вхід не повинен ні панікувати, ні ламати властивості: encode -> decode повертає
// вихідні байти; decode довільного рядка або віддає байти, або Err (ніколи паніка); sha256 і
// HMAC завжди дають 32 байти й детерміновані.
fn crypto_prop(s: String) -> Result<(), Fail> {
    use crate::stdlib_crypto::*;
    let bytes = s.as_bytes();
    for url in [false, true] {
        let enc = base64_encode(bytes, url);
        match base64_decode(&enc, url) {
            Ok(back) if back == bytes => {}
            other => return Err(Fail::Property(format!("base64 (url={url}) не повертає вихідне: {:?}", other.map(|b| b.len())))),
        }
        let _ = base64_decode(&s, url); // довільний рядок як base64: Ok або Err, без паніки
    }
    match hex_decode(&hex_encode(bytes)) {
        Ok(back) if back == bytes => {}
        _ => return Err(Fail::Property("hex не повертає вихідне".into())),
    }
    let _ = hex_decode(&s);
    let d = sha256(bytes);
    if d != sha256(bytes) { return Err(Fail::Property("sha256 недетермінований".into())); }
    let key = &bytes[..bytes.len() / 2];
    let (a, b) = (hmac_sha256(key, &bytes[bytes.len() / 2..]), hmac_sha256(key, &bytes[bytes.len() / 2..]));
    if a != b || !constant_time_eq(&a, &b) { return Err(Fail::Property("hmac недетермінований".into())); }
    if bytes.len() > 0 && constant_time_eq(bytes, &d) && bytes != d { return Err(Fail::Property("constant_time_eq хибно рівний".into())); }
    Ok(())
}

#[test]
fn fuzz_crypto_builtins() {
    let seeds: Vec<String> = vec!["".into(), "foobar".into(), "Zm9vYmFy".into(), "00ff417a".into(), "-__-".into(), "привіт, світе 😀".into(), "====".into()];
    let mut c = Campaign::new("crypto");
    c.run(iters(20000), |rng| if rng.chance(30) { String::from_utf8_lossy(&random_bytes(rng, 200)).into_owned() } else { mutate(rng, &seeds) },
          |s| guard_str(s, crypto_prop));
    c.finish();
}
