/// Oberih JSON — власний парсер і серіалізатор.
/// Нуль залежностей, повна підтримка JSON spec.
///
/// Функції:
///   jsonParse(str)       -> Ok(Value) | Err(String)
///   jsonStringify(value) -> String

use crate::vm::{Value, OberihStruct};
use crate::gc::GcList;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Публічний API
// ---------------------------------------------------------------------------

pub fn json_parse(input: &str) -> Result<Value, String> {
    let input = input.trim();
    let mut parser = JsonParser::new(input);
    let val = parser.parse_value()?;
    parser.skip_ws();
    if parser.pos < parser.chars.len() {
        return Err(format!("Зайвий текст після JSON на позиції {}", parser.pos));
    }
    Ok(val)
}

pub fn json_stringify(value: &Value) -> String {
    stringify_value(value, 0, false)
}

pub fn json_stringify_pretty(value: &Value) -> String {
    stringify_value(value, 0, true)
}

// ---------------------------------------------------------------------------
// Парсер
// ---------------------------------------------------------------------------

struct JsonParser {
    chars: Vec<char>,
    pos:   usize,
}

impl JsonParser {
    fn new(s: &str) -> Self {
        JsonParser { chars: s.chars().collect(), pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() { self.pos += 1; }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.advance();
        }
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        self.skip_ws();
        match self.advance() {
            Some(got) if got == c => Ok(()),
            Some(got) => Err(format!("Очікувався '{}', отримано '{}' на позиції {}", c, got, self.pos)),
            None => Err(format!("Очікувався '{}', але рядок закінчився", c)),
        }
    }

    fn parse_value(&mut self) -> Result<Value, String> {
        self.skip_ws();
        match self.peek() {
            Some('"')  => self.parse_string().map(Value::Str),
            Some('{')  => self.parse_object(),
            Some('[')  => self.parse_array(),
            Some('t')  => self.parse_literal("true",  Value::Bool(true)),
            Some('f')  => self.parse_literal("false", Value::Bool(false)),
            Some('n')  => self.parse_literal("null",  Value::Nil),
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(format!("Неочікуваний символ '{}' на позиції {}", c, self.pos)),
            None    => Err("Несподіваний кінець JSON".into()),
        }
    }

    fn parse_literal(&mut self, expected: &str, val: Value) -> Result<Value, String> {
        for c in expected.chars() {
            match self.advance() {
                Some(got) if got == c => {}
                Some(got) => return Err(format!(
                    "Очікувався символ '{}' з '{}', отримано '{}'", c, expected, got
                )),
                None => return Err(format!("Несподіваний кінець при читанні '{}'", expected)),
            }
        }
        Ok(val)
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some('-') { self.advance(); }
        while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            self.advance();
        }
        if self.peek() == Some('.') {
            self.advance();
            while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                self.advance();
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.advance();
            if matches!(self.peek(), Some('+') | Some('-')) { self.advance(); }
            while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                self.advance();
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        s.parse::<f64>()
            .map(Value::Num)
            .map_err(|_| format!("Невірне число: {}", s))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut s = String::new();
        loop {
            match self.advance() {
                None      => return Err("Незакритий рядок".into()),
                Some('"') => break,
                Some('\\') => {
                    match self.advance() {
                        Some('"')  => s.push('"'),
                        Some('\\') => s.push('\\'),
                        Some('/')  => s.push('/'),
                        Some('b')  => s.push('\x08'),
                        Some('f')  => s.push('\x0C'),
                        Some('n')  => s.push('\n'),
                        Some('r')  => s.push('\r'),
                        Some('t')  => s.push('\t'),
                        Some('u')  => {
                            // \uXXXX
                            let mut hex = String::new();
                            for _ in 0..4 {
                                match self.advance() {
                                    Some(c) => hex.push(c),
                                    None    => return Err("Незавершений \\uXXXX".into()),
                                }
                            }
                            let code = u32::from_str_radix(&hex, 16)
                                .map_err(|_| format!("Невірний \\u{}", hex))?;
                            let c = char::from_u32(code)
                                .ok_or_else(|| format!("Невірний Unicode: {}", code))?;
                            s.push(c);
                        }
                        Some(c) => return Err(format!("Невірна escape послідовність: \\{}", c)),
                        None    => return Err("Незакритий escape".into()),
                    }
                }
                Some(c) => s.push(c),
            }
        }
        Ok(s)
    }

    fn parse_array(&mut self) -> Result<Value, String> {
        self.expect('[')?;
        self.skip_ws();
        let mut elems = Vec::new();
        if self.peek() == Some(']') {
            self.advance();
            return Ok(Value::List(GcList::new(elems)));
        }
        loop {
            elems.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => { self.advance(); }
                Some(']') => { self.advance(); break; }
                Some(c)   => return Err(format!("Очікувалась ',' або ']', отримано '{}'", c)),
                None      => return Err("Незакритий масив".into()),
            }
        }
        Ok(Value::List(GcList::new(elems)))
    }

    fn parse_object(&mut self) -> Result<Value, String> {
        self.expect('{')?;
        self.skip_ws();
        let mut fields: HashMap<String, Value> = HashMap::new();
        if self.peek() == Some('}') {
            self.advance();
            return Ok(Value::Struct(OberihStruct::new("JsonObject".into(), fields)));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(':')?;
            let val = self.parse_value()?;
            fields.insert(key, val);
            self.skip_ws();
            match self.peek() {
                Some(',') => { self.advance(); }
                Some('}') => { self.advance(); break; }
                Some(c)   => return Err(format!("Очікувалась ',' або '}}', отримано '{}'", c)),
                None      => return Err("Незакритий об'єкт".into()),
            }
        }
        Ok(Value::Struct(OberihStruct::new("JsonObject".into(), fields)))
    }
}

