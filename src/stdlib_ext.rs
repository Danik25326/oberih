/// Розширення стандартної бібліотеки Oberih: списки, рядки, математика,
/// час/дати (UTC), файли, псевдовипадкові числа. Тільки std, без залежностей.
///
/// Функції вищого порядку (`map`, `filter`, `reduce`, `each`, `any`, `all`,
/// `find`, `sortBy`) тут лише оголошені — виконуються у VM, бо їм потрібно
/// викликати функції користувача (див. `VM::call_higher_order`).

use std::cmp::Ordering;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::gc::{GcList, GcMap, MapKey, OMap};
use crate::vm::{RuntimeError, Value};

type VR<T> = Result<T, RuntimeError>;

fn rt_err(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::General(msg.into())
}

fn ok(v: Value) -> Value { Value::Ok(Box::new(v)) }
fn err(msg: impl Into<String>) -> Value { Value::Err(Box::new(Value::Str(msg.into()))) }

/// Функції, що викликають функції користувача, — виконуються у VM.
pub fn is_higher_order(name: &str) -> bool {
    matches!(name, "map" | "filter" | "reduce" | "each" | "any" | "all" | "find" | "sortBy")
}

/// Чи належить ім'я цьому модулю (включно з функціями вищого порядку).
pub fn is_ext(name: &str) -> bool {
    is_higher_order(name) || matches!(name,
        "gcCollectCycles" | "gcStats" |
        // Списки
        "append" | "slice" | "concat" | "indexOf" | "sort" | "sum" |
        // Рядки
        "strIndexOf" | "strRepeat" | "strPadLeft" | "strPadRight" |
        // Числа
        "sin" | "cos" | "tan" | "log" | "exp" | "pi" | "sign" |
        "random" | "randInt" | "seedRandom" | "parseNumber" |
        // Час (UTC)
        "isoTime" | "dateParts" | "timeFromParts" | "parseIso" |
        // Файли
        "fileExists" | "readLines" | "listDir" | "deleteFile" | "mkdir"
    )
}

// ---------------------------------------------------------------------------
// Допоміжні
// ---------------------------------------------------------------------------

fn need_str(args: &[Value], i: usize, f: &str) -> VR<String> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(o) => Err(rt_err(format!("{}: аргумент {} має бути String, отримано {}", f, i, o))),
        None    => Err(rt_err(format!("{}: потрібен аргумент {}", f, i))),
    }
}

fn need_num(args: &[Value], i: usize, f: &str) -> VR<f64> {
    match args.get(i) {
        Some(Value::Num(n)) => Ok(*n),
        Some(o) => Err(rt_err(format!("{}: аргумент {} має бути Number, отримано {}", f, i, o))),
        None    => Err(rt_err(format!("{}: потрібен аргумент {}", f, i))),
    }
}

fn need_list(args: &[Value], i: usize, f: &str) -> VR<GcList> {
    match args.get(i) {
        Some(Value::List(l)) => Ok(l.clone()),
        Some(o) => Err(rt_err(format!("{}: аргумент {} має бути List, отримано {}", f, i, o))),
        None    => Err(rt_err(format!("{}: потрібен аргумент {}", f, i))),
    }
}

/// Невід'ємне ціле з числа (для довжин, індексів, кількості повторів).
fn need_count(args: &[Value], i: usize, f: &str) -> VR<usize> {
    let n = need_num(args, i, f)?;
    if n < 0.0 || n.fract() != 0.0 || !n.is_finite() {
        return Err(rt_err(format!("{}: аргумент {} має бути невід'ємним цілим, отримано {}", f, i, n)));
    }
    Ok(n as usize)
}

