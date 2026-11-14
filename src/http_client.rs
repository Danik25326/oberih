/// Oberih HTTP клієнт — підтримує http:// і https://.
/// http://  — чистий TCP (std::net)
/// https:// — TLS через rustls (чистий Rust, без OpenSSL)

use std::io::{Read, Write};
use std::net::TcpStream;
use crate::vm::Value;

#[derive(Debug)]
pub struct HttpResponse {
    pub status:  u16,
    pub body:    String,
    pub headers: Vec<(String, String)>,
}

/// Розбирає URL на (scheme, host, port, path)
pub fn parse_url(url: &str) -> Result<(String, String, u16, String), String> {
    let (scheme, rest) = if url.starts_with("https://") {
        ("https".to_string(), &url[8..])
    } else if url.starts_with("http://") {
        ("http".to_string(), &url[7..])
    } else {
        return Err(format!("Непідтримувана схема URL: {}", url));
    };

    let (host_port, path) = if let Some(idx) = rest.find('/') {
        (&rest[..idx], rest[idx..].to_string())
    } else {
        (rest, "/".to_string())
    };

    let (host, port) = if let Some(idx) = host_port.rfind(':') {
        let port = host_port[idx+1..].parse::<u16>()
            .map_err(|_| format!("Невірний порт: {}", &host_port[idx+1..]))?;
        (host_port[..idx].to_string(), port)
    } else {
        let default_port = if scheme == "https" { 443 } else { 80 };
        (host_port.to_string(), default_port)
    };

    Ok((scheme, host, port, path))
}

/// Виконує HTTP або HTTPS запит
pub fn http_request(
    method:       &str,
    url:          &str,
    body:         Option<&str>,
    timeout_secs: u64,
) -> Result<HttpResponse, String> {
    let (scheme, host, port, path) = parse_url(url)?;

    let body_str = body.unwrap_or("");
    let request  = build_request(method, &host, &path, body_str);

    let raw = if scheme == "https" {
        send_https(&host, port, &request, timeout_secs)?
    } else {
        send_http(&host, port, &request, timeout_secs)?
    };

    parse_response(&raw)
}

fn build_request(method: &str, host: &str, path: &str, body: &str) -> String {
    format!(
        "{} {} HTTP/1.1\r\n\
         Host: {}\r\n\
         User-Agent: Oberih/0.3\r\n\
         Accept: */*\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        method, path, host, body.len(), body
    )
}

/// Максимальний розмір відповіді, який клієнт прочитає в пам'ять. Без ліміту злий або
/// зламаний сервер міг би віддавати дані безкінечно й вичерпати пам'ять процесу.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Момент, до якого весь запит (підключення + рукостискання + запис + читання) має завершитись.
/// `None` — без обмеження (`timeout_secs == 0`, поведінка збережена з попередніх версій).
fn deadline_from(timeout_secs: u64) -> Option<std::time::Instant> {
    if timeout_secs == 0 { None } else { Some(std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs)) }
}

fn remaining(deadline: Option<std::time::Instant>) -> Option<std::time::Duration> {
    deadline.map(|d| d.saturating_duration_since(std::time::Instant::now()))
}

fn io_err(prefix: &str, e: &std::io::Error, timeout_secs: u64) -> String {
    use std::io::ErrorKind::{TimedOut, WouldBlock};
    // Linux повертає WouldBlock, Windows/macOS — TimedOut: для користувача це одне й те саме.
    if matches!(e.kind(), TimedOut | WouldBlock) {
        format!("{}: час очікування вичерпано ({} с)", prefix, timeout_secs)
    } else {
        format!("{}: {}", prefix, e)
    }
}

