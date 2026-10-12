/// Стандартна бібліотека Oberih.
/// Всі вбудовані функції — без зовнішніх залежностей, тільки std.

use std::time::{SystemTime, UNIX_EPOCH};
use std::io::{self, BufRead, Write};
use crate::vm::{Value, RuntimeError};
use crate::gc::{GcList, GcMap, MapKey, OMap};

type VR<T> = Result<T, RuntimeError>;

fn rt_err(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::General(msg.into())
}

/// Повертає true якщо ім'я — вбудована функція.
pub fn is_builtin(name: &str) -> bool {
    is_core_builtin(name) || crate::stdlib_ext::is_ext(name) || crate::regex_ops::is_ext(name)
}

fn is_core_builtin(name: &str) -> bool {
    matches!(name,
        // IO
        "print" | "println" | "readLine" | "readFile" | "writeFile" | "appendFile" |
        // HTTP
        "httpGet" | "httpPost" | "httpPut" | "httpDelete" |
        // JSON
        "jsonParse" | "jsonStringify" | "jsonPretty" |
        // Конвертація
        "toString" | "toNumber" | "toBool" |
        // Рядки
        "strLen" | "strTrim" | "strUpper" | "strLower" |
        "strContains" | "strStartsWith" | "strEndsWith" |
        "strSplit" | "strJoin" | "strReplace" | "strSlice" |
        // Числа
        "floor" | "ceil" | "round" | "abs" | "sqrt" | "pow" | "min" | "max" |
        // Map
        "keys" | "values" | "entries" | "mapHas" | "mapGet" | "mapSet" |
        "mapDelete" | "mapMerge" |
        // Типи та ітерація
        "typeOf" | "__iterable" |
        // Списки
        "len" | "push" | "pop" | "first" | "last" | "reverse" | "contains" |
        "range" |
        // Час
        "now" | "sleep" |
        // Процес
        "exit" | "args" | "env" |
        // Відладка
        "debug" | "assert" | "panic" | "gcstats" |
        // WeakRef
        "weakRef" | "upgrade" | "isAlive"
    )
}

