//! Структуровані логи: один рядок = одна подія.
//!
//! Два формати:
//!   * **JSON** (для контейнерів і агрегаторів логів — Loki, ELK, CloudWatch):
//!     `{"ts":"2026-10-31T14:05:00.123Z","level":"info","msg":"запит","path":"/x","status":200}`
//!   * **text** (для термінала): `2026-10-31T14:05:00.123Z INFO  запит path=/x status=200`
//!
//! За замовчуванням: text, якщо stdout — термінал, інакше JSON. `OBERIH_LOG_FORMAT=json|text`
//! перевизначає. Мінімальний рівень: `OBERIH_LOG_LEVEL=debug|info|warn|error` (за замовчуванням
//! info). Логи йдуть у stdout (twelve-factor): збирати їх — справа платформи, а не програми.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::gc::MapKey;
use crate::vm::Value;

/// Максимальна кількість додаткових полів в одному записі (захист від роздування рядка).
const MAX_FIELDS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info  => "info",
            Level::Warn  => "warn",
            Level::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "debug"            => Some(Level::Debug),
            "info"             => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error"            => Some(Level::Error),
            _ => None,
        }
    }
}

/// Одне додаткове поле запису: ключ і його значення вже у двох виглядах.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key:  String,
    /// Значення як валідний JSON-літерал (для формату json).
    pub json: String,
    /// Значення для текстового формату.
    pub text: String,
}

impl Field {
    pub fn str(key: &str, value: &str) -> Field {
        Field { key: key.to_string(), json: json_quote(value), text: text_value(value) }
    }

    pub fn num(key: &str, value: f64) -> Field {
        let n = fmt_num(value);
        Field { key: key.to_string(), json: if value.is_finite() { n.clone() } else { "null".to_string() }, text: n }
    }

    pub fn from_value(key: &str, v: &Value) -> Field {
        // Числа йдуть через `num`: NaN/inf у JSON неприпустимі (стають null).
        if let Value::Num(n) = v {
            return Field::num(key, *n);
        }
        let text = match v {
            Value::Str(s) => text_value(s),
            other => other.to_string(),
        };
        Field { key: key.to_string(), json: crate::json::json_stringify(v), text }
    }
}

fn fmt_num(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

/// Рядок як JSON-літерал: екранує лапки, `\`, керівні символи; UTF-8 лишає як є.
pub fn json_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Значення в текстовому форматі: у лапках, якщо порожнє або містить пробіли/`=`/лапки.
fn text_value(s: &str) -> String {
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c == '=' || c == '"' || c.is_control()) {
        json_quote(s)
    } else {
        s.to_string()
    }
}

/// Повідомлення для текстового формату: керівні символи (зокрема переводи рядка) екрануються,
/// щоб значення не могло створити окремий «підроблений» рядок логу.
fn text_msg(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Ключ для текстового формату: пробіли, `=` і керівні символи замінюються на `_`.
fn text_key(k: &str) -> String {
    k.chars().map(|c| if c.is_whitespace() || c.is_control() || c == '=' { '_' } else { c }).collect()
}

fn is_reserved(key: &str) -> bool {
    matches!(key, "ts" | "level" | "msg")
}

/// Збирає один рядок логу. Чиста функція (без часу й вводу-виводу) — легко тестується.
pub fn format_line(ts: &str, level: Level, msg: &str, fields: &[Field], json: bool) -> String {
    let mut out = String::with_capacity(96 + msg.len());
    if json {
        out.push_str("{\"ts\":");
        out.push_str(&json_quote(ts));
        out.push_str(",\"level\":");
        out.push_str(&json_quote(level.as_str()));
        out.push_str(",\"msg\":");
        out.push_str(&json_quote(msg));
        for f in fields {
            // Зарезервовані імена не перезаписуємо — користувацьке поле отримує префікс.
            let key = if is_reserved(&f.key) { format!("field_{}", f.key) } else { f.key.clone() };
            out.push(',');
            out.push_str(&json_quote(&key));
            out.push(':');
            out.push_str(&f.json);
        }
        out.push('}');
    } else {
        out.push_str(ts);
        out.push(' ');
        out.push_str(&format!("{:<5}", level.as_str().to_uppercase()));
        out.push(' ');
        out.push_str(&text_msg(msg));
        for f in fields {
            out.push(' ');
            out.push_str(&text_key(&f.key));
            out.push('=');
            out.push_str(&f.text);
        }
    }
    out
}

// --- налаштування з середовища (читаються один раз) ---

static LEVEL: AtomicU8 = AtomicU8::new(u8::MAX);
static JSON:  AtomicU8 = AtomicU8::new(u8::MAX);

fn level_threshold() -> u8 {
    let v = LEVEL.load(Ordering::Relaxed);
    if v != u8::MAX {
        return v;
    }
    let lvl = std::env::var("OBERIH_LOG_LEVEL").ok().and_then(|s| Level::parse(&s)).unwrap_or(Level::Info);
    LEVEL.store(lvl as u8, Ordering::Relaxed);
    lvl as u8
}

fn use_json() -> bool {
    let v = JSON.load(Ordering::Relaxed);
    if v != u8::MAX {
        return v == 1;
    }
    let json = match std::env::var("OBERIH_LOG_FORMAT").ok().as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        Some(ref s) if s == "json" => true,
        Some(ref s) if s == "text" => false,
        _ => !std::io::stdout().is_terminal(),
    };
    JSON.store(if json { 1 } else { 0 }, Ordering::Relaxed);
    json
}