// ---------------------------------------------------------------------------
// Серіалізатор
// ---------------------------------------------------------------------------

fn stringify_value(val: &Value, indent: usize, pretty: bool) -> String {
    match val {
        Value::Nil        => "null".into(),
        Value::Bool(b)    => b.to_string(),
        Value::Num(n)     => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", *n as i64)
            } else {
                format!("{}", n)
            }
        }
        Value::Str(s)     => stringify_string(s),
        Value::List(l)    => {
            let items = l.to_vec();
            if items.is_empty() { return "[]".into(); }
            if pretty {
                let inner_indent = indent + 2;
                let pad     = " ".repeat(inner_indent);
                let end_pad = " ".repeat(indent);
                let parts: Vec<String> = items.iter()
                    .map(|v| format!("{}{}", pad, stringify_value(v, inner_indent, pretty)))
                    .collect();
                format!("[\n{}\n{}]", parts.join(",\n"), end_pad)
            } else {
                let parts: Vec<String> = items.iter()
                    .map(|v| stringify_value(v, indent, pretty))
                    .collect();
                format!("[{}]", parts.join(","))
            }
        }
        Value::Struct(s)  => {
            let fields = s.fields.lock().unwrap();
            if fields.is_empty() { return "{}".into(); }
            if pretty {
                let inner_indent = indent + 2;
                let pad     = " ".repeat(inner_indent);
                let end_pad = " ".repeat(indent);
                let mut parts: Vec<String> = fields.iter()
                    .map(|(k, v)| format!("{}{}: {}", pad,
                        stringify_string(k),
                        stringify_value(v, inner_indent, pretty)))
                    .collect();
                parts.sort(); // детермінований вивід
                format!("{{\n{}\n{}}}", parts.join(",\n"), end_pad)
            } else {
                let mut parts: Vec<String> = fields.iter()
                    .map(|(k, v)| format!("{}:{}", stringify_string(k), stringify_value(v, indent, pretty)))
                    .collect();
                parts.sort();
                format!("{{{}}}", parts.join(","))
            }
        }
        Value::Ok(v)      => format!("{{\"ok\":{}}}", stringify_value(v, indent, pretty)),
        Value::Err(v)     => format!("{{\"err\":{}}}", stringify_value(v, indent, pretty)),
        Value::Fn(n)      => format!("\"<fn {}>\"", n),
        _                 => "null".into(),
    }
}

fn stringify_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Тести
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_primitives() {
        assert!(matches!(json_parse("null"), Ok(Value::Nil)));
        assert!(matches!(json_parse("true"), Ok(Value::Bool(true))));
        assert!(matches!(json_parse("false"), Ok(Value::Bool(false))));
        assert!(matches!(json_parse("42"), Ok(Value::Num(n)) if n == 42.0));
        assert!(matches!(json_parse("3.14"), Ok(Value::Num(n)) if (n - 3.14).abs() < 1e-10));
        assert!(matches!(json_parse("\"hello\""), Ok(Value::Str(s)) if s == "hello"));
    }

    #[test]
    fn test_parse_array() {
        let v = json_parse("[1, 2, 3]").unwrap();
        if let Value::List(l) = v {
            assert_eq!(l.len(), 3);
        } else { panic!("Очікувався List"); }
    }

    #[test]
    fn test_parse_object() {
        let v = json_parse(r#"{"name": "Oberih", "version": 1}"#).unwrap();
        if let Value::Struct(s) = v {
            assert!(s.get("name").is_some());
            assert!(s.get("version").is_some());
        } else { panic!("Очікувався Struct"); }
    }

    #[test]
    fn test_parse_nested() {
        let v = json_parse(r#"{"user": {"id": 1, "tags": ["a", "b"]}}"#).unwrap();
        assert!(matches!(v, Value::Struct(_)));
    }

    #[test]
    fn test_stringify_roundtrip() {
        let original = r#"{"completed":false,"id":1,"title":"test","userId":1}"#;
        let v = json_parse(original).unwrap();
        let s = json_stringify(&v);
        assert_eq!(s, original);
    }

    #[test]
    fn test_unicode_escape() {
        let v = json_parse(r#""\u041F\u0440\u0438\u0432\u0456\u0442""#).unwrap();
        assert!(matches!(v, Value::Str(s) if s == "Привіт"));
    }

    #[test]
    fn test_empty_collections() {
        assert!(matches!(json_parse("[]"),  Ok(Value::List(_))));
        assert!(matches!(json_parse("{}"),  Ok(Value::Struct(_))));
    }
}
