//! LSP-сервер (Language Server Protocol) для Oberih: `oberih lsp`.
//!
//! Протокол: JSON-RPC по stdin/stdout із заголовком `Content-Length`. Усе, що не є
//! повідомленням протоколу, пишеться ЛИШЕ в stderr (stdout належить протоколу).
//!
//! Підтримується:
//!   * діагностика (лексер, парсер, typechecker) з точними діапазонами, колонки в UTF-16
//!     (як вимагає LSP), для didOpen/didChange/didSave; порожня при didClose;
//!   * форматування документа (той самий `Formatter`, що й `oberih fmt`);
//!   * структура документа (documentSymbol), перехід до визначення (go-to-definition),
//!     hover із сигнатурою, автодоповнення (ключові слова, типи, модифікатори, вбудовані
//!     функції, імена з документа, поля структур після `.`).
//!
//! Чесні межі: аналіз однофайловий (`import` не розгортається; typechecker і так не
//! вимагає знати імпортовані імена); немає rename, references, signatureHelp, семантичних
//! токенів; автодоповнення не залежить від типу виразу ліворуч від `.`.
//!
//! Сервер не повинен падати від довільного вводу: кожне повідомлення обробляється під
//! `catch_unwind`, розмір повідомлення й заголовка обмежені, позиції клієнта (будь-які,
//! навіть поза документом) безпечно обрізаються.

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::gc::MapKey;
use crate::json::{json_parse, json_stringify};
use crate::lexer;
use crate::parser::ast::{Item, Program};
use crate::parser;
use crate::vm::Value;

/// Максимальний розмір одного повідомлення (захист від вичерпання пам'яті).
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_HEADER_LINE: u64 = 8192;

pub const KEYWORDS: &[&str] = &[
    "fn", "resilient", "struct", "enum", "let", "return", "if", "else", "while", "for", "in",
    "match", "spawn", "import", "private", "true", "false", "Ok", "Err",
];
pub const MODIFIERS: &[&str] = &[
    "deadline", "retryBudget", "retries", "fallback", "timeout", "circuitBreaker", "idempotent",
    "cache", "emergencyFallback", "rateLimit", "bulkhead", "hedging", "durable", "traced", "budget",
];
pub const TYPES: &[&str] = &["Number", "String", "Bool", "List", "Map", "Result", "Fn"];
/// Вбудовані функції (звіряється з вихідним кодом тестом `builtin_list_matches_sources`).
pub const BUILTINS: &[&str] = &[
    "abs", "all", "any", "append", "appendFile", "args", "assert", "base64Decode",
    "base64DecodeToHex", "base64Encode", "base64EncodeHex", "base64UrlDecode",
    "base64UrlDecodeToHex", "base64UrlEncode", "base64UrlEncodeHex", "ceil", "concat",
    "constantTimeEq", "contains", "cos", "dateParts", "debug", "deleteFile", "each", "entries",
    "env", "exit", "exp", "fileExists", "filter", "find", "first", "floor", "gcCollectCycles",
    "gcStats", "gcstats", "hexDecode", "hexEncode", "hmacSha256", "hmacSha256Hex",
    "httpDelete", "httpGet", "httpPost", "httpPut", "indexOf", "isAlive", "isoTime",
    "jsonParse", "jsonPretty", "jsonStringify", "keys", "last", "len", "listDir", "log",
    "logDebug", "logError", "logInfo", "logWarn", "map", "mapDelete", "mapGet", "mapHas",
    "mapMerge", "mapSet", "max", "metricAdd", "metricInc", "metrics", "min", "mkdir", "now",
    "panic", "parseIso", "parseNumber", "pi", "pop", "pow", "print", "println", "push",
    "randInt", "random", "randomHex", "range", "reFind", "reFindAll", "reReplace",
    "reReplaceAll", "reSplit", "reTest", "readFile", "readLine", "readLines", "reduce",
    "regex", "reverse", "round", "seedRandom", "serve", "sha256", "sign", "sin", "sleep",
    "slice", "sort", "sortBy", "sqrt", "stopServer", "strContains", "strEndsWith",
    "strIndexOf", "strJoin", "strLen", "strLower", "strPadLeft", "strPadRight", "strRepeat",
    "strReplace", "strSlice", "strSplit", "strStartsWith", "strTrim", "strUpper", "sum", "tan",
    "timeFromParts", "toBool", "toNumber", "toString", "typeOf", "upgrade", "uuid", "values",
    "weakRef", "writeFile",
];

// ------------------------------------------------------------------ JSON-допоміжні
fn field(v: &Value, key: &str) -> Option<Value> {
    match v { Value::Map(m) => m.get(&MapKey::Str(key.to_string())), _ => None }
}
fn path<'a>(v: &Value, keys: &[&'a str]) -> Option<Value> {
    let mut cur = v.clone();
    for k in keys { cur = field(&cur, k)?; }
    Some(cur)
}
fn as_string(v: &Value) -> Option<String> { if let Value::Str(s) = v { Some(s.clone()) } else { None } }
fn as_index(v: &Value) -> Option<usize> {
    if let Value::Num(n) = v { if n.is_finite() && *n >= 0.0 { return Some(*n as usize); } }
    None
}
fn q(s: &str) -> String { json_stringify(&Value::Str(s.to_string())) }

// ------------------------------------------------------------------ фреймінг
/// Читає одне повідомлення. `Ok(None)` — кінець потоку.
pub fn read_message<R: BufRead>(r: &mut R) -> std::io::Result<Option<String>> {
    let mut content_len: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = r.by_ref().take(MAX_HEADER_LINE + 1).read_line(&mut line)?;
        if n == 0 { return Ok(None); }
        if n as u64 > MAX_HEADER_LINE {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "заголовок завеликий"));
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            if content_len.is_some() { break; }
            continue;
        }
        if let Some((k, v)) = l.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") { content_len = v.trim().parse().ok(); }
        }
    }
    let len = content_len.unwrap_or(0);
    if len > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "повідомлення завелике"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