/// Підключення з урахуванням загального дедлайну (раніше `TcpStream::connect` міг висіти
/// стільки, скільки дозволяла ОС, ігноруючи `timeout`). Перебирає всі адреси DNS-відповіді.
/// Примітка: сам DNS-запит (`to_socket_addrs`) у стандартній бібліотеці таймауту не має.
fn connect_with_deadline(
    host: &str, port: u16, deadline: Option<std::time::Instant>, timeout_secs: u64,
) -> Result<TcpStream, String> {
    use std::net::ToSocketAddrs;
    let addr = format!("{}:{}", host, port);
    let fail = |e: &std::io::Error| io_err(&format!("Не вдалось підключитись до {}", addr), e, timeout_secs);
    let addrs = addr.to_socket_addrs().map_err(|e| fail(&e))?;
    let mut last: Option<std::io::Error> = None;
    for a in addrs {
        let res = match remaining(deadline) {
            None => TcpStream::connect(a),
            Some(left) if left.is_zero() => Err(std::io::Error::from(std::io::ErrorKind::TimedOut)),
            Some(left) => TcpStream::connect_timeout(&a, left),
        };
        match res {
            Ok(s) => return Ok(s),
            Err(e) => {
                let timed_out = matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock);
                last = Some(e);
                if timed_out && remaining(deadline).map_or(false, |d| d.is_zero()) { break; }
            }
        }
    }
    Err(fail(&last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "адресу не знайдено"))))
}

/// Читає до EOF із двома запобіжниками: ЗАГАЛЬНИЙ дедлайн (а не таймаут окремого `read`,
/// який «повільний» сервер, що віддає по байту, обходить) і ліміт розміру.
fn read_all_limited<R: Read>(
    r: &mut R, ctl: &TcpStream, deadline: Option<std::time::Instant>,
    max_bytes: usize, timeout_secs: u64, prefix: &str,
) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        if let Some(left) = remaining(deadline) {
            if left.is_zero() {
                return Err(format!("{}: час очікування вичерпано ({} с)", prefix, timeout_secs));
            }
            ctl.set_read_timeout(Some(left)).ok();
        }
        match r.read(&mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => {
                if out.len() + n > max_bytes {
                    return Err(format!("{}: відповідь перевищує ліміт {} байт", prefix, max_bytes));
                }
                out.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_err(prefix, &e, timeout_secs)),
        }
    }
}

fn send_http(host: &str, port: u16, request: &str, timeout_secs: u64) -> Result<Vec<u8>, String> {
    send_http_with(host, port, request, timeout_secs, MAX_RESPONSE_BYTES)
}

fn send_http_with(
    host: &str, port: u16, request: &str, timeout_secs: u64, max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let deadline = deadline_from(timeout_secs);
    let mut stream = connect_with_deadline(host, port, deadline, timeout_secs)?;
    if let Some(left) = remaining(deadline) { stream.set_write_timeout(Some(left.max(std::time::Duration::from_millis(1)))).ok(); }

    stream.write_all(request.as_bytes())
        .map_err(|e| io_err("Помилка запиту", &e, timeout_secs))?;

    let ctl = stream.try_clone().map_err(|e| format!("Помилка відповіді: {}", e))?;
    let response = read_all_limited(&mut stream, &ctl, deadline, max_bytes, timeout_secs, "Помилка відповіді")?;

    Ok(response)
}

fn send_https(host: &str, port: u16, request: &str, timeout_secs: u64) -> Result<Vec<u8>, String> {
    // Додаткові довірені CA з PEM-файлу: SSL_CERT_FILE (стандартна змінна середовища).
    let extra_ca = std::env::var("SSL_CERT_FILE").ok();
    send_https_with(host, port, request, timeout_secs, extra_ca.as_deref(), MAX_RESPONSE_BYTES)
}

