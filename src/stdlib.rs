/// Стандартна бібліотека Oberih.
/// Всі вбудовані функції — без зовнішніх залежностей, тільки std.

use std::time::{SystemTime, UNIX_EPOCH};
use std::io::{self, BufRead, Write};
use crate::vm::{Value, RuntimeError};
use crate::gc::GcList;

type VR<T> = Result<T, RuntimeError>;

fn rt_err(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::General(msg.into())
}

/// Повертає true якщо ім'я — вбудована функція.
pub fn is_builtin(name: &str) -> bool {
    matches!(name,
        // IO
        "print" | "println" | "readLine" | "readFile" | "writeFile" | "appendFile" |
        // Конвертація
        "toString" | "toNumber" | "toBool" |
        // Рядки
        "strLen" | "strTrim" | "strUpper" | "strLower" |
        "strContains" | "strStartsWith" | "strEndsWith" |
        "strSplit" | "strJoin" | "strReplace" | "strSlice" |
        // Числа
        "floor" | "ceil" | "round" | "abs" | "sqrt" | "pow" | "min" | "max" |
        // Списки
        "len" | "push" | "pop" | "first" | "last" | "reverse" | "contains" |
        "map" | "filter" | "range" |
        // Час
        "now" | "sleep" |
        // Процес
        "exit" | "args" | "env" |
        // Відладка
        "debug" | "assert" | "panic" | "gcstats"
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
                Err(e)      => Ok(Value::Err(Box::new(Value::Str(e.to_string())))),
            }
        }
        "writeFile" => {
            let path    = require_str(&args, 0, "writeFile")?;
            let content = require_str(&args, 1, "writeFile")?;
            match std::fs::write(&path, &content) {
                Ok(_)  => Ok(Value::Ok(Box::new(Value::Nil))),
                Err(e) => Ok(Value::Err(Box::new(Value::Str(e.to_string())))),
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
                Err(e) => Ok(Value::Err(Box::new(Value::Str(e.to_string())))),
            }
        }

        // --- Конвертація ---
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
        "len" => {
            match args.into_iter().next() {
                Some(Value::List(l)) => Ok(Value::Num(l.len() as f64)),
                Some(Value::Str(s))  => Ok(Value::Num(s.chars().count() as f64)),
                _ => Err(rt_err("len: потрібен List або String")),
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
            let parts: Vec<String> = args.iter().map(|v| format!("{:?}", v)).collect();
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

        "gcstats" => {
            let stats = crate::gc::gc_stats();
            println!("GC: allocs={} drops={} live={}",
                stats.total_allocs, stats.total_drops, stats.live_objects);
            Ok(Value::Nil)
        }

        _ => Err(rt_err(format!("Невідома вбудована функція: '{}'", name))),
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