pub fn write_message<W: Write>(w: &mut W, body: &str) -> std::io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    w.flush()
}

// ------------------------------------------------------------------ текст і позиції
fn lines_of(text: &str) -> Vec<&str> { text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect() }

fn utf16_len(s: &str) -> usize { s.chars().map(|c| c.len_utf16()).sum() }

/// Символьний індекс -> колонка UTF-16.
fn char_to_utf16(line: &str, char_idx: usize) -> usize { line.chars().take(char_idx).map(|c| c.len_utf16()).sum() }

/// Колонка UTF-16 -> символьний індекс (обрізається до довжини рядка).
fn utf16_to_char(line: &str, col16: usize) -> usize {
    let mut acc = 0;
    for (i, c) in line.chars().enumerate() {
        if acc >= col16 { return i; }
        acc += c.len_utf16();
    }
    line.chars().count()
}

fn is_ident_char(c: char) -> bool { c.is_alphanumeric() || c == '_' }

/// Слово під курсором: (початок, кінець) у символьних індексах.
fn word_at(line: &str, char_idx: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = char_idx.min(chars.len());
    if i == chars.len() || !is_ident_char(chars[i]) {
        if i > 0 && is_ident_char(chars[i - 1]) { i -= 1; } else { return None; }
    }
    let mut s = i;
    while s > 0 && is_ident_char(chars[s - 1]) { s -= 1; }
    let mut e = i;
    while e < chars.len() && is_ident_char(chars[e]) { e += 1; }
    Some((s, e))
}

fn range_json(line: usize, c0: usize, c1: usize, line_text: &str) -> String {
    let (a, b) = (char_to_utf16(line_text, c0), char_to_utf16(line_text, c1));
    format!("{{\"start\":{{\"line\":{},\"character\":{}}},\"end\":{{\"line\":{},\"character\":{}}}}}", line, a, line, b)
}

// ------------------------------------------------------------------ діагностика
pub struct Diag { pub line: usize, pub col: usize, pub end_col: usize, pub message: String }

/// Діагностика тексту (0-базові line/col у СИМВОЛАХ; перетворення в UTF-16 — при серіалізації).
pub fn diagnostics_for(text: &str) -> Vec<Diag> {
    let lines = lines_of(text);
    let mk = |line1: usize, col1: usize, msg: String| -> Diag {
        let line = line1.saturating_sub(1).min(lines.len().saturating_sub(1));
        let l = lines.get(line).copied().unwrap_or("");
        let nchars = l.chars().count();
        let col = col1.saturating_sub(1).min(nchars);
        let end_col = match word_at(l, col) { Some((s, e)) if s == col || s < col => e.max(col + 1).min(nchars.max(col)), _ => (col + 1).min(nchars.max(col)) };
        Diag { line, col, end_col: end_col.max(col), message: msg }
    };
    let (tokens, _c) = match lexer::Lexer::new(text).tokenize_with_comments() {
        Ok(t) => t,
        Err(e) => return vec![mk(e.line, e.col, e.message)],
    };
    let program = match parser::Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => return vec![mk(e.line, e.col, e.message)],
    };
    // Аналіз одного файлу: якщо є `import`, імпортованих імен ми не знаємо, тож перевірку
    // визначеності імен вимикаємо (інакше кожен виклик з модуля був би «невизначеним»).
    let has_imports = program.items.iter().any(|i| matches!(i, Item::Import(_)));
    let tc = crate::typechecker::Typechecker::new();
    let tc = if has_imports { tc.without_name_checks() } else { tc };
    match tc.check(&program) {
        Ok(_) => vec![],
        Err(errs) => errs.into_iter().map(|e| mk(e.line, e.col, e.message)).collect(),
    }
}

fn diagnostics_json(text: &str) -> String {
    let lines = lines_of(text);
    let items: Vec<String> = diagnostics_for(text).iter().map(|d| {
        let l = lines.get(d.line).copied().unwrap_or("");
        format!("{{\"range\":{},\"severity\":1,\"source\":\"oberih\",\"message\":{}}}",
                range_json(d.line, d.col, d.end_col, l), q(&d.message))
    }).collect();
    format!("[{}]", items.join(","))
}

// ------------------------------------------------------------------ аналіз документа
fn parse_doc(text: &str) -> Option<(Program, Vec<lexer::Comment>)> {
    let (tokens, comments) = lexer::Lexer::new(text).tokenize_with_comments().ok()?;
    let program = parser::Parser::new(tokens).parse_program().ok()?;
    Some((program, comments))
}

