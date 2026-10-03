/// Місток між рушієм `regex.rs` і значеннями VM: компіляція, методи об'єкта
/// `Regex`, і функції-скорочення (`reTest` тощо) для одноразового використання.
use std::sync::Arc;

use crate::gc::{GcList, GcMap, MapKey, OMap};
use crate::regex::Regex;
use crate::vm::{RuntimeError, Value};

type VR<T> = Result<T, RuntimeError>;

fn rt_err(msg: impl Into<String>) -> RuntimeError { RuntimeError::General(msg.into()) }
fn ok(v: Value) -> Value { Value::Ok(Box::new(v)) }
fn err(msg: impl Into<String>) -> Value { Value::Err(Box::new(Value::Str(msg.into()))) }

pub fn is_ext(name: &str) -> bool {
    matches!(name, "regex" | "reTest" | "reFind" | "reFindAll" | "reReplace" | "reReplaceAll" | "reSplit")
}

fn need_str(args: &[Value], i: usize, f: &str) -> VR<String> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(o) => Err(rt_err(format!("{}: аргумент {} має бути String, отримано {}", f, i, o))),
        None    => Err(rt_err(format!("{}: потрібен аргумент {}", f, i))),
    }
}

fn compile(pattern: &str) -> Value {
    match Regex::new(pattern) {
        Ok(re) => ok(Value::Regex(Arc::new(re), pattern.to_string())),
        Err(e) => err(format!("недопустимий шаблон '{}': {}", pattern, e)),
    }
}

/// Об'єкт "збіг" як Map: {text, start, end, groups}.
/// `groups[0]` — увесь збіг (те саме, що `text`); `groups[i]` для i>=1 — захоплюючі
/// групи, `nil` якщо група не брала участі в цьому збігу.
fn captures_to_value(text: &[char], caps: &[Option<(usize, usize)>]) -> Value {
    let slice = |r: (usize, usize)| -> String { text[r.0..r.1].iter().collect() };
    let (start, end) = caps[0].unwrap();
    let groups: Vec<Value> = caps.iter()
        .map(|g| g.map(|r| Value::Str(slice(r))).unwrap_or(Value::Nil))
        .collect();
    let mut m = OMap::new();
    m.insert(MapKey::Str("text".into()), Value::Str(slice((start, end))));
    m.insert(MapKey::Str("start".into()), Value::Num(start as f64));
    m.insert(MapKey::Str("end".into()),   Value::Num(end as f64));
    m.insert(MapKey::Str("groups".into()), Value::List(GcList::new(groups)));
    Value::Map(GcMap::new(m))
}

/// Заміна `$1`, `$2`, ... і `$$` (літеральний `$`) у шаблоні заміни.
/// Невідомий (поза межами кількості груп або не задіяний у цьому збігу) `$N`
/// підставляється як порожній рядок — так само роблять інші мови.
fn expand_replacement(repl: &str, text: &[char], caps: &[Option<(usize, usize)>]) -> String {
    let rc: Vec<char> = repl.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < rc.len() {
        if rc[i] == '$' && i + 1 < rc.len() {
            if rc[i + 1] == '$' { out.push('$'); i += 2; continue; }
            if rc[i + 1].is_ascii_digit() {
                let mut j = i + 1;
                while j < rc.len() && rc[j].is_ascii_digit() { j += 1; }
                let n: usize = rc[i + 1..j].iter().collect::<String>().parse().unwrap_or(0);
                if let Some(Some((s, e))) = caps.get(n) {
                    out.extend(&text[*s..*e]);
                }
                i = j;
                continue;
            }
        }
        out.push(rc[i]);
        i += 1;
    }
    out
}

fn replace_impl(re: &Regex, text: &str, repl: &str, all: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let matches: Vec<_> = if all { re.find_all(&chars) } else {
        re.find_at(&chars, 0).into_iter().collect()
    };
    if matches.is_empty() { return text.to_string(); }
    let mut out = String::new();
    let mut last = 0usize;
    for caps in &matches {
        let (s, e) = caps[0].unwrap();
        out.extend(&chars[last..s]);
        out.push_str(&expand_replacement(repl, &chars, caps));
        last = e;
    }
    out.extend(&chars[last..]);
    out
}