/// Викликає вбудовану функцію.
pub fn call_builtin(name: &str, args: Vec<Value>) -> VR<Value> {
    match name {
        // --- IO ---
        "print" => {
            let parts: Vec<String> = args.iter().map(|v| v.to_string()).collect();
            print!("{}", parts.join(" "));
            io::stdout().flush().ok();
            Ok(Value::Nil)
        }
        "println" => {
            let parts: Vec<String> = args.iter().map(|v| v.to_string()).collect();
            println!("{}", parts.join(" "));
            Ok(Value::Nil)
        }
        "readLine" => {
            let stdin = io::stdin();
            let mut line = String::new();
            stdin.lock().read_line(&mut line)
                .map_err(|e| rt_err(format!("readLine: {}", e)))?;
            Ok(Value::Str(line.trim_end_matches('\n').to_string()))
        }
        "readFile" => {
            let path = require_str(&args, 0, "readFile")?;
            match std::fs::read_to_string(&path) {
                Ok(content) => Ok(Value::Ok(Box::new(Value::Str(content)))),
                Err(e)      => Ok(Value::Err(Box::new(Value::Str(format!("readFile '{}': {}", path, e))))),
            }
        }
        "writeFile" => {
            let path    = require_str(&args, 0, "writeFile")?;
            let content = require_str(&args, 1, "writeFile")?;
            match std::fs::write(&path, &content) {
                Ok(_)  => Ok(Value::Ok(Box::new(Value::Nil))),
                Err(e) => Ok(Value::Err(Box::new(Value::Str(format!("writeFile '{}': {}", path, e))))),
            }
        }
        "appendFile" => {
            let path    = require_str(&args, 0, "appendFile")?;
            let content = require_str(&args, 1, "appendFile")?;
            use std::io::Write;
            match std::fs::OpenOptions::new().append(true).create(true).open(&path) {
                Ok(mut f) => {
                    f.write_all(content.as_bytes())
                        .map_err(|e| rt_err(e.to_string()))?;
                    Ok(Value::Ok(Box::new(Value::Nil)))
                }
                Err(e) => Ok(Value::Err(Box::new(Value::Str(format!("appendFile '{}': {}", path, e))))),
            }
        }

        // --- JSON ---
        "jsonParse" => {
            let s = require_str(&args, 0, "jsonParse")?;
            match crate::json::json_parse(&s) {
                Ok(v)  => Ok(Value::Ok(Box::new(v))),
                Err(e) => Ok(Value::Err(Box::new(Value::Str(e)))),
            }
        }
        "jsonStringify" => {
            let v = args.into_iter().next().unwrap_or(Value::Nil);
            Ok(Value::Str(crate::json::json_stringify(&v)))
        }
        "jsonPretty" => {
            let v = args.into_iter().next().unwrap_or(Value::Nil);
            Ok(Value::Str(crate::json::json_stringify_pretty(&v)))
        }

        // --- HTTP / HTTPS ---
        "httpGet" => {
            let url     = require_str(&args, 0, "httpGet")?;
            let timeout = args.get(1).and_then(|v| if let Value::Num(n) = v { Some(*n as u64) } else { None }).unwrap_or(10);
            match crate::http_client::http_request("GET", &url, None, timeout) {
                Ok(resp) => Ok(Value::Ok(Box::new(crate::http_client::response_to_value(resp)))),
                Err(e)   => Ok(Value::Err(Box::new(Value::Str(e)))),
            }
        }
        "httpPost" => {
            let url     = require_str(&args, 0, "httpPost")?;
            let body    = args.get(1).map(|v| v.to_string()).unwrap_or_default();
            let timeout = args.get(2).and_then(|v| if let Value::Num(n) = v { Some(*n as u64) } else { None }).unwrap_or(10);
            match crate::http_client::http_request("POST", &url, Some(&body), timeout) {
                Ok(resp) => Ok(Value::Ok(Box::new(crate::http_client::response_to_value(resp)))),
                Err(e)   => Ok(Value::Err(Box::new(Value::Str(e)))),
            }
        }
        "httpPut" => {
            let url     = require_str(&args, 0, "httpPut")?;
            let body    = args.get(1).map(|v| v.to_string()).unwrap_or_default();
            let timeout = args.get(2).and_then(|v| if let Value::Num(n) = v { Some(*n as u64) } else { None }).unwrap_or(10);
            match crate::http_client::http_request("PUT", &url, Some(&body), timeout) {
                Ok(resp) => Ok(Value::Ok(Box::new(crate::http_client::response_to_value(resp)))),
                Err(e)   => Ok(Value::Err(Box::new(Value::Str(e)))),
            }
        }
        "httpDelete" => {
            let url     = require_str(&args, 0, "httpDelete")?;
            let timeout = args.get(1).and_then(|v| if let Value::Num(n) = v { Some(*n as u64) } else { None }).unwrap_or(10);
            match crate::http_client::http_request("DELETE", &url, None, timeout) {
                Ok(resp) => Ok(Value::Ok(Box::new(crate::http_client::response_to_value(resp)))),
                Err(e)   => Ok(Value::Err(Box::new(Value::Str(e)))),
            }
        }
        "toString" => {
            Ok(Value::Str(args.into_iter().next().unwrap_or(Value::Nil).to_string()))
        }
        "toNumber" => {
            match args.into_iter().next() {
                Some(Value::Num(n))  => Ok(Value::Num(n)),
                Some(Value::Str(s))  => {
                    s.trim().parse::<f64>()
                        .map(Value::Num)
                        .map_err(|_| rt_err(format!("toNumber: '{}' не число", s)))
                }
                Some(Value::Bool(b)) => Ok(Value::Num(if b { 1.0 } else { 0.0 })),
                _ => Err(rt_err("toNumber: потрібен String, Number або Bool")),
            }
        }
        "toBool" => {
            match args.into_iter().next() {
                Some(Value::Bool(b)) => Ok(Value::Bool(b)),
                Some(Value::Num(n))  => Ok(Value::Bool(n != 0.0)),
                Some(Value::Str(s))  => Ok(Value::Bool(!s.is_empty())),
                Some(Value::Nil)     => Ok(Value::Bool(false)),
                _                    => Ok(Value::Bool(true)),
            }
        }

        // --- Рядки ---
        "strLen"        => Ok(Value::Num(require_str(&args, 0, "strLen")?.chars().count() as f64)),
        "strTrim"       => Ok(Value::Str(require_str(&args, 0, "strTrim")?.trim().to_string())),
        "strUpper"      => Ok(Value::Str(require_str(&args, 0, "strUpper")?.to_uppercase())),
        "strLower"      => Ok(Value::Str(require_str(&args, 0, "strLower")?.to_lowercase())),
        "strContains"   => {
            let s = require_str(&args, 0, "strContains")?;
            let p = require_str(&args, 1, "strContains")?;
            Ok(Value::Bool(s.contains(p.as_str())))
        }
        "strStartsWith" => {
            let s = require_str(&args, 0, "strStartsWith")?;
            let p = require_str(&args, 1, "strStartsWith")?;
            Ok(Value::Bool(s.starts_with(p.as_str())))
        }
        "strEndsWith" => {
            let s = require_str(&args, 0, "strEndsWith")?;
            let p = require_str(&args, 1, "strEndsWith")?;
            Ok(Value::Bool(s.ends_with(p.as_str())))
        }
        "strSplit" => {
            let s   = require_str(&args, 0, "strSplit")?;
            let sep = require_str(&args, 1, "strSplit")?;
            let parts: Vec<Value> = s.split(sep.as_str())
                .map(|p| Value::Str(p.to_string()))
                .collect();
            Ok(Value::List(GcList::new(parts)))
        }
        "strJoin" => {
            // Приймаємо обидва порядки: strJoin(sep, list) і strJoin(list, sep).
            let mut args = args;
            if matches!(args.get(0), Some(Value::List(_))) && matches!(args.get(1), Some(Value::Str(_))) {
                args.swap(0, 1);
            }
            let sep = require_str(&args, 0, "strJoin")?;
            let list = require_list(&args, 1, "strJoin")?;
            let parts: Vec<String> = list.iter().map(|v| v.to_string()).collect();
            Ok(Value::Str(parts.join(sep.as_str())))
        }
        "strReplace" => {
            let s    = require_str(&args, 0, "strReplace")?;
            let from = require_str(&args, 1, "strReplace")?;
            let to   = require_str(&args, 2, "strReplace")?;
            Ok(Value::Str(s.replace(from.as_str(), to.as_str())))
        }
        "strSlice" => {
            let s     = require_str(&args, 0, "strSlice")?;
            let start = require_num(&args, 1, "strSlice")? as usize;
            let end   = require_num(&args, 2, "strSlice")? as usize;
            let chars: Vec<char> = s.chars().collect();
            let slice: String = chars.get(start..end.min(chars.len()))
                .unwrap_or(&[])
                .iter()
                .collect();
            Ok(Value::Str(slice))
        }

        // --- Числа ---
        "floor"  => Ok(Value::Num(require_num(&args, 0, "floor")?.floor())),
        "ceil"   => Ok(Value::Num(require_num(&args, 0, "ceil")?.ceil())),
        "round"  => Ok(Value::Num(require_num(&args, 0, "round")?.round())),
        "abs"    => Ok(Value::Num(require_num(&args, 0, "abs")?.abs())),
        "sqrt"   => Ok(Value::Num(require_num(&args, 0, "sqrt")?.sqrt())),
        "pow"    => {
            let base = require_num(&args, 0, "pow")?;
            let exp  = require_num(&args, 1, "pow")?;
            Ok(Value::Num(base.powf(exp)))
        }
        "min"    => {
            let a = require_num(&args, 0, "min")?;
            let b = require_num(&args, 1, "min")?;
            Ok(Value::Num(a.min(b)))
        }
        "max"    => {
            let a = require_num(&args, 0, "max")?;
            let b = require_num(&args, 1, "max")?;
            Ok(Value::Num(a.max(b)))
        }

        // --- Списки ---
        "keys" | "values" | "entries" | "mapHas" | "mapGet" | "mapSet" |
        "mapDelete" | "mapMerge" => {
            let method = match name {
                "mapHas" => "has", "mapGet" => "get", "mapSet" => "set",
                "mapDelete" => "delete", "mapMerge" => "merge",
                other => other,               // keys / values / entries
            };
            let mut it = args.into_iter();
            match it.next() {
                Some(Value::Map(m)) => map_op(&m, method, it.collect()),
                Some(other) => Err(rt_err(format!(
                    "{}: перший аргумент має бути Map, отримано {}", name, other
                ))),
                None => Err(rt_err(format!("{}: потрібен Map", name))),
            }
        }
        "typeOf" => {
            let v = args.into_iter().next().unwrap_or(Value::Nil);
            Ok(Value::Str(type_of(&v)))
        }
        // Службова: `for (x in <expr>)` компілюється через неї.
        // List -> сам список; Map -> знімок ключів; String -> список символів.
        "__iterable" => {
            match args.into_iter().next() {
                Some(Value::List(l)) => Ok(Value::List(l)),
                Some(Value::Map(m))  => Ok(Value::List(GcList::new(m.keys()))),
                Some(Value::Str(s))  => Ok(Value::List(GcList::new(
                    s.chars().map(|c| Value::Str(c.to_string())).collect()
                ))),
                Some(other) => Err(rt_err(format!(
                    "for: можна перебирати List, Map (ключі) або String, отримано {}", other
                ))),
                None => Err(rt_err("for: немає що перебирати")),
            }
        }
        "len" => {
            match args.into_iter().next() {
                Some(Value::List(l)) => Ok(Value::Num(l.len() as f64)),
                Some(Value::Str(s))  => Ok(Value::Num(s.chars().count() as f64)),
                Some(Value::Map(m))  => Ok(Value::Num(m.len() as f64)),
                _ => Err(rt_err("len: потрібен List, Map або String")),
            }
        }
        "push" => {
            let mut list = require_list(&args, 0, "push")?;
            let item = args.into_iter().nth(1).unwrap_or(Value::Nil);
            list.push(item);
            Ok(Value::List(GcList::new(list)))
        }
        "pop" => {
            if let Some(Value::List(l)) = args.into_iter().next() {
                Ok(l.pop().unwrap_or(Value::Nil))
            } else { Err(rt_err("pop: потрібен List")) }
        }
        "first" => {
            if let Some(Value::List(l)) = args.into_iter().next() {
                Ok(l.first().unwrap_or(Value::Nil))
            } else { Err(rt_err("first: потрібен List")) }
        }
        "last" => {
            if let Some(Value::List(l)) = args.into_iter().next() {
                Ok(l.last().unwrap_or(Value::Nil))
            } else { Err(rt_err("last: потрібен List")) }
        }
        "reverse" => {
            let mut list = require_list(&args, 0, "reverse")?;
            list.reverse();
            Ok(Value::List(GcList::new(list)))
        }
        "contains" => {
            let list_val = args.get(0).cloned().unwrap_or(Value::Nil);
            let item = args.into_iter().nth(1).unwrap_or(Value::Nil);
            if let Value::List(l) = list_val {
                Ok(Value::Bool(l.contains(&item)))
            } else { Err(rt_err("contains: потрібен List")) }
        }
        "range" => {
            let from = require_num(&args, 0, "range")? as i64;
            let to   = require_num(&args, 1, "range")? as i64;
            let list: Vec<Value> = (from..to).map(|n| Value::Num(n as f64)).collect();
            Ok(Value::List(GcList::new(list)))
        }

        // --- Час ---
        "now" => {
            let ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            Ok(Value::Num(ms as f64))
        }
        "sleep" => {
            let ms = require_num(&args, 0, "sleep")?;
            std::thread::sleep(std::time::Duration::from_millis(ms as u64));
            Ok(Value::Nil)
        }

        // --- Процес ---
        "exit" => {
            let code = args.into_iter().next()
                .and_then(|v| if let Value::Num(n) = v { Some(n as i32) } else { None })
                .unwrap_or(0);
            std::process::exit(code);
        }
        "args" => {
            let list: Vec<Value> = std::env::args()
                .skip(3) // пропускаємо "oberih", "run", "file.obh"
                .map(Value::Str)
                .collect();
            Ok(Value::List(GcList::new(list)))
        }
        "env" => {
            let key = require_str(&args, 0, "env")?;
            match std::env::var(&key) {
                Ok(val) => Ok(Value::Ok(Box::new(Value::Str(val)))),
                Err(_)  => Ok(Value::Err(Box::new(Value::Str(format!("env var '{}' не знайдено", key))))),
            }
        }

        // --- Відладка ---
        "debug" => {
            // Значення друкуємо як у println (раніше витікав Rust-формат `Num(5.0)`),
            // разом із типом — щоб відрізняти 5 від "5".
            let parts: Vec<String> = args.iter()
                .map(|v| format!("{} ({})", v, type_of(v)))
                .collect();
            eprintln!("[debug] {}", parts.join(" "));
            Ok(Value::Nil)
        }
        "assert" => {
            let cond = args.first().map(|v| v.is_truthy()).unwrap_or(false);
            if !cond {
                let msg = args.get(1)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "assertion failed".to_string());
                Err(rt_err(format!("assert: {}", msg)))
            } else {
                Ok(Value::Nil)
            }
        }
        "panic" => {
            let msg = args.into_iter().next()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "panic".to_string());
            Err(rt_err(format!("panic: {}", msg)))
        }

        "weakRef" => {
            // weakRef(list) -> WeakRef
            // Створює слабке посилання — не утримує список живим
            match args.into_iter().next() {
                Some(Value::List(l)) => Ok(Value::WeakRef(l.downgrade())),
                _ => Err(rt_err("weakRef: потрібен List")),
            }
        }
        "upgrade" => {
            // upgrade(weakRef) -> Ok(list) або Err("dropped")
            // Намагається отримати сильне посилання
            match args.into_iter().next() {
                Some(Value::WeakRef(w)) => {
                    match w.upgrade() {
                        Some(list) => Ok(Value::Ok(Box::new(Value::List(list)))),
                        None       => Ok(Value::Err(Box::new(Value::Str("об'єкт вже звільнено GC".into())))),
                    }
                }
                _ => Err(rt_err("upgrade: потрібен WeakRef")),
            }
        }
        "isAlive" => {
            // isAlive(weakRef) -> Bool
            match args.into_iter().next() {
                Some(Value::WeakRef(w)) => Ok(Value::Bool(w.is_alive())),
                Some(Value::List(_))    => Ok(Value::Bool(true)),  // сильне посилання — завжди живе
                _                       => Ok(Value::Bool(false)),
            }
        }

        "gcstats" => {
            let stats = crate::gc::gc_stats();
            println!("GC: allocs={} drops={} live={}",
                stats.total_allocs, stats.total_drops, stats.live_objects);
            Ok(Value::Nil)
        }

        _ => {
            if crate::regex_ops::is_ext(name) { crate::regex_ops::call_ext(name, args) }
            else { crate::stdlib_ext::call_ext(name, args) }
        }
    }
}