struct Decl { name: String, kind: &'static str, line: usize, col: usize, end_line: usize, sig: String, method_of: Option<String> }

fn decls_of(program: &Program) -> Vec<Decl> {
    let mut out = Vec::new();
    for item in &program.items {
        match item {
            Item::Fn(f) => {
                let params: Vec<String> = f.params.iter().map(|p| format!("{}: {}", p.name, crate::formatter::fmt_type(&p.ty))).collect();
                let q = if f.is_private { "private " } else { "" };
                let r = if f.is_resilient { "resilient " } else { "" };
                let full = match &f.method_of { Some(t) => format!("{}.{}", t, f.name), None => f.name.clone() };
                out.push(Decl { name: f.name.clone(), kind: "fn", line: f.span.line, col: f.span.col, end_line: f.end_line,
                    sig: format!("{}{}fn {}({}) -> {}", q, r, full, params.join(", "), crate::formatter::fmt_type(&f.return_type)),
                    method_of: f.method_of.clone() });
            }
            Item::Struct(s) => {
                let fields: Vec<String> = s.fields.iter().map(|f| format!("{}: {}", f.name, crate::formatter::fmt_type(&f.ty))).collect();
                out.push(Decl { name: s.name.clone(), kind: "struct", line: s.span.line, col: s.span.col, end_line: s.end_line,
                    sig: format!("struct {} {{ {} }}", s.name, fields.join(", ")), method_of: None });
            }
            Item::Enum(e) => {
                out.push(Decl { name: e.name.clone(), kind: "enum", line: e.span.line, col: e.span.col, end_line: e.end_line,
                    sig: format!("enum {} {{ {} }}", e.name, e.variants.join(", ")), method_of: None });
            }
            Item::Import(_) => {}
        }
    }
    out
}

/// Знаходить у рядку `line_idx` (0-базовому) діапазон імені `name` після колонки `from_col`.
fn name_range(lines: &[&str], line_idx: usize, from_col: usize, name: &str) -> (usize, usize) {
    let l = lines.get(line_idx).copied().unwrap_or("");
    let chars: Vec<char> = l.chars().collect();
    let nm: Vec<char> = name.chars().collect();
    if !nm.is_empty() && chars.len() >= nm.len() {
        let mut i = from_col.min(chars.len());
        while i + nm.len() <= chars.len() {
            let ends_ok = i + nm.len() == chars.len() || !is_ident_char(chars[i + nm.len()]);
            let starts_ok = i == 0 || !is_ident_char(chars[i - 1]);
            if chars[i..i + nm.len()] == nm[..] && ends_ok && starts_ok { return (i, i + nm.len()); }
            i += 1;
        }
    }
    (0, 0)
}


/// Імена, знайдені СТІЙКИМ сканом токенів. Під час набору коду документ майже завжди
/// синтаксично некоректний (`p.` без продовження), тож AST для автодоповнення не годиться.
#[derive(Default)]
struct Names { fns: Vec<String>, methods: Vec<String>, structs: Vec<String>, enums: Vec<String>, fields: Vec<String>, vars: Vec<String> }

fn scan_names(text: &str) -> Names {
    use lexer::Token as T;
    let mut n = Names::default();
    let Ok((toks, _)) = lexer::Lexer::new(text).tokenize_with_comments() else { return n };
    let mut i = 0;
    while i < toks.len() {
        match &toks[i].kind {
            T::Fn => {
                // `fn name`, `fn Type.name`; параметри (`ident :`) у дужках — теж змінні
                if let Some(T::Ident(a)) = toks.get(i + 1).map(|t| &t.kind) {
                    if matches!(toks.get(i + 2).map(|t| &t.kind), Some(T::Dot)) {
                        if let Some(T::Ident(b)) = toks.get(i + 3).map(|t| &t.kind) { n.methods.push(b.clone()); }
                    } else { n.fns.push(a.clone()); }
                }
            }
            T::Struct => {
                if let Some(T::Ident(name)) = toks.get(i + 1).map(|t| &t.kind) {
                    n.structs.push(name.clone());
                    // поля: `ident :` на глибині 1 всередині `{ … }`
                    let mut j = i + 2;
                    while j < toks.len() && !matches!(toks[j].kind, T::LBrace) && j < i + 40 { j += 1; }
                    let mut depth = 0;
                    while j < toks.len() {
                        match &toks[j].kind {
                            T::LBrace => depth += 1,
                            T::RBrace => { depth -= 1; if depth <= 0 { break; } }
                            T::Ident(f) if depth == 1 && matches!(toks.get(j + 1).map(|t| &t.kind), Some(T::Colon)) => n.fields.push(f.clone()),
                            _ => {}
                        }
                        j += 1;
                    }
                }
            }
            T::Enum => { if let Some(T::Ident(name)) = toks.get(i + 1).map(|t| &t.kind) { n.enums.push(name.clone()); } }
            T::Let => { if let Some(T::Ident(name)) = toks.get(i + 1).map(|t| &t.kind) { n.vars.push(name.clone()); } }
            T::Ident(name) if matches!(toks.get(i + 1).map(|t| &t.kind), Some(T::Colon)) => n.vars.push(name.clone()), // параметри
            _ => {}
        }
        i += 1;
    }
    n
}

// ------------------------------------------------------------------ сервер
#[derive(Default)]
pub struct Server { docs: HashMap<String, String>, pub shutdown_requested: bool, pub exit: bool }

impl Server {
    pub fn new() -> Self { Self::default() }

    /// Обробляє одне вхідне повідомлення (JSON-рядок) і повертає вихідні повідомлення.
    pub fn handle_body(&mut self, body: &str) -> Vec<String> {
        let msg = match json_parse(body) {
            Ok(v @ Value::Map(_)) => v,
            _ => return vec![error_response("null", -32700, "некоректний JSON")],
        };
        let id = field(&msg, "id").map(|v| json_stringify(&v));
        let r = catch_unwind(AssertUnwindSafe(|| self.dispatch(&msg, id.as_deref())));
        match r {
            Ok(v) => v,
            Err(_) => match id { Some(i) => vec![error_response(&i, -32603, "внутрішня помилка сервера")], None => vec![] },
        }
    }