pub fn call_ext(name: &str, args: Vec<Value>) -> VR<Value> {
    match name {
        "regex" => Ok(compile(&need_str(&args, 0, "regex")?)),
        "reTest" => {
            let pat = need_str(&args, 0, "reTest")?;
            let s   = need_str(&args, 1, "reTest")?;
            match Regex::new(&pat) {
                Ok(re) => Ok(Value::Bool(re.is_match(&s.chars().collect::<Vec<_>>()))),
                Err(e) => Err(rt_err(format!("reTest: недопустимий шаблон '{}': {}", pat, e))),
            }
        }
        "reFind" => {
            let pat = need_str(&args, 0, "reFind")?;
            let s   = need_str(&args, 1, "reFind")?;
            let re = Regex::new(&pat).map_err(|e| rt_err(format!("reFind: недопустимий шаблон '{}': {}", pat, e)))?;
            let chars: Vec<char> = s.chars().collect();
            Ok(match re.find_at(&chars, 0) { Some(c) => captures_to_value(&chars, &c), None => Value::Nil })
        }
        "reFindAll" => {
            let pat = need_str(&args, 0, "reFindAll")?;
            let s   = need_str(&args, 1, "reFindAll")?;
            let re = Regex::new(&pat).map_err(|e| rt_err(format!("reFindAll: недопустимий шаблон '{}': {}", pat, e)))?;
            let chars: Vec<char> = s.chars().collect();
            let items = re.find_all(&chars).into_iter().map(|c| captures_to_value(&chars, &c)).collect();
            Ok(Value::List(GcList::new(items)))
        }
        "reReplace" | "reReplaceAll" => {
            let pat  = need_str(&args, 0, name)?;
            let s    = need_str(&args, 1, name)?;
            let repl = need_str(&args, 2, name)?;
            let re = Regex::new(&pat).map_err(|e| rt_err(format!("{}: недопустимий шаблон '{}': {}", name, pat, e)))?;
            Ok(Value::Str(replace_impl(&re, &s, &repl, name == "reReplaceAll")))
        }
        "reSplit" => {
            let pat = need_str(&args, 0, "reSplit")?;
            let s   = need_str(&args, 1, "reSplit")?;
            let re = Regex::new(&pat).map_err(|e| rt_err(format!("reSplit: недопустимий шаблон '{}': {}", pat, e)))?;
            Ok(Value::List(GcList::new(split_impl(&re, &s))))
        }
        other => Err(rt_err(format!("Невідома вбудована функція: '{}'", other))),
    }
}

fn split_impl(re: &Regex, text: &str) -> Vec<Value> {
    let chars: Vec<char> = text.chars().collect();
    let matches = re.find_all(&chars);
    let mut out = Vec::new();
    let mut last = 0usize;
    for caps in &matches {
        let (s, e) = caps[0].unwrap();
        if e == s { continue; } // роздільник не може бути порожнім — інакше нескінченно дробимо кожен символ
        out.push(Value::Str(chars[last..s].iter().collect()));
        last = e;
    }
    out.push(Value::Str(chars[last..].iter().collect()));
    out
}

/// Методи значення `Regex`: `r.test(s)`, `r.find(s)`, `r.findAll(s)`,
/// `r.replace(s, repl)`, `r.replaceAll(s, repl)`, `r.split(s)`, `r.source()`.
pub fn regex_method(receiver: Value, method: &str, args: Vec<Value>) -> VR<Value> {
    let (re, pattern) = match receiver { Value::Regex(re, p) => (re, p), _ => unreachable!() };
    match method {
        "test" => {
            let s = need_str(&args, 0, "test")?;
            Ok(Value::Bool(re.is_match(&s.chars().collect::<Vec<_>>())))
        }
        "find" => {
            let s = need_str(&args, 0, "find")?;
            let chars: Vec<char> = s.chars().collect();
            Ok(match re.find_at(&chars, 0) { Some(c) => captures_to_value(&chars, &c), None => Value::Nil })
        }
        "findAll" => {
            let s = need_str(&args, 0, "findAll")?;
            let chars: Vec<char> = s.chars().collect();
            let items = re.find_all(&chars).into_iter().map(|c| captures_to_value(&chars, &c)).collect();
            Ok(Value::List(GcList::new(items)))
        }
        "replace" | "replaceAll" => {
            let s    = need_str(&args, 0, method)?;
            let repl = need_str(&args, 1, method)?;
            Ok(Value::Str(replace_impl(&re, &s, &repl, method == "replaceAll")))
        }
        "split" => {
            let s = need_str(&args, 0, "split")?;
            Ok(Value::List(GcList::new(split_impl(&re, &s))))
        }
        "source" => Ok(Value::Str(pattern)),
        _ => Err(rt_err(format!("Невідомий метод Regex: {}", method))),
    }
}