/// Те саме, що `send_https`, але додатковий файл довірених CA передається параметром, а не
/// береться зі змінної середовища. Так перевірка сертифікатів тестується без зміни стану
/// процесу (тести в Rust виконуються паралельно).
fn send_https_with(
    host: &str,
    port: u16,
    request: &str,
    timeout_secs: u64,
    extra_ca_file: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    use rustls::{ClientConfig, ClientConnection, RootCertStore, Stream};
    use rustls_pki_types::ServerName;
    use std::sync::Arc;

    // Завантажуємо кореневі сертифікати
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    // Додаткові довірені CA з PEM-файлу (SSL_CERT_FILE). Потрібно за корпоративними
    // проксі з TLS-перехопленням, чий CA відсутній у вбудованому наборі webpki-roots.
    if let Some(path) = extra_ca_file {
        use rustls_pki_types::{CertificateDer, pem::PemObject};
        match CertificateDer::pem_file_iter(path) {
            Ok(iter) => {
                for cert in iter.flatten() {
                    // Невалідні сертифікати в бандлі просто пропускаємо.
                    let _ = root_store.add(cert);
                }
            }
            Err(e) => return Err(format!("SSL_CERT_FILE '{}': {}", path, e)),
        }
    }

    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|_| format!("Невірне ім'я хоста: {}", host))?;

    let mut conn = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|e| format!("TLS помилка: {}", e))?;

    let deadline = deadline_from(timeout_secs);
    let mut tcp = connect_with_deadline(host, port, deadline, timeout_secs)?;
    // Окремий дескриптор того ж сокета: Stream тримає &mut tcp, а таймаут читання
    // треба оновлювати перед кожним read (див. read_all_limited).
    let ctl = tcp.try_clone().map_err(|e| format!("TLS відповідь помилка: {}", e))?;
    if let Some(left) = remaining(deadline) {
        let left = left.max(std::time::Duration::from_millis(1));
        tcp.set_read_timeout(Some(left)).ok(); // рукостискання відбувається всередині першого write
        tcp.set_write_timeout(Some(left)).ok();
    }

    let mut tls_stream = Stream::new(&mut conn, &mut tcp);

    tls_stream.write_all(request.as_bytes())
        .map_err(|e| io_err("TLS запит помилка", &e, timeout_secs))?;

    let response = read_all_limited(&mut tls_stream, &ctl, deadline, max_bytes, timeout_secs, "TLS відповідь помилка")?;

    Ok(response)
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() { return None; }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Розбирає відповідь. Вхід — СИРІ БАЙТИ: розміри чанків у chunked-кодуванні рахуються в байтах
/// мережевого потоку, тож декодувати їх у `String` до розбору не можна (межа чанка може
/// потрапити всередину багатобайтового символу, а `from_utf8_lossy` ще й міняє довжини).
/// Перетворення в текст — лише для готового тіла.
pub(crate) fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let (head_bytes, body): (&[u8], Vec<u8>) = match find_sub(raw, b"\r\n\r\n") {
        Some(idx) => (&raw[..idx], raw[idx + 4..].to_vec()),
        None      => (raw, Vec::new()),
    };
    let head = String::from_utf8_lossy(head_bytes);
    let mut lines = head.lines();

    // Статус рядок: HTTP/1.1 200 OK
    let status = lines.next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    // Заголовки
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let mut parts = l.splitn(2, ':');
            let k = parts.next()?.trim().to_lowercase();
            let v = parts.next()?.trim().to_string();
            Some((k, v))
        })
        .collect();

    // Chunked transfer encoding
    let body_bytes = if headers.iter().any(|(k, v)| k == "transfer-encoding" && v.contains("chunked")) {
        decode_chunked(&body)
    } else {
        body
    };

    Ok(HttpResponse { status, body: String::from_utf8_lossy(&body_bytes).into_owned(), headers })
}

pub(crate) fn decode_chunked(input: &[u8]) -> Vec<u8> {
    let mut result: Vec<u8> = Vec::new();
    let mut rest = input;
    loop {
        let size_end = match find_sub(rest, b"\r\n") {
            Some(i) => i,
            None    => break,
        };
        let size_line = String::from_utf8_lossy(&rest[..size_end]);
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        if size == 0 { break; }
        rest = &rest[size_end + 2..];
        if rest.len() < size { break; }
        result.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        if rest.starts_with(b"\r\n") { rest = &rest[2..]; }
    }
    result
}

/// Перетворює HttpResponse на Oberih Value::Struct
pub fn response_to_value(resp: HttpResponse) -> Value {
    use crate::vm::OberihStruct;
    use std::collections::HashMap;

    let mut fields = HashMap::new();
    fields.insert("status".to_string(), Value::Num(resp.status as f64));
    fields.insert("body".to_string(),   Value::Str(resp.body));
    fields.insert("ok".to_string(),     Value::Bool(resp.status >= 200 && resp.status < 300));

    Value::Struct(OberihStruct::new("HttpResponse".to_string(), fields))
}