    fn dispatch(&mut self, msg: &Value, id: Option<&str>) -> Vec<String> {
        let method = field(msg, "method").and_then(|v| as_string(&v)).unwrap_or_default();
        let params = field(msg, "params").unwrap_or(Value::Nil);
        let reply = |result: String| -> Vec<String> {
            match id { Some(i) => vec![format!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{}}}", i, result)], None => vec![] }
        };
        match method.as_str() {
            "initialize" => reply(format!(
                "{{\"capabilities\":{{\"textDocumentSync\":{{\"openClose\":true,\"change\":1,\"save\":{{}}}},\
                 \"completionProvider\":{{\"triggerCharacters\":[\".\"]}},\"documentFormattingProvider\":true,\
                 \"documentSymbolProvider\":true,\"definitionProvider\":true,\"hoverProvider\":true}},\
                 \"serverInfo\":{{\"name\":\"oberih\",\"version\":{}}}}}", q(env!("CARGO_PKG_VERSION")))),
            "initialized" => vec![],
            "shutdown" => { self.shutdown_requested = true; reply("null".into()) }
            "exit" => { self.exit = true; vec![] }
            "textDocument/didOpen" => {
                let (Some(uri), Some(text)) = (path(&params, &["textDocument", "uri"]).and_then(|v| as_string(&v)),
                                               path(&params, &["textDocument", "text"]).and_then(|v| as_string(&v))) else { return vec![] };
                self.docs.insert(uri.clone(), text.clone());
                vec![publish(&uri, &diagnostics_json(&text))]
            }
            "textDocument/didChange" => {
                let Some(uri) = path(&params, &["textDocument", "uri"]).and_then(|v| as_string(&v)) else { return vec![] };
                // Повна синхронізація (change=1): беремо ОСТАННЮ зміну.
                let text = match field(&params, "contentChanges") {
                    Some(Value::List(l)) => l.0.lock().unwrap().last().and_then(|c| field(c, "text")).and_then(|v| as_string(&v)),
                    _ => None,
                };
                let Some(text) = text else { return vec![] };
                self.docs.insert(uri.clone(), text.clone());
                vec![publish(&uri, &diagnostics_json(&text))]
            }
            "textDocument/didSave" => {
                let Some(uri) = path(&params, &["textDocument", "uri"]).and_then(|v| as_string(&v)) else { return vec![] };
                match self.docs.get(&uri) { Some(t) => vec![publish(&uri, &diagnostics_json(t))], None => vec![] }
            }
            "textDocument/didClose" => {
                let Some(uri) = path(&params, &["textDocument", "uri"]).and_then(|v| as_string(&v)) else { return vec![] };
                self.docs.remove(&uri);
                vec![publish(&uri, "[]")]
            }
            "textDocument/formatting" => reply(self.formatting(&params)),
            "textDocument/documentSymbol" => reply(self.document_symbols(&params)),
            "textDocument/definition" => reply(self.definition(&params)),
            "textDocument/hover" => reply(self.hover(&params)),
            "textDocument/completion" => reply(self.completion(&params)),
            _ => match id {
                // Невідомий запит: «метод не знайдено»; невідомі сповіщення ігноруємо.
                Some(i) if !method.starts_with("$/") => vec![error_response(i, -32601, "метод не підтримується")],
                Some(i) => vec![error_response(i, -32601, "метод не підтримується")],
                None => vec![],
            },
        }
    }

    fn doc_and_pos(&self, params: &Value) -> Option<(String, String, usize, usize)> {
        let uri = path(params, &["textDocument", "uri"]).and_then(|v| as_string(&v))?;
        let text = self.docs.get(&uri)?.clone();
        let line = path(params, &["position", "line"]).and_then(|v| as_index(&v)).unwrap_or(0);
        let ch = path(params, &["position", "character"]).and_then(|v| as_index(&v)).unwrap_or(0);
        Some((uri, text, line, ch))
    }

    fn formatting(&self, params: &Value) -> String {
        let Some(uri) = path(params, &["textDocument", "uri"]).and_then(|v| as_string(&v)) else { return "null".into() };
        let Some(text) = self.docs.get(&uri) else { return "null".into() };
        let Some((program, comments)) = parse_doc(text) else { return "null".into() };
        let formatted = crate::formatter::Formatter::new().format_program_with_comments(&program, comments);
        if !crate::formatter::parses(&formatted) { return "null".into(); } // не пропонуємо правку, що ламає код
        if &formatted == text { return "[]".into(); }
        let lines = lines_of(text);
        let last = lines.len().saturating_sub(1);
        let end_c = utf16_len(lines.get(last).copied().unwrap_or(""));
        format!("[{{\"range\":{{\"start\":{{\"line\":0,\"character\":0}},\"end\":{{\"line\":{},\"character\":{}}}}},\"newText\":{}}}]",
                last, end_c, q(&formatted))
    }

    fn document_symbols(&self, params: &Value) -> String {
        let Some(uri) = path(params, &["textDocument", "uri"]).and_then(|v| as_string(&v)) else { return "[]".into() };
        let Some(text) = self.docs.get(&uri) else { return "[]".into() };
        let Some((program, _)) = parse_doc(text) else { return "[]".into() };
        let lines = lines_of(text);
        let syms: Vec<String> = decls_of(&program).iter().map(|d| {
            let l0 = d.line.saturating_sub(1).min(lines.len().saturating_sub(1));
            let l1 = d.end_line.saturating_sub(1).max(l0).min(lines.len().saturating_sub(1));
            let (n0, n1) = name_range(&lines, l0, d.col.saturating_sub(1), &d.name);
            let end_c = utf16_len(lines.get(l1).copied().unwrap_or(""));
            let kind = match (d.kind, &d.method_of) { ("fn", Some(_)) => 6, ("fn", None) => 12, ("struct", _) => 23, _ => 10 };
            let shown = match &d.method_of { Some(t) => format!("{}.{}", t, d.name), None => d.name.clone() };
            format!("{{\"name\":{},\"kind\":{},\"range\":{{\"start\":{{\"line\":{},\"character\":0}},\"end\":{{\"line\":{},\"character\":{}}}}},\"selectionRange\":{}}}",
                    q(&shown), kind, l0, l1, end_c, range_json(l0, n0, n1, lines.get(l0).copied().unwrap_or("")))
        }).collect();
        format!("[{}]", syms.join(","))
    }

    fn ident_under_cursor(&self, params: &Value) -> Option<(String, String, String)> {
        let (uri, text, line, ch) = self.doc_and_pos(params)?;
        let lines = lines_of(&text);
        let l = lines.get(line).copied()?;
        let (s, e) = word_at(l, utf16_to_char(l, ch))?;
        let word: String = l.chars().skip(s).take(e - s).collect();
        Some((uri, text, word))
    }

