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

fn send_http(host: &str, port: u16, request: &str, timeout_secs: u64) -> Result<String, String> {
    let addr = format!("{}:{}", host, port);
    let timeout = std::time::Duration::from_secs(timeout_secs);

    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| format!("Не вдалось підключитись до {}: {}", addr, e))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    stream.write_all(request.as_bytes())
        .map_err(|e| format!("Помилка запиту: {}", e))?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)
        .map_err(|e| format!("Помилка відповіді: {}", e))?;

    String::from_utf8_lossy(&response).into_owned().pipe_ok()
}

fn send_https(host: &str, port: u16, request: &str, timeout_secs: u64) -> Result<String, String> {
    use rustls::{ClientConfig, ClientConnection, RootCertStore, Stream};
    use rustls_pki_types::ServerName;
    use std::sync::Arc;

    // Завантажуємо кореневі сертифікати
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|_| format!("Невірне ім'я хоста: {}", host))?;

    let mut conn = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|e| format!("TLS помилка: {}", e))?;

    let addr = format!("{}:{}", host, port);
    let timeout = std::time::Duration::from_secs(timeout_secs);

    let mut tcp = TcpStream::connect(&addr)
        .map_err(|e| format!("Не вдалось підключитись до {}: {}", addr, e))?;
    tcp.set_read_timeout(Some(timeout)).ok();
    tcp.set_write_timeout(Some(timeout)).ok();

    let mut tls_stream = Stream::new(&mut conn, &mut tcp);

    tls_stream.write_all(request.as_bytes())
        .map_err(|e| format!("TLS запит помилка: {}", e))?;

    let mut response = Vec::new();
    tls_stream.read_to_end(&mut response)
        .map_err(|e| format!("TLS відповідь помилка: {}", e))?;

    String::from_utf8_lossy(&response).into_owned().pipe_ok()
}

fn parse_response(raw: &str) -> Result<HttpResponse, String> {
    let (head, body) = if let Some(idx) = raw.find("\r\n\r\n") {
        (&raw[..idx], raw[idx+4..].to_string())
    } else {
        (raw, String::new())
    };

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
    let final_body = if headers.iter().any(|(k, v)| k == "transfer-encoding" && v.contains("chunked")) {
        decode_chunked(&body)
    } else {
        body
    };

    Ok(HttpResponse { status, body: final_body, headers })
}

/// Декодує chunked transfer encoding
fn decode_chunked(input: &str) -> String {
    let mut result = String::new();
    let mut rest = input;
    loop {
        let size_end = match rest.find("\r\n") {
            Some(i) => i,
            None    => break,
        };
        let size_str = rest[..size_end].trim();
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0"), 16)
            .unwrap_or(0);
        if size == 0 { break; }
        rest = &rest[size_end+2..];
        if rest.len() < size { break; }
        result.push_str(&rest[..size]);
        rest = &rest[size..];
        if rest.starts_with("\r\n") { rest = &rest[2..]; }
    }
    result
}

// Хелпер trait для pipe
trait PipeOk {
    fn pipe_ok(self) -> Result<String, String>;
}
impl PipeOk for String {
    fn pipe_ok(self) -> Result<String, String> { Ok(self) }
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