// ---------------------------------------------------------------------------------------------
// Тести перевірки сертифікатів TLS. Повністю локальні: власний CA, локальний TLS-сервер,
// жодних звернень до зовнішніх сайтів (раніше цю перевірку робив лише приклад із badssl.com,
// який падав через мережеві збої сторонньої служби, а не через помилку в коді).
//
// Кожен тест "відхиляє" має парний "приймає": та сама схема з довіреним CA проходить. Без
// такого контролю відмова могла б бути просто збоєм мережі, а не результатом перевірки.
// ---------------------------------------------------------------------------------------------
#[cfg(test)]
mod tls_tests {
    use super::*;
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Pki {
        ca_pem: String,
        ca_file: std::path::PathBuf,
    }

    impl Drop for Pki {
        fn drop(&mut self) { let _ = std::fs::remove_file(&self.ca_file); }
    }

    /// Новий CA, записаний у тимчасовий PEM-файл (унікальне ім'я на кожен виклик).
    fn new_ca() -> (Pki, rcgen::Certificate, KeyPair) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.distinguished_name.push(DnType::CommonName, "Oberih test CA");
        let cert = params.self_signed(&key).unwrap();
        let ca_pem = cert.pem();
        let ca_file = std::env::temp_dir().join(format!(
            "oberih_tls_test_ca_{}_{}.pem",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&ca_file, &ca_pem).unwrap();
        (Pki { ca_pem, ca_file }, cert, key)
    }