    fn definition(&self, params: &Value) -> String {
        let Some((uri, text, word)) = self.ident_under_cursor(params) else { return "null".into() };
        let Some((program, _)) = parse_doc(&text) else { return "null".into() };
        let lines = lines_of(&text);
        for d in decls_of(&program) {
            if d.name == word {
                let l0 = d.line.saturating_sub(1).min(lines.len().saturating_sub(1));
                let (n0, n1) = name_range(&lines, l0, d.col.saturating_sub(1), &d.name);
                return format!("{{\"uri\":{},\"range\":{}}}", q(&uri), range_json(l0, n0, n1, lines.get(l0).copied().unwrap_or("")));
            }
        }
        "null".into()
    }

    fn hover(&self, params: &Value) -> String {
        let Some((_uri, text, word)) = self.ident_under_cursor(params) else { return "null".into() };
        let md = if let Some((program, _)) = parse_doc(&text) {
            decls_of(&program).into_iter().find(|d| d.name == word).map(|d| format!("```oberih\n{}\n```", d.sig))
        } else { None };
        let md = md.or_else(|| if BUILTINS.contains(&word.as_str()) { Some(format!("вбудована функція `{}`", word)) }
                                else if KEYWORDS.contains(&word.as_str()) { Some(format!("ключове слово `{}`", word)) }
                                else if MODIFIERS.contains(&word.as_str()) { Some(format!("модифікатор стійкості `{}`", word)) }
                                else { None });
        match md {
            Some(m) => format!("{{\"contents\":{{\"kind\":\"markdown\",\"value\":{}}}}}", q(&m)),
            None => "null".into(),
        }
    }

    fn completion(&self, params: &Value) -> String {
        let Some((_uri, text, line, ch)) = self.doc_and_pos(params) else { return "[]".into() };
        let lines = lines_of(&text);
        let l = lines.get(line).copied().unwrap_or("");
        let cidx = utf16_to_char(l, ch);
        let chars: Vec<char> = l.chars().collect();
        // Початок слова, яке дописується; символ перед ним.
        let mut s = cidx.min(chars.len());
        while s > 0 && is_ident_char(chars[s - 1]) { s -= 1; }
        let after_dot = s > 0 && chars[s - 1] == '.';
        let mut items: Vec<(String, u32)> = Vec::new();
        let names = scan_names(&text);
        if after_dot {
            for f in &names.fields { items.push((f.clone(), 5)); }
            for m in &names.methods { items.push((m.clone(), 2)); }
        } else {
            for k in KEYWORDS { items.push((k.to_string(), 14)); }
            for k in TYPES { items.push((k.to_string(), 7)); }
            for k in MODIFIERS { items.push((k.to_string(), 14)); }
            for k in BUILTINS { items.push((k.to_string(), 3)); }
            for f in &names.fns { items.push((f.clone(), 3)); }
            for st in &names.structs { items.push((st.clone(), 22)); }
            for e in &names.enums { items.push((e.clone(), 13)); }
            for v in &names.vars { items.push((v.clone(), 6)); }
        }
        items.sort(); items.dedup_by(|a, b| a.0 == b.0);
        let out: Vec<String> = items.iter().map(|(n, k)| format!("{{\"label\":{},\"kind\":{}}}", q(n), k)).collect();
        format!("[{}]", out.join(","))
    }
}

fn publish(uri: &str, diags_json: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{{\"uri\":{},\"diagnostics\":{}}}}}", q(uri), diags_json)
}

fn error_response(id: &str, code: i32, message: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{},\"message\":{}}}}}", id, code, q(message))
}

/// Головний цикл: stdin -> stdout. Код виходу за специфікацією LSP: 0, якщо перед `exit`
/// був `shutdown`, інакше 1.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut server = Server::new();
    loop {
        match read_message(&mut reader) {
            Ok(Some(body)) => {
                for m in server.handle_body(&body) {
                    if write_message(&mut out, &m).is_err() { return 1; }
                }
                if server.exit { break; }
            }
            Ok(None) => break,
            Err(e) => { eprintln!("oberih lsp: помилка читання: {}", e); return 1; }
        }
    }
    if server.shutdown_requested { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const URI: &str = "file:///t/x.obh";

    fn jv(s: &str) -> Value { json_parse(s).unwrap_or_else(|e| panic!("некоректний JSON від сервера: {e}\n{s}")) }
    fn gets(v: &Value, keys: &[&str]) -> Value { path(v, keys).unwrap_or_else(|| panic!("немає шляху {keys:?}")) }
    fn list(v: &Value) -> Vec<Value> { match v { Value::List(l) => l.0.lock().unwrap().clone(), other => panic!("очікувався список: {other:?}") } }
    fn num(v: &Value) -> f64 { match v { Value::Num(n) => *n, o => panic!("очікувалось число: {o:?}") } }

    fn open(srv: &mut Server, text: &str) -> Vec<Value> {
        let body = format!("{{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/didOpen\",\"params\":{{\"textDocument\":{{\"uri\":{},\"text\":{}}}}}}}", q(URI), q(text));
        srv.handle_body(&body).iter().map(|m| jv(m)).collect()
    }
    fn request(srv: &mut Server, id: u32, method: &str, params: &str) -> Value {
        let out = srv.handle_body(&format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"{method}\",\"params\":{params}}}"));
        assert_eq!(out.len(), 1, "очікувалась одна відповідь на {method}: {out:?}");
        jv(&out[0])
    }
    fn at(line: usize, ch: usize) -> String { format!("{{\"textDocument\":{{\"uri\":{}}},\"position\":{{\"line\":{line},\"character\":{ch}}}}}", q(URI)) }
    fn diags(msgs: &[Value]) -> Vec<Value> { list(&gets(&msgs[0], &["params", "diagnostics"])) }

    const GOOD: &str = "struct Point { x: Number, y: Number }\nfn dist(a: Point, b: Point) -> Number {\n    return a.x - b.x\n}\nfn main() -> Number {\n    let p = Point(1, 2)\n    return dist(p, p)\n}\n";