// ---------------------------------------------------------------------------
// Хелпери
// ---------------------------------------------------------------------------

fn require_str(args: &[Value], idx: usize, fn_name: &str) -> VR<String> {
    match args.get(idx) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(other) => Err(rt_err(format!("{}: аргумент {} має бути String, отримано {}", fn_name, idx, other))),
        None        => Err(rt_err(format!("{}: потрібен аргумент {}", fn_name, idx))),
    }
}

fn require_num(args: &[Value], idx: usize, fn_name: &str) -> VR<f64> {
    match args.get(idx) {
        Some(Value::Num(n)) => Ok(*n),
        Some(other) => Err(rt_err(format!("{}: аргумент {} має бути Number, отримано {}", fn_name, idx, other))),
        None        => Err(rt_err(format!("{}: потрібен аргумент {}", fn_name, idx))),
    }
}

fn require_list(args: &[Value], idx: usize, fn_name: &str) -> VR<Vec<Value>> {
    match args.get(idx) {
        Some(Value::List(l)) => Ok(l.to_vec()),
        Some(other) => Err(rt_err(format!("{}: аргумент {} має бути List, отримано {}", fn_name, idx, other))),
        None        => Err(rt_err(format!("{}: потрібен аргумент {}", fn_name, idx))),
    }
}