    /// Листовий сертифікат, підписаний CA. `expired` — термін дії закінчився у 2021 році.
    fn leaf(
        san: &str,
        expired: bool,
        ca: &rcgen::Certificate,
        ca_key: &KeyPair,
    ) -> (Vec<u8>, Vec<u8>) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec![san.to_string()]).unwrap();
        if expired {
            params.not_before = rcgen::date_time_ymd(2020, 1, 1);
            params.not_after = rcgen::date_time_ymd(2021, 1, 1);
        }
        let cert = params.signed_by(&key, ca, ca_key).unwrap();
        (cert.der().to_vec(), key.serialize_der())
    }

    /// Локальний TLS-сервер на одне з'єднання. Повертає порт. Помилки рукостискання сервер
    /// ігнорує: клієнт, який відхилив сертифікат, обриває з'єднання, і це очікувано.
    fn serve_once(cert_der: Vec<u8>, key_der: Vec<u8>) -> u16 {
        serve_body(cert_der, key_der, b"ok".to_vec())
    }

    fn serve_body(cert_der: Vec<u8>, key_der: Vec<u8>, body: Vec<u8>) -> u16 {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        use rustls::{ServerConfig, ServerConnection, Stream};

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert_der)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
            )
            .unwrap();
        let config = Arc::new(config);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Ok((mut tcp, _)) = listener.accept() else { return };
            tcp.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
            let Ok(mut conn) = ServerConnection::new(config) else { return };
            let mut tls = Stream::new(&mut conn, &mut tcp);
            // Читаємо запит до порожнього рядка, відповідаємо і коректно закриваємо TLS.
            let mut buf = [0u8; 1024];
            let mut got = Vec::new();
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match tls.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = tls.write_all(head.as_bytes());
            let _ = tls.write_all(&body);
            tls.conn.send_close_notify();
            let _ = tls.flush();
        });
        port
    }

    fn get(port: u16, ca_file: Option<&str>) -> Result<Vec<u8>, String> {
        let req = build_request("GET", "localhost", "/", "");
        send_https_with("localhost", port, &req, 10, ca_file, MAX_RESPONSE_BYTES)
    }

    fn ca_path(p: &Pki) -> String { p.ca_file.to_string_lossy().into_owned() }

    #[test]
    fn tls_trusted_certificate_is_accepted_control() {
        let (pki, ca, ca_key) = new_ca();
        let (cert, key) = leaf("localhost", false, &ca, &ca_key);
        let port = serve_once(cert, key);
        let raw = get(port, Some(&ca_path(&pki))).expect("довірений сертифікат має прийматись");
        let raw = String::from_utf8_lossy(&raw).into_owned();
        assert!(raw.contains("200 OK") && raw.ends_with("ok"), "відповідь: {raw}");
        assert!(pki.ca_pem.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn tls_untrusted_issuer_is_rejected() {
        // Той самий коректний сертифікат, але клієнт НЕ довіряє CA (ніби самопідписаний/чужий).
        let (_pki, ca, ca_key) = new_ca();
        let (cert, key) = leaf("localhost", false, &ca, &ca_key);
        let port = serve_once(cert, key);
        let err = get(port, None).expect_err("недовірений видавець мусить відхилятись");
        assert!(err.contains("invalid peer certificate") && err.contains("UnknownIssuer"),
            "очікувалась помилка сертифіката UnknownIssuer, отримано: {err}");
    }

    #[test]
    fn tls_expired_certificate_is_rejected() {
        // CA довірений, підпис коректний, ім'я збігається: єдина вада — термін дії.
        let (pki, ca, ca_key) = new_ca();
        let (cert, key) = leaf("localhost", true, &ca, &ca_key);
        let port = serve_once(cert, key);
        let err = get(port, Some(&ca_path(&pki))).expect_err("прострочений сертифікат мусить відхилятись");
        assert!(err.contains("invalid peer certificate") && err.to_lowercase().contains("expired"),
            "очікувалась помилка Expired, отримано: {err}");
    }

    #[test]
    fn tls_wrong_hostname_is_rejected() {
        // CA довірений, термін дійсний, але сертифікат виданий на інше ім'я.
        let (pki, ca, ca_key) = new_ca();
        let (cert, key) = leaf("wrong.example", false, &ca, &ca_key);
        let port = serve_once(cert, key);
        let err = get(port, Some(&ca_path(&pki))).expect_err("чуже ім'я мусить відхилятись");
        assert!(err.contains("invalid peer certificate") && err.contains("not valid for name"),
            "очікувалась помилка імені, отримано: {err}");
    }

    // ----- Дедлайн і ліміт розміру (те, чого раніше не було) -----

    use std::io::{Read as _, Write as _};
    use std::time::{Duration, Instant};

    /// Простий TCP-сервер на одне з'єднання: `f` отримує сокет після accept.
    fn tcp_server(f: impl FnOnce(std::net::TcpStream) + Send + 'static) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || { if let Ok((s, _)) = l.accept() { f(s); } });
        port
    }

    fn drain_request(s: &mut std::net::TcpStream) {
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut got = Vec::new();
        let mut b = [0u8; 512];
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            match s.read(&mut b) { Ok(0) | Err(_) => return, Ok(n) => got.extend_from_slice(&b[..n]) }
        }
    }

    #[test]
    fn http_slow_drip_server_hits_total_deadline() {
        // Сервер віддає по байту кожні 200 мс (< 1 с), тож таймаут окремого read ніколи
        // не спрацював би. Загальний дедлайн 1 с мусить обірвати запит.
        let port = tcp_server(|mut s| {
            drain_request(&mut s);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n");
            for _ in 0..40 {
                if s.write_all(b"x").is_err() { return; }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let req = build_request("GET", "127.0.0.1", "/", "");
        let t = Instant::now();
        let err = send_http_with("127.0.0.1", port, &req, 1, MAX_RESPONSE_BYTES)
            .expect_err("повільний сервер мусить обриватись за дедлайном");
        let took = t.elapsed();
        assert!(err.contains("час очікування вичерпано"), "помилка: {err}");
        assert!(took < Duration::from_secs(3), "дедлайн 1 с, а запит тривав {took:?}");
    }

    #[test]
    fn http_response_over_limit_is_rejected_and_within_limit_is_ok() {
        let big = tcp_server(|mut s| {
            drain_request(&mut s);
            let body = vec![b'a'; 5000];
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes());
            let _ = s.write_all(&body);
        });
        let small = tcp_server(|mut s| {
            drain_request(&mut s);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        });
        let req = build_request("GET", "127.0.0.1", "/", "");
        let err = send_http_with("127.0.0.1", big, &req, 5, 1000).expect_err("5000 Б > ліміту 1000 Б");
        assert!(err.contains("перевищує ліміт 1000"), "помилка: {err}");
        // Контроль: той самий ліміт, відповідь менша — приймається.
        let ok = send_http_with("127.0.0.1", small, &req, 5, 1000).expect("мала відповідь має проходити");
        assert!(ok.ends_with(b"ok"));
    }

    #[test]
    fn connect_respects_deadline_and_succeeds_when_time_remains() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        // Дедлайн уже минув: підключення не робиться, помилка «час вичерпано».
        let past = Some(Instant::now() - Duration::from_millis(5));
        let err = connect_with_deadline("127.0.0.1", port, past, 1).expect_err("дедлайн минув");
        assert!(err.contains("час очікування вичерпано"), "помилка: {err}");
        // Контроль: той самий сервер і запас часу — підключення успішне.
        let future = Some(Instant::now() + Duration::from_secs(5));
        assert!(connect_with_deadline("127.0.0.1", port, future, 5).is_ok());
    }

    #[test]
    fn tls_response_over_limit_is_rejected() {
        let (pki, ca, ca_key) = new_ca();
        let (cert, key) = leaf("localhost", false, &ca, &ca_key);
        let port = serve_body(cert, key, vec![b'a'; 5000]);
        let req = build_request("GET", "localhost", "/", "");
        let err = send_https_with("localhost", port, &req, 10, Some(&ca_path(&pki)), 1000)
            .expect_err("5000 Б > ліміту 1000 Б");
        assert!(err.contains("перевищує ліміт 1000"), "помилка: {err}");
    }

    #[test]
    fn tls_stalled_handshake_times_out() {
        // Сервер приймає TCP, але мовчить (рукостискання не починає): клієнт не має висіти.
        let port = tcp_server(|s| { std::thread::sleep(Duration::from_secs(5)); drop(s); });
        let req = build_request("GET", "localhost", "/", "");
        let t = Instant::now();
        let err = send_https_with("localhost", port, &req, 1, None, MAX_RESPONSE_BYTES)
            .expect_err("зависле рукостискання мусить обриватись");
        assert!(err.contains("час очікування вичерпано"), "помилка: {err}");
        assert!(t.elapsed() < Duration::from_secs(3), "тривало {:?}", t.elapsed());
    }

    // ----- Регресії, знайдені фазером -----

    #[test]
    fn chunked_boundary_inside_multibyte_char_does_not_panic() {
        // «Привіт» = 12 байт; чанки по 5 і 7 байт: межа посеред літери «и»/«і». Раніше паніка
        // `byte index 5 is not a char boundary`.
        let body = "Привіт".as_bytes();
        let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        raw.extend_from_slice(b"5\r\n"); raw.extend_from_slice(&body[..5]); raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(b"7\r\n"); raw.extend_from_slice(&body[5..]); raw.extend_from_slice(b"\r\n0\r\n\r\n");
        let r = parse_response(&raw).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "Привіт");
    }

    #[test]
    fn chunk_sizes_count_network_bytes_even_for_invalid_utf8() {
        // Чанк 3 байти невалідного UTF-8, далі чанк «ab». Раніше from_utf8_lossy роздував
        // 3 байти до 9 і розміри «з'їжджали».
        let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n".to_vec();
        raw.extend_from_slice(&[0xff, 0xfe, 0xfd]);
        raw.extend_from_slice(b"\r\n2\r\nab\r\n0\r\n\r\n");
        let r = parse_response(&raw).unwrap();
        assert_eq!(r.body, "\u{fffd}\u{fffd}\u{fffd}ab");
    }

    #[test]
    fn non_chunked_body_is_preserved_byte_exact_for_utf8() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\nПривіт".as_bytes().to_vec();
        assert_eq!(parse_response(&raw).unwrap().body, "Привіт");
    }
}