    // ----- фреймінг -----
    #[test]
    fn framing_reads_crlf_extra_headers_and_case_insensitive_length() {
        let raw = "Content-Type: application/vscode-jsonrpc\r\ncontent-length: 5\r\n\r\nhello".to_string() + "Content-Length: 2\r\n\r\nok";
        let mut r = Cursor::new(raw.into_bytes());
        assert_eq!(read_message(&mut r).unwrap().as_deref(), Some("hello"));
        assert_eq!(read_message(&mut r).unwrap().as_deref(), Some("ok"));
        assert_eq!(read_message(&mut r).unwrap(), None, "кінець потоку");
    }

    #[test]
    fn framing_rejects_oversized_message_and_header_without_allocating() {
        let mut r = Cursor::new(format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1).into_bytes());
        assert!(read_message(&mut r).is_err(), "завелике повідомлення має відхилятись");
        let mut r = Cursor::new(format!("X: {}\r\n\r\n", "a".repeat(20_000)).into_bytes());
        assert!(read_message(&mut r).is_err(), "завеликий заголовок має відхилятись");
        let mut r = Cursor::new(b"Content-Length: 10\r\n\r\nshort".to_vec());
        assert!(read_message(&mut r).is_err(), "обірване тіло — помилка, а не паніка");
    }

    #[test]
    fn framing_counts_bytes_not_chars() {
        let mut out = Vec::new();
        write_message(&mut out, "привіт").unwrap();
        assert!(String::from_utf8(out.clone()).unwrap().starts_with("Content-Length: 12\r\n\r\n"));
        assert_eq!(read_message(&mut Cursor::new(out)).unwrap().as_deref(), Some("привіт"));
    }

    // ----- діагностика -----
    #[test]
    fn valid_document_has_no_diagnostics() {
        let mut s = Server::new();
        assert!(diags(&open(&mut s, GOOD)).is_empty());
    }

    #[test]
    fn syntax_error_is_reported_at_its_position() {
        let mut s = Server::new();
        let d = diags(&open(&mut s, "fn main() -> Number {\n    let x = (1 +\n    return 0\n}\n"));
        assert_eq!(d.len(), 1);
        assert_eq!(num(&gets(&d[0], &["range", "start", "line"])), 2.0, "помилка на 3-му рядку (0-базово 2)");
        assert_eq!(gets(&d[0], &["source"]), Value::Str("oberih".into()));
    }

    #[test]
    fn type_error_has_real_coordinates_unlike_the_python_prototype() {
        let mut s = Server::new();
        let d = diags(&open(&mut s, "fn main() -> Number {\n    let x: Number = \"a\"\n    return x\n}\n"));
        assert!(!d.is_empty(), "очікувалась помилка типу");
        assert_eq!(num(&gets(&d[0], &["range", "start", "line"])), 1.0, "прив'язка до рядка 2, а не до рядка 1");
    }

    #[test]
    fn columns_are_utf16_code_units() {
        // Емодзі = 1 символ, але 2 одиниці UTF-16: колонка помилки зсувається на 1.
        let mut s = Server::new();
        let d = diags(&open(&mut s, "fn main() -> Number {\n    let 😀d = 1\n    return 0\n}\n"));
        assert_eq!(d.len(), 1);
        let (a, b) = (num(&gets(&d[0], &["range", "start", "character"])), num(&gets(&d[0], &["range", "end", "character"])));
        assert_eq!(a, 8.0);
        assert_eq!(b - a, 2.0, "діапазон емодзі — 2 одиниці UTF-16");
    }

    #[test]
    fn change_replaces_diagnostics_and_close_clears_them() {
        let mut s = Server::new();
        assert!(!diags(&open(&mut s, "fn main( {")).is_empty());
        let body = format!("{{\"method\":\"textDocument/didChange\",\"params\":{{\"textDocument\":{{\"uri\":{}}},\"contentChanges\":[{{\"text\":\"ignored\"}},{{\"text\":{}}}]}}}}", q(URI), q(GOOD));
        let out: Vec<Value> = s.handle_body(&body).iter().map(|m| jv(m)).collect();
        assert!(diags(&out).is_empty(), "береться ОСТАННЯ зміна (повна синхронізація)");
        let out: Vec<Value> = s.handle_body(&format!("{{\"method\":\"textDocument/didClose\",\"params\":{{\"textDocument\":{{\"uri\":{}}}}}}}", q(URI))).iter().map(|m| jv(m)).collect();
        assert!(diags(&out).is_empty());
    }

    #[test]
    fn single_letter_struct_is_not_mistaken_for_a_generic() {
        // Регресія: `struct P` + `a.x` давали хибне «доступ до поля на не-struct P».
        let mut s = Server::new();
        let src = "struct P { x: Number }\nfn f(a: P) -> Number {\n    return a.x\n}\nfn main() -> Number {\n    return f(P(1))\n}\n";
        assert!(diags(&open(&mut s, src)).is_empty());
        // Контроль: справжній дженерик `T` далі працює.
        let src2 = "fn id<T>(x: T) -> T {\n    return x\n}\nfn main() -> Number {\n    return id(1)\n}\n";
        assert!(diags(&open(&mut s, src2)).is_empty());
    }

    // ----- запити -----
    #[test]
    fn initialize_advertises_implemented_capabilities_only() {
        let mut s = Server::new();
        let r = request(&mut s, 1, "initialize", "{}");
        let caps = gets(&r, &["result", "capabilities"]);
        for k in ["documentFormattingProvider", "documentSymbolProvider", "definitionProvider", "hoverProvider"] {
            assert_eq!(gets(&caps, &[k]), Value::Bool(true), "{k}");
        }
        assert!(path(&caps, &["renameProvider"]).is_none() && path(&caps, &["referencesProvider"]).is_none(), "не обіцяємо того, чого немає");
    }

    #[test]
    fn document_symbols_list_items_with_kinds_and_name_ranges() {
        let mut s = Server::new();
        open(&mut s, GOOD);
        let r = request(&mut s, 2, "textDocument/documentSymbol", &format!("{{\"textDocument\":{{\"uri\":{}}}}}", q(URI)));
        let syms = list(&gets(&r, &["result"]));
        let names: Vec<String> = syms.iter().map(|x| as_string(&gets(x, &["name"])).unwrap()).collect();
        assert_eq!(names, vec!["Point", "dist", "main"]);
        assert_eq!(num(&gets(&syms[0], &["kind"])), 23.0); // Struct
        assert_eq!(num(&gets(&syms[1], &["kind"])), 12.0); // Function
        // selectionRange вказує на САМЕ ім'я: `fn dist` -> колонки 3..7
        assert_eq!(num(&gets(&syms[1], &["selectionRange", "start", "character"])), 3.0);
        assert_eq!(num(&gets(&syms[1], &["selectionRange", "end", "character"])), 7.0);
        assert_eq!(num(&gets(&syms[1], &["range", "end", "line"])), 3.0, "функція закінчується на 4-му рядку");
    }

    #[test]
    fn definition_jumps_from_call_to_declaration_and_unknown_is_null() {
        let mut s = Server::new();
        open(&mut s, GOOD);
        // `dist(p, p)` на рядку 6, колонка 11
        let r = request(&mut s, 3, "textDocument/definition", &at(6, 11));
        assert_eq!(num(&gets(&r, &["result", "range", "start", "line"])), 1.0);
        assert_eq!(num(&gets(&r, &["result", "range", "start", "character"])), 3.0);
        // на пробілі / літералі — нічого
        assert_eq!(gets(&request(&mut s, 4, "textDocument/definition", &at(6, 0)), &["result"]), Value::Nil);
        assert_eq!(gets(&request(&mut s, 5, "textDocument/definition", &at(500, 500)), &["result"]), Value::Nil, "позиція поза документом не панікує");
    }

    #[test]
    fn hover_shows_signature_and_describes_builtins() {
        let mut s = Server::new();
        open(&mut s, "fn add(a: Number, b: Number) -> Number {\n    return a + b\n}\nfn main() -> Number {\n    println(\"x\")\n    return add(1, 2)\n}\n");
        let h = request(&mut s, 6, "textDocument/hover", &at(5, 12));
        let v = as_string(&gets(&h, &["result", "contents", "value"])).unwrap();
        assert!(v.contains("fn add(a: Number, b: Number) -> Number"), "{v}");
        let h = request(&mut s, 7, "textDocument/hover", &at(4, 6));
        assert!(as_string(&gets(&h, &["result", "contents", "value"])).unwrap().contains("println"));
    }

    #[test]
    fn completion_offers_language_builtins_and_document_names_and_fields_after_dot() {
        let mut s = Server::new();
        open(&mut s, GOOD);
        let r = request(&mut s, 8, "textDocument/completion", &at(6, 11));
        let labels: Vec<String> = list(&gets(&r, &["result"])).iter().map(|x| as_string(&gets(x, &["label"])).unwrap()).collect();
        for want in ["fn", "match", "Number", "deadline", "println", "httpGet", "dist", "Point", "p"] {
            assert!(labels.iter().any(|l| l == want), "немає `{want}` в автодоповненні");
        }
        // Після `.` — поля структур, а не ключові слова.
        open(&mut s, "struct Point { x: Number, y: Number }\nfn main() -> Number {\n    let p = Point(1, 2)\n    return p.\n}\n");
        let r = request(&mut s, 9, "textDocument/completion", &at(3, 13));
        let labels: Vec<String> = list(&gets(&r, &["result"])).iter().map(|x| as_string(&gets(x, &["label"])).unwrap()).collect();
        assert!(labels.contains(&"x".to_string()) && labels.contains(&"y".to_string()), "{labels:?}");
        assert!(!labels.contains(&"fn".to_string()));
    }

    #[test]
    fn formatting_returns_one_full_document_edit_matching_oberih_fmt() {
        let mut s = Server::new();
        open(&mut s, "fn main()->Number{\nlet x=1\nreturn x}\n");
        let r = request(&mut s, 10, "textDocument/formatting", &format!("{{\"textDocument\":{{\"uri\":{}}}}}", q(URI)));
        let edits = list(&gets(&r, &["result"]));
        assert_eq!(edits.len(), 1);
        let new = as_string(&gets(&edits[0], &["newText"])).unwrap();
        let (p, c) = parse_doc("fn main()->Number{\nlet x=1\nreturn x}\n").unwrap();
        assert_eq!(new, crate::formatter::Formatter::new().format_program_with_comments(&p, c));
        assert!(new.contains("    let x = 1"), "{new}");
        // вже відформатований текст — порожній список правок; код з помилкою — null
        open(&mut s, &new);
        assert!(list(&gets(&request(&mut s, 11, "textDocument/formatting", &format!("{{\"textDocument\":{{\"uri\":{}}}}}", q(URI))), &["result"])).is_empty());
        open(&mut s, "fn (((");
        assert_eq!(gets(&request(&mut s, 12, "textDocument/formatting", &format!("{{\"textDocument\":{{\"uri\":{}}}}}", q(URI))), &["result"]), Value::Nil);
    }

    // ----- життєвий цикл і стійкість -----
    #[test]
    fn shutdown_then_exit_and_unknown_requests() {
        let mut s = Server::new();
        let r = request(&mut s, 20, "no/such", "{}");
        assert_eq!(num(&gets(&r, &["error", "code"])), -32601.0);
        assert!(s.handle_body("{\"method\":\"no/such/notification\"}").is_empty(), "невідоме сповіщення ігнорується");
        assert_eq!(gets(&request(&mut s, 21, "shutdown", "null"), &["result"]), Value::Nil);
        assert!(s.shutdown_requested && !s.exit);
        assert!(s.handle_body("{\"method\":\"exit\"}").is_empty());
        assert!(s.exit);
    }

    #[test]
    fn garbage_input_never_panics_and_gets_a_parse_error() {
        let mut s = Server::new();
        for bad in ["", "{", "[1,2", "null", "42", "\"x\"", "{\"id\":1}", "{\"id\":1,\"method\":5}", "{\"method\":\"textDocument/hover\",\"id\":3,\"params\":5}"] {
            let _ = s.handle_body(bad); // головне — без паніки
        }
        let out = s.handle_body("{ не json");
        assert_eq!(num(&gets(&jv(&out[0]), &["error", "code"])), -32700.0);
        // Позиції: від'ємні, дробові, величезні.
        open(&mut s, GOOD);
        for pos in ["{\"line\":-1,\"character\":-5}", "{\"line\":1e300,\"character\":1e300}", "{\"line\":0.5,\"character\":\"x\"}", "{}", "null"] {
            for m in ["hover", "definition", "completion"] {
                let _ = s.handle_body(&format!("{{\"id\":1,\"method\":\"textDocument/{m}\",\"params\":{{\"textDocument\":{{\"uri\":{}}},\"position\":{pos}}}}}", q(URI)));
            }
        }
    }

    // ----- списки імен не розходяться з рантаймом -----
    #[test]
    fn builtin_list_matches_sources() {
        for b in BUILTINS { assert!(crate::stdlib::is_builtin(b), "`{b}` у BUILTINS, але не є вбудованою функцією"); }
        // Усі імена з `matches!(name, …)` у вихідному коді мають бути в BUILTINS.
        fn names_in(src: &str, marker: &str) -> Vec<String> {
            let i = src.find(marker).unwrap_or_else(|| panic!("немає {marker}"));
            let j = i + src[i..].find("matches!(").unwrap() + "matches!".len();
            let (mut depth, mut instr, mut prev) = (0i32, false, ' ');
            let mut end = j;
            for (k, c) in src[j..].char_indices() {
                if instr { if c == '"' && prev != '\\' { instr = false; } }
                else if c == '"' { instr = true; }
                else if c == '(' { depth += 1; }
                else if c == ')' { depth -= 1; if depth == 0 { end = j + k; break; } }
                prev = c;
            }
            let body: String = src[j..end].lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
            body.split('"').enumerate().filter(|(n, _)| n % 2 == 1).map(|(_, s)| s.to_string())
                .filter(|s| s.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_')).collect()
        }
        let mut all = names_in(include_str!("stdlib.rs"), "fn is_core_builtin");
        all.extend(names_in(include_str!("stdlib_ext.rs"), "pub fn is_ext"));
        all.extend(names_in(include_str!("stdlib_ext.rs"), "fn is_higher_order"));
        all.extend(names_in(include_str!("regex_ops.rs"), "pub fn is_ext"));
        all.extend(names_in(include_str!("stdlib_crypto.rs"), "pub fn is_ext"));
        assert!(all.len() > 100, "парсер списку зламався: {}", all.len());
        for n in all.iter().filter(|n| !n.starts_with("__")) {
            assert!(BUILTINS.contains(&n.as_str()), "вбудована функція `{n}` відсутня в lsp::BUILTINS (оновіть список)");
        }
    }

    #[test]
    fn keyword_and_modifier_lists_are_real() {
        for k in KEYWORDS {
            let toks = lexer::Lexer::new(k).tokenize().unwrap();
            assert!(!matches!(toks[0].kind, lexer::Token::Ident(_)), "`{k}` лексер вважає звичайним іменем");
        }
        // Кожен модифікатор парситься як модифікатор функції.
        for m in MODIFIERS {
            let arg = match *m { "deadline" | "timeout" => "(1s)".to_string(), "fallback" => "(0)".into(), _ => "(1)".to_string() };
            let src = format!("resilient fn f() -> Number\n    {m}{arg}\n{{\n    return 1\n}}\n");
            let toks = lexer::Lexer::new(&src).tokenize().unwrap();
            let r = parser::Parser::new(toks).parse_program();
            assert!(!matches!(&r, Err(e) if e.message.contains("модифікатор")), "`{m}` не розпізнається як модифікатор: {r:?}");
        }
    }

    #[test]
    fn editor_grammar_covers_all_language_names() {
        // Граматика підсвітки VS Code згенерована зі списків вище; тест не дає їм розійтись
        // (раніше граматика вміла лише стару мову: без enum/match/for/spawn/import/Ok/Err).
        let g = include_str!("../editor/vscode-oberih/syntaxes/oberih.tmLanguage.json");
        for n in KEYWORDS.iter().chain(MODIFIERS).chain(TYPES).chain(BUILTINS) {
            assert!(g.contains(n), "`{n}` відсутній у граматиці editor/vscode-oberih (перегенеруйте її)");
        }
        let ext = include_str!("../editor/vscode-oberih/extension.js");
        assert!(ext.contains("args: [\"lsp\"]") && !ext.contains("python"), "розширення має запускати `oberih lsp`, а не Python");
        let pkg = include_str!("../editor/vscode-oberih/package.json");
        assert!(pkg.contains("oberih.serverPath") && !pkg.contains("pythonPath"));
    }

    #[test]
    fn undefined_names_are_diagnosed_with_position_but_not_when_imports_exist() {
        let mut s = Server::new();
        let d = diags(&open(&mut s, "fn main() -> Number {\n    let x = fooo(1)\n    return 0\n}\n"));
        assert_eq!(d.len(), 1);
        assert!(as_string(&gets(&d[0], &["message"])).unwrap().contains("Невизначена функція 'fooo'"));
        assert_eq!(num(&gets(&d[0], &["range", "start", "line"])), 1.0);
        assert_eq!(num(&gets(&d[0], &["range", "start", "character"])), 12.0);
        // З `import` імена модуля невідомі аналізу одного файлу: без хибних помилок.
        let d = diags(&open(&mut s, "import \"util.obh\"\nfn main() -> Number {\n    let x = fromUtil(1)\n    return 0\n}\n"));
        assert!(d.is_empty(), "{d:?}");
    }
}