pub fn enabled(level: Level) -> bool {
    level as u8 >= level_threshold()
}

/// Записує подію в stdout (один `writeln!` під блокуванням — рядки різних потоків не перемішуються).
pub fn emit(level: Level, msg: &str, fields: &[Field]) {
    if !enabled(level) {
        return;
    }
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let line = format_line(&crate::stdlib_ext::iso_utc_millis(ms), level, msg, fields, use_json());
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{}", line);
}

/// Поля з Map мови (`logInfo("..", {"user": "ann", "n": 3})`) -> список полів.
pub fn fields_from_value(v: &Value) -> Result<Vec<Field>, String> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::Map(m) => {
            let mut out = Vec::new();
            for (k, val) in m.snapshot() {
                if out.len() >= MAX_FIELDS {
                    break;
                }
                let key = match k {
                    MapKey::Str(s)  => s,
                    MapKey::Num(b)  => fmt_num(f64::from_bits(b)),
                    MapKey::Bool(b) => b.to_string(),
                };
                out.push(Field::from_value(&key, &val));
            }
            Ok(out)
        }
        other => Err(format!("поля логу мають бути Map, отримано {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_parsing_and_ordering() {
        assert_eq!(Level::parse("INFO"), Some(Level::Info));
        assert_eq!(Level::parse(" warning "), Some(Level::Warn));
        assert_eq!(Level::parse("loud"), None);
        assert!(Level::Debug < Level::Info && Level::Info < Level::Warn && Level::Warn < Level::Error);
    }

    #[test]
    fn json_quote_escapes_everything_dangerous() {
        assert_eq!(json_quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_quote("рядок\n2\tтаб\r"), "\"рядок\\n2\\tтаб\\r\"");
        assert_eq!(json_quote("\u{1}"), "\"\\u0001\"");
        // юнікод лишається як є
        assert_eq!(json_quote("Київ 🚀"), "\"Київ 🚀\"");
    }

    #[test]
    fn json_line_is_valid_and_has_fields() {
        let fields = vec![Field::str("path", "/x"), Field::num("status", 200.0), Field::num("took_ms", 1.5)];
        let line = format_line("2026-10-31T14:05:00.123Z", Level::Info, "запит", &fields, true);
        assert_eq!(
            line,
            "{\"ts\":\"2026-10-31T14:05:00.123Z\",\"level\":\"info\",\"msg\":\"запит\",\"path\":\"/x\",\"status\":200,\"took_ms\":1.5}"
        );
        // і справді валідний JSON для нашого власного парсера
        let parsed = crate::json::json_parse(&line).unwrap();
        match parsed {
            Value::Map(m) => assert!(m.get(&MapKey::Str("level".to_string())).is_some()),
            other => panic!("очікувався об'єкт, отримано {:?}", other),
        }
    }

    #[test]
    fn newline_in_message_cannot_forge_a_second_log_line() {
        // Захист від log injection: значення з \n не може створити окремий рядок.
        let line = format_line("T", Level::Warn, "x\n{\"level\":\"error\"}", &[Field::str("u", "a\nb")], true);
        assert!(!line.contains('\n'), "{}", line);
    }

    #[test]
    fn text_format_also_cannot_forge_log_lines() {
        // У текстовому форматі повідомлення й ключі теж не можуть містити переводу рядка.
        let fields = vec![Field::str("k\nfake", "v\nsecond"), Field::str("a=b", "c")];
        let line = format_line("T", Level::Info, "first\nT ERROR forged", &fields, false);
        assert!(!line.contains('\n') && !line.contains('\r'), "{:?}", line);
        assert!(line.contains("first\\nT ERROR forged"), "{}", line);
        assert!(line.contains("k_fake="), "{}", line);
        assert!(line.contains("a_b=c"), "{}", line);
    }

    #[test]
    fn reserved_field_names_are_prefixed_not_overwritten() {
        let line = format_line("T", Level::Info, "m", &[Field::str("level", "evil"), Field::str("ts", "x")], true);
        assert!(line.contains("\"field_level\":\"evil\""), "{}", line);
        assert!(line.contains("\"field_ts\":\"x\""), "{}", line);
        assert!(line.contains("\"level\":\"info\""), "{}", line);
    }

    #[test]
    fn text_line_quotes_only_when_needed() {
        let fields = vec![Field::str("user", "ann"), Field::str("note", "two words"), Field::str("empty", ""), Field::num("n", 3.0)];
        let line = format_line("T", Level::Error, "boom", &fields, false);
        assert_eq!(line, "T ERROR boom user=ann note=\"two words\" empty=\"\" n=3");
        let w = format_line("T", Level::Warn, "m", &[], false);
        assert_eq!(w, "T WARN  m");
    }

    #[test]
    fn non_finite_numbers_become_null_in_json() {
        assert_eq!(Field::num("x", f64::NAN).json, "null");
        assert_eq!(Field::num("x", f64::INFINITY).json, "null");
        assert_eq!(Field::num("x", 42.0).json, "42");
    }
}
