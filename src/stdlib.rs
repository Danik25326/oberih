/// Стандартна бібліотека Oberih.
/// Всі вбудовані функції — без зовнішніх залежностей, тільки std.

use std::time::{SystemTime, UNIX_EPOCH};
use std::io::{self, BufRead, Write, Read};
use std::net::TcpStream;
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
        // HTTP
        "httpGet" | "httpPost" | "httpPut" | "httpDelete" |
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

        // --- HTTP ---
        "httpGet" => {
            let url = require_str(&args, 0, "httpGet")?;
            match http_request("GET", &url, None, args.get(1)) {
                Ok(resp)  => Ok(Value::Ok(Box::new(resp))),
                Err(e)    => Ok(Value::Err(Box::new(Value::Str(e)))),
            }
        }
        "httpPost" => {
            let url  = require_str(&args, 0, "httpPost")?;
            let body = args.get(1).map(|v| v.to_string()).unwrap_or_default();
            match http_request("POST", &url, Some(&body), args.get(2)) {
                Ok(resp) => Ok(Value::Ok(Box::new(resp))),
                Err(e)   => Ok(Value::Err(Box::new(Value::Str(e)))),
            }
        }
        "httpPut" => {
            let url  = require_str(&args, 0, "httpPut")?;
            let body = args.get(1).map(|v| v.to_string()).unwrap_or_default();
            match http_request("PUT", &url, Some(&body), args.get(2)) {
                Ok(resp) => Ok(Value::Ok(Box::new(resp))),
                Err(e)   => Ok(Value::Err(Box::new(Value::Str(e)))),
            }
        }
        "httpDelete" => {
            let url = require_str(&args, 0, "httpDelete")?;
            match http_request("DELETE", &url, None, args.get(1)) {
                Ok(resp) => Ok(Value::Ok(Box::new(resp))),
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

        _ => Err(rt_err(format!("Невідома вбудована функція: '{}'", name))),
    }
}

// ---------------------------------------------------------------------------
// HTTP клієнт — чистий TCP, нуль залежностей
// ---------------------------------------------------------------------------

fn parse_url(url: &str) -> Result<(String, u16, String), String> {
    let url = url.trim_start_matches("http://");
    let (host_port, path) = if let Some(idx) = url.find('/') {
        (&url[..idx], url[idx..].to_string())
    } else {
        (url, "/".to_string())
    };
    let (host, port) = if let Some(idx) = host_port.rfind(':') {
        let port = host_port[idx+1..].parse::<u16>()
            .map_err(|_| format!("Невірний порт: {}", &host_port[idx+1..]))?;
        (host_port[..idx].to_string(), port)
    } else {
        (host_port.to_string(), 80)
    };
    Ok((host, port, path))
}

fn http_request(
    method:  &str,
    url:     &str,
    body:    Option<&str>,
    _headers: Option<&Value>,
) -> Result<Value, String> {
    if !url.starts_with("http://") {
        return Err(format!("Тільки http:// підтримується: {}", url));
    }

    let (host, port, path) = parse_url(url)?;

    let mut stream = TcpStream::connect(format!("{}:{}", host, port))
        .map_err(|e| format!("З'єднання невдале: {}", e))?;

    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;

    let body_str = body.unwrap_or("");
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\nUser-Agent: Oberih/0.3\r\n\r\n{}",
        method, path, host, body_str.len(), body_str
    );

    stream.write_all(request.as_bytes())
        .map_err(|e| format!("Помилка запиту: {}", e))?;

    let mut response = String::new();
    stream.read_to_string(&mut response)
        .map_err(|e| format!("Помилка відповіді: {}", e))?;

    // Розбираємо HTTP відповідь
    let (head, body_resp) = if let Some(idx) = response.find("\r\n\r\n") {
        (&response[..idx], &response[idx+4..])
    } else {
        (response.as_str(), "")
    };

    // Статус код
    let status: u16 = head.lines().next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // Повертаємо struct з status і body
    let mut fields = std::collections::HashMap::new();
    fields.insert("status".to_string(), Value::Num(status as f64));
    fields.insert("body".to_string(),   Value::Str(body_resp.to_string()));
    fields.insert("ok".to_string(),     Value::Bool(status >= 200 && status < 300));

    Ok(Value::Struct(crate::vm::OberihStruct::new("HttpResponse".to_string(), fields)))
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