/// Порівняння значень для `sort`/`sortBy`: Number, String або Bool одного типу.
pub fn cmp_values(a: &Value, b: &Value) -> Result<Ordering, String> {
    match (a, b) {
        (Value::Num(x),  Value::Num(y))  => Ok(x.total_cmp(y)),
        (Value::Str(x),  Value::Str(y))  => Ok(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        _ => Err(format!(
            "не можна порівняти {} і {}: потрібні значення одного типу (Number, String або Bool)",
            a, b
        )),
    }
}

/// Стабільне сортування за ключами; повертає помилку, якщо ключі непорівнянні.
pub fn sort_by_keys(items: Vec<Value>, keys: Vec<Value>) -> Result<Vec<Value>, String> {
    let mut pairs: Vec<(Value, Value)> = keys.into_iter().zip(items).collect();
    let mut failure: Option<String> = None;
    pairs.sort_by(|a, b| match cmp_values(&a.0, &b.0) {
        Ok(o)  => o,
        Err(e) => { if failure.is_none() { failure = Some(e); } Ordering::Equal }
    });
    match failure {
        Some(e) => Err(e),
        None    => Ok(pairs.into_iter().map(|(_, v)| v).collect()),
    }
}

// ---------------------------------------------------------------------------
// Псевдовипадкові числа (xorshift64*)
// ---------------------------------------------------------------------------

static RNG: Mutex<u64> = Mutex::new(0);

fn next_u64() -> u64 {
    let mut s = RNG.lock().unwrap();
    if *s == 0 {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
        *s = nanos | 1;
    }
    let mut x = *s;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *s = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

// ---------------------------------------------------------------------------
// Дата/час: григоріанський календар, UTC (алгоритми Говарда Хіннанта)
// ---------------------------------------------------------------------------

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12; // березень = 0
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 }

fn days_in_month(y: i64, m: i64) -> i64 {
    match m { 1 | 3 | 5 | 7 | 8 | 10 | 12 => 31, 4 | 6 | 9 | 11 => 30, _ => if is_leap(y) { 29 } else { 28 } }
}

/// Необов'язковий аргумент-зсув у хвилинах: |зсув| <= 24 год, ціле.
fn offset_arg(args: &[Value], i: usize, f: &str) -> VR<i64> {
    if i >= args.len() { return Ok(0); }
    let n = need_num(args, i, f)?;
    if n.fract() != 0.0 || n.abs() > 24.0 * 60.0 {
        return Err(rt_err(format!("{}: зсув у хвилинах має бути цілим у межах ±1440, отримано {}", f, n)));
    }
    Ok(n as i64)
}

fn fmt_offset(off: i64) -> String {
    if off == 0 { return "Z".to_string(); }
    let sign = if off < 0 { '-' } else { '+' };
    format!("{}{:02}:{:02}", sign, off.abs() / 60, off.abs() % 60)
}

fn parse_iso(s: &str) -> Result<i64, String> {
    let b = s.trim().as_bytes();
    let digits = |from: usize, len: usize| -> Result<i64, String> {
        let part = b.get(from..from + len).ok_or_else(|| "рядок закороткий".to_string())?;
        if part.iter().all(|c| c.is_ascii_digit()) {
            Ok(std::str::from_utf8(part).unwrap().parse::<i64>().unwrap())
        } else { Err(format!("очікувались цифри на позиції {}", from)) }
    };
    let expect = |pos: usize, ch: u8| -> Result<(), String> {
        if b.get(pos) == Some(&ch) { Ok(()) } else { Err(format!("очікувався '{}' на позиції {}", ch as char, pos)) }
    };
    let y = digits(0, 4)?; expect(4, b'-')?;
    let mo = digits(5, 2)?; expect(7, b'-')?;
    let d = digits(8, 2)?;
    let (mut h, mut mi, mut sec, mut milli) = (0, 0, 0, 0);
    let mut pos = 10;
    if pos < b.len() && (b[pos] == b'T' || b[pos] == b' ') {
        h = digits(11, 2)?; expect(13, b':')?;
        mi = digits(14, 2)?;
        pos = 16;
        if b.get(pos) == Some(&b':') {
            sec = digits(17, 2)?;
            pos = 19;
            if b.get(pos) == Some(&b'.') {
                let start = pos + 1;
                let mut end = start;
                while end < b.len() && b[end].is_ascii_digit() { end += 1; }
                if end == start { return Err("порожня дробова частина секунд".into()); }
                let frac = std::str::from_utf8(&b[start..end]).unwrap();
                let three: String = frac.chars().chain("000".chars()).take(3).collect();
                milli = three.parse::<i64>().unwrap();
                pos = end;
            }
        }
    }
    let mut off = 0i64;
    if pos < b.len() {
        match b[pos] {
            b'Z' | b'z' => { pos += 1; }
            b'+' | b'-' => {
                let sign = if b[pos] == b'-' { -1 } else { 1 };
                let oh = digits(pos + 1, 2)?;
                let (om, next) = if b.get(pos + 3) == Some(&b':') { (digits(pos + 4, 2)?, pos + 6) } else { (digits(pos + 3, 2)?, pos + 5) };
                if oh > 23 || om > 59 { return Err("недопустимий зсув поясу".into()); }
                off = sign * (oh * 60 + om);
                pos = next;
            }
            _ => return Err(format!("зайві символи на позиції {}", pos)),
        }
    }
    if pos != b.len() { return Err(format!("зайві символи на позиції {}", pos)); }
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) || h > 23 || mi > 59 || sec > 59 {
        return Err("недопустима дата або час".into());
    }
    Ok((days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec - off * 60) * 1000 + milli)
}