/// Назва типу значення для `typeOf`.
pub fn type_of(v: &Value) -> String {
    match v {
        Value::Num(_)  => "Number".into(),
        Value::Str(_)  => "String".into(),
        Value::Bool(_) => "Bool".into(),
        Value::Nil     => "Nil".into(),
        Value::Ok(_) | Value::Err(_) => "Result".into(),
        Value::Struct(s) => s.type_name.clone(),
        Value::List(_) => "List".into(),
        Value::Map(_)  => "Map".into(),
        Value::WeakRef(_) => "WeakRef".into(),
        Value::Spawn(_)   => "SpawnHandle".into(),
        Value::Fn(_) | Value::Closure(..) => "Fn".into(),
        Value::Regex(..) => "Regex".into(),
        Value::EnumVal(ty, _) => ty.clone(),
    }
}

/// Операції над Map. Спільна для методів (`m.keys()`) і функцій (`keys(m)`).
pub fn map_op(m: &GcMap, method: &str, args: Vec<Value>) -> VR<Value> {
    let key_of = |v: &Value| MapKey::from_value(v).map_err(|e| rt_err(e));
    let need = |n: usize| -> VR<()> {
        if args.len() < n {
            Err(rt_err(format!("Map.{}: потрібно аргументів: {}", method, n)))
        } else { Ok(()) }
    };
    match method {
        "len"     => Ok(Value::Num(m.len() as f64)),
        "keys"    => Ok(Value::List(GcList::new(m.keys()))),
        "values"  => Ok(Value::List(GcList::new(m.values()))),
        "entries" => Ok(Value::List(GcList::new(
            m.snapshot().into_iter()
                .map(|(k, v)| Value::List(GcList::new(vec![k.to_value(), v])))
                .collect()
        ))),
        "has" => {
            need(1)?;
            Ok(Value::Bool(m.contains(&key_of(&args[0])?)))
        }
        // get(k) -> значення або nil;  get(k, default) -> default якщо ключа немає
        "get" => {
            need(1)?;
            match m.get(&key_of(&args[0])?) {
                Some(v) => Ok(v),
                None    => Ok(args.get(1).cloned().unwrap_or(Value::Nil)),
            }
        }
        // set(k, v) змінює Map на місці і повертає його (можна ланцюжком)
        "set" => {
            need(2)?;
            m.insert(key_of(&args[0])?, args[1].clone());
            Ok(Value::Map(m.clone()))
        }
        // delete(k) -> видалене значення або nil
        "delete" => {
            need(1)?;
            Ok(m.remove(&key_of(&args[0])?).unwrap_or(Value::Nil))
        }
        // merge(other) -> НОВИЙ Map: вміст цього + other (other перекриває)
        "merge" => {
            need(1)?;
            match &args[0] {
                Value::Map(other) => {
                    let mut out = OMap::new();
                    for (k, v) in m.snapshot()     { out.insert(k, v); }
                    for (k, v) in other.snapshot() { out.insert(k, v); }
                    Ok(Value::Map(GcMap::new(out)))
                }
                o => Err(rt_err(format!("Map.merge: потрібен Map, отримано {}", o))),
            }
        }
        _ => Err(rt_err(format!("Невідомий метод Map: {}", method))),
    }
}