struct Parts { year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64, milli: i64, weekday: i64 }

fn split_time(ms: i64) -> Parts {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86400);
    let rem  = secs.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    Parts {
        year, month, day,
        hour: rem / 3600, minute: (rem % 3600) / 60, second: rem % 60,
        milli: ms.rem_euclid(1000),
        weekday: (days + 3).rem_euclid(7) + 1, // ISO: 1 = понеділок … 7 = неділя
    }
}

// ---------------------------------------------------------------------------
// Виконання
// ---------------------------------------------------------------------------

pub fn call_ext(name: &str, args: Vec<Value>) -> VR<Value> {
    match name {
        // --- Списки ---
        // Додає елемент НА МІСЦІ (без копіювання) і повертає той самий список.
        // На відміну від `push`, що повертає новий список: O(1) замість O(n).
        "append" => {
            let l = need_list(&args, 0, "append")?;
            l.push(args.get(1).cloned().unwrap_or(Value::Nil));
            Ok(Value::List(l))
        }
        // slice(list, from, to?) -> новий список [from, to); межі обрізаються до довжини
        "slice" => {
            let v = need_list(&args, 0, "slice")?.to_vec();
            let from = need_count(&args, 1, "slice")?.min(v.len());
            let to = if args.len() > 2 { need_count(&args, 2, "slice")?.min(v.len()) } else { v.len() };
            let part = if from <= to { v[from..to].to_vec() } else { vec![] };
            Ok(Value::List(GcList::new(part)))
        }
        "concat" => {
            let mut a = need_list(&args, 0, "concat")?.to_vec();
            a.extend(need_list(&args, 1, "concat")?.to_vec());
            Ok(Value::List(GcList::new(a)))
        }
        // indexOf(list, x) -> індекс першого входження або -1
        "indexOf" => {
            let v = need_list(&args, 0, "indexOf")?.to_vec();
            let x = args.get(1).cloned().unwrap_or(Value::Nil);
            Ok(Value::Num(v.iter().position(|e| *e == x).map(|i| i as f64).unwrap_or(-1.0)))
        }
        // sort(list) -> новий відсортований список (за зростанням; Number, String або Bool)
        "sort" => {
            let v = need_list(&args, 0, "sort")?.to_vec();
            let keys = v.clone();
            let sorted = sort_by_keys(v, keys).map_err(|e| rt_err(format!("sort: {}", e)))?;
            Ok(Value::List(GcList::new(sorted)))
        }
        "sum" => {
            let v = need_list(&args, 0, "sum")?.to_vec();
            let mut total = 0.0;
            for e in v {
                match e {
                    Value::Num(n) => total += n,
                    o => return Err(rt_err(format!("sum: елементи мають бути Number, отримано {}", o))),
                }
            }
            Ok(Value::Num(total))
        }

        // --- Рядки ---
        // strIndexOf(s, sub) -> індекс (у символах) або -1
        "strIndexOf" => {
            let s = need_str(&args, 0, "strIndexOf")?;
            let sub = need_str(&args, 1, "strIndexOf")?;
            Ok(Value::Num(match s.find(sub.as_str()) {
                Some(byte_pos) => s[..byte_pos].chars().count() as f64,
                None => -1.0,
            }))
        }
        "strRepeat" => {
            let s = need_str(&args, 0, "strRepeat")?;
            let n = need_count(&args, 1, "strRepeat")?;
            if s.len().saturating_mul(n) > 100_000_000 {
                return Err(rt_err("strRepeat: результат завеликий (понад 100 МБ)"));
            }
            Ok(Value::Str(s.repeat(n)))
        }
        // strPadLeft(s, width, fill?) / strPadRight — доповнення до ширини (в символах)
        "strPadLeft" | "strPadRight" => {
            let s = need_str(&args, 0, name)?;
            let width = need_count(&args, 1, name)?;
            let fill = if args.len() > 2 { need_str(&args, 2, name)? } else { " ".to_string() };
            let fill_ch = fill.chars().next()
                .ok_or_else(|| rt_err(format!("{}: символ-заповнювач не може бути порожнім", name)))?;
            let len = s.chars().count();
            let pad: String = std::iter::repeat(fill_ch).take(width.saturating_sub(len)).collect();
            Ok(Value::Str(if name == "strPadLeft" { format!("{}{}", pad, s) } else { format!("{}{}", s, pad) }))
        }

        // --- Числа ---
        "sin" => Ok(Value::Num(need_num(&args, 0, "sin")?.sin())),
        "cos" => Ok(Value::Num(need_num(&args, 0, "cos")?.cos())),
        "tan" => Ok(Value::Num(need_num(&args, 0, "tan")?.tan())),
        "exp" => Ok(Value::Num(need_num(&args, 0, "exp")?.exp())),
        // log(x) — натуральний; log(x, base) — за довільною основою
        "log" => {
            let x = need_num(&args, 0, "log")?;
            if x <= 0.0 { return Err(rt_err(format!("log: аргумент має бути > 0, отримано {}", x))); }
            if args.len() > 1 {
                let b = need_num(&args, 1, "log")?;
                if b <= 0.0 || b == 1.0 { return Err(rt_err(format!("log: недопустима основа {}", b))); }
                Ok(Value::Num(x.ln() / b.ln()))
            } else {
                Ok(Value::Num(x.ln()))
            }
        }
        "pi" => Ok(Value::Num(std::f64::consts::PI)),
        "sign" => {
            let x = need_num(&args, 0, "sign")?;
            Ok(Value::Num(if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }))
        }
        // random() -> [0, 1)
        "random" => Ok(Value::Num((next_u64() >> 11) as f64 / (1u64 << 53) as f64)),
        // randInt(lo, hi) -> ціле з [lo, hi] (обидві межі включно)
        "randInt" => {
            let lo = need_num(&args, 0, "randInt")?;
            let hi = need_num(&args, 1, "randInt")?;
            if lo.fract() != 0.0 || hi.fract() != 0.0 || hi < lo {
                return Err(rt_err(format!("randInt: потрібні цілі lo <= hi, отримано {} і {}", lo, hi)));
            }
            let span = (hi - lo) as u64 + 1;
            Ok(Value::Num(lo + (next_u64() % span) as f64))
        }
        // seedRandom(n) — детермінована послідовність (для тестів)
        "seedRandom" => {
            let n = need_num(&args, 0, "seedRandom")?;
            *RNG.lock().unwrap() = (n.to_bits()) | 1;
            Ok(Value::Nil)
        }
        // parseNumber(s) -> Ok(Number) | Err(текст): безпечна альтернатива toNumber
        "parseNumber" => {
            let s = need_str(&args, 0, "parseNumber")?;
            match s.trim().parse::<f64>() {
                Ok(n) if n.is_finite() => Ok(ok(Value::Num(n))),
                _ => Ok(err(format!("'{}' не є числом", s))),
            }
        }

        // --- Час (мілісекунди від 1970-01-01T00:00:00Z) ---
        // Необов'язковий останній аргумент — фіксований зсув від UTC у ХВИЛИНАХ
        // (Київ узимку 120, влітку 180). Іменовані пояси (Europe/Kyiv) не підтримуються:
        // їм потрібна база tzdata.
        //
        // isoTime(ms, offsetMin?) -> "2026-09-28T12:34:56Z" або "...+03:00"
        "isoTime" => {
            let ms = need_num(&args, 0, "isoTime")? as i64;
            let off = offset_arg(&args, 1, "isoTime")?;
            let p = split_time(ms + off * 60_000);
            Ok(Value::Str(format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}",
                p.year, p.month, p.day, p.hour, p.minute, p.second, fmt_offset(off)
            )))
        }
        // dateParts(ms, offsetMin?) -> {year, month, day, hour, minute, second, millisecond, weekday(1=Пн..7=Нд)}
        "dateParts" => {
            let ms = need_num(&args, 0, "dateParts")? as i64;
            let off = offset_arg(&args, 1, "dateParts")?;
            let p = split_time(ms + off * 60_000);
            let mut m = OMap::new();
            for (k, v) in [
                ("year", p.year), ("month", p.month), ("day", p.day),
                ("hour", p.hour), ("minute", p.minute), ("second", p.second),
                ("millisecond", p.milli), ("weekday", p.weekday),
            ] {
                m.insert(MapKey::Str(k.into()), Value::Num(v as f64));
            }
            Ok(Value::Map(GcMap::new(m)))
        }
        // timeFromParts(year, month, day, hour?, minute?, second?, offsetMin?) -> ms
        // Частини — це ЛОКАЛЬНИЙ час у поясі offsetMin (за замовчуванням UTC).
        "timeFromParts" => {
            let get = |i: usize, dflt: i64| -> VR<i64> {
                if i < args.len() { Ok(need_num(&args, i, "timeFromParts")? as i64) } else { Ok(dflt) }
            };
            let y = need_num(&args, 0, "timeFromParts")? as i64;
            let (mo, d) = (get(1, 1)?, get(2, 1)?);
            let (h, mi, s) = (get(3, 0)?, get(4, 0)?, get(5, 0)?);
            let off = offset_arg(&args, 6, "timeFromParts")?;
            if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) ||
               !(0..=23).contains(&h) || !(0..=59).contains(&mi) || !(0..=59).contains(&s) {
                return Err(rt_err(format!(
                    "timeFromParts: недопустима дата/час {}-{}-{} {}:{}:{}", y, mo, d, h, mi, s
                )));
            }
            let days = days_from_civil(y, mo, d);
            Ok(Value::Num(((days * 86400 + h * 3600 + mi * 60 + s - off * 60) * 1000) as f64))
        }
        // parseIso("2026-09-28T12:34:56+03:00") -> Ok(ms) | Err(текст)
        // Формати: YYYY-MM-DD[(T| )HH:MM[:SS[.fff]]][Z|±HH:MM|±HHMM]; без зсуву — UTC.
        "parseIso" => {
            let s = need_str(&args, 0, "parseIso")?;
            match parse_iso(&s) {
                Ok(ms)  => Ok(ok(Value::Num(ms as f64))),
                Err(e)  => Ok(err(format!("parseIso '{}': {}", s, e))),
            }
        }

        // --- Файли (помилки містять шлях) ---
        "fileExists" => Ok(Value::Bool(std::path::Path::new(&need_str(&args, 0, "fileExists")?).exists())),
        // readLines(path) -> Ok(List<String>) без завершального порожнього рядка
        "readLines" => {
            let path = need_str(&args, 0, "readLines")?;
            match std::fs::read_to_string(&path) {
                Ok(c) => Ok(ok(Value::List(GcList::new(
                    c.lines().map(|l| Value::Str(l.to_string())).collect()
                )))),
                Err(e) => Ok(err(format!("readLines '{}': {}", path, e))),
            }
        }
        // listDir(path) -> Ok(List<String>) імен, відсортованих за алфавітом
        "listDir" => {
            let path = need_str(&args, 0, "listDir")?;
            match std::fs::read_dir(&path) {
                Ok(rd) => {
                    let mut names: Vec<String> = rd.filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect();
                    names.sort();
                    Ok(ok(Value::List(GcList::new(names.into_iter().map(Value::Str).collect()))))
                }
                Err(e) => Ok(err(format!("listDir '{}': {}", path, e))),
            }
        }
        "deleteFile" => {
            let path = need_str(&args, 0, "deleteFile")?;
            match std::fs::remove_file(&path) {
                Ok(_)  => Ok(ok(Value::Nil)),
                Err(e) => Ok(err(format!("deleteFile '{}': {}", path, e))),
            }
        }
        // mkdir(path) — створює й проміжні теки
        "mkdir" => {
            let path = need_str(&args, 0, "mkdir")?;
            match std::fs::create_dir_all(&path) {
                Ok(_)  => Ok(ok(Value::Nil)),
                Err(e) => Ok(err(format!("mkdir '{}': {}", path, e))),
            }
        }

        // gcCollectCycles(v1, v2, ...) — усе передане тут ЗАЛИШАЄТЬСЯ живим;
        // усе решта, зареєстроване раніше і недосяжне звідси (типово — цикл
        // Struct/List/Map, який ref-counting сам по собі ніколи не звільнить),
        // примусово звільняється. Повертає кількість очищених об'єктів.
        "gcCollectCycles" => Ok(Value::Num(crate::gc::collect_cycles(&args) as f64)),
        "gcStats" => {
            let s = crate::gc::gc_stats();
            let mut m = OMap::new();
            m.insert(MapKey::Str("allocs".into()), Value::Num(s.total_allocs as f64));
            m.insert(MapKey::Str("drops".into()),  Value::Num(s.total_drops as f64));
            m.insert(MapKey::Str("live".into()),   Value::Num(s.live_objects as f64));
            Ok(Value::Map(GcMap::new(m)))
        }

        other if is_higher_order(other) => Err(rt_err(format!(
            "'{}' викликає функції — її не можна викликати як значення builtin поза VM", other
        ))),
        other => Err(rt_err(format!("Невідома вбудована функція: '{}'", other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip_and_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-03-01 (після високосного лютого) і 2024-02-29
        assert_eq!(civil_from_days(days_from_civil(2000, 3, 1)), (2000, 3, 1));
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        // до 1970
        assert_eq!(civil_from_days(days_from_civil(1969, 12, 31)), (1969, 12, 31));
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn iso_time_and_weekday() {
        // 1970-01-01 — четвер (ISO 4)
        assert_eq!(split_time(0).weekday, 4);
        // 2000-01-01T00:00:00Z = 946684800 с, субота (ISO 6)
        let p = split_time(946_684_800_000);
        assert_eq!((p.year, p.month, p.day, p.weekday), (2000, 1, 1, 6));
        // від'ємний час (до 1970) не дає паніки/зсуву: -1 мс = 1969-12-31T23:59:59.999
        let p = split_time(-1);
        assert_eq!((p.year, p.month, p.day, p.hour, p.minute, p.second, p.milli), (1969, 12, 31, 23, 59, 59, 999));
    }

    #[test]
    fn sort_is_stable_and_rejects_mixed() {
        let v = vec![Value::Num(3.0), Value::Num(1.0), Value::Num(2.0)];
        let s = sort_by_keys(v.clone(), v).unwrap();
        assert_eq!(s, vec![Value::Num(1.0), Value::Num(2.0), Value::Num(3.0)]);
        let mixed = vec![Value::Num(1.0), Value::Str("a".into())];
        assert!(sort_by_keys(mixed.clone(), mixed).is_err());
    }

    #[test]
    fn parse_iso_variants() {
        assert_eq!(parse_iso("1970-01-01").unwrap(), 0);
        assert_eq!(parse_iso("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(parse_iso("1970-01-01 03:00:00+03:00").unwrap(), 0);
        assert_eq!(parse_iso("1970-01-01T00:00:00.250Z").unwrap(), 250);
        assert_eq!(parse_iso("1970-01-01T05:30:00+0530").unwrap(), 0);
        assert_eq!(parse_iso("1969-12-31T19:00-05:00").unwrap(), 0);
        assert!(parse_iso("2023-02-29").is_err());       // не високосний
        assert!(parse_iso("2024-02-29").is_ok());
        assert!(parse_iso("2024-13-01").is_err());
        assert!(parse_iso("2024-01-01T25:00:00").is_err());
        assert!(parse_iso("2024-01-01junk").is_err());
        assert!(parse_iso("nope").is_err());
    }

    #[test]
    fn offsets_format() {
        assert_eq!(fmt_offset(0), "Z");
        assert_eq!(fmt_offset(180), "+03:00");
        assert_eq!(fmt_offset(-330), "-05:30");
    }
}
