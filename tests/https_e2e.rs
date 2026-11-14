//! Наскрізна перевірка HTTPS у СПРАВЖНЬОМУ бінарнику `oberih`, без інтернету.
//!
//! Замінює залежність CI від зовнішнього сайту (`examples/28_https_smoke.obh` ходить на
//! example.com): якщо той недоступний, збірка червоніла б не через наш код. Тут бінарник
//! запускається як окремий процес і робить `httpGet` до ЛОКАЛЬНОГО TLS-сервера з власним CA.
//! Так перевіряється саме те, що ловив smoke-приклад: у реальному процесі TLS-провайдер
//! (ring) встановлений, рукостискання працює, а `SSL_CERT_FILE` підключає довірений CA.
//! Контрольний випадок без `SSL_CERT_FILE` має ВІДХИЛИТИ сертифікат.

use std::io::{Read, Write};
use std::process::Command;
use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};

fn pki() -> (String, Vec<u8>, Vec<u8>) {
    let ca_key = KeyPair::generate().unwrap();
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    p.distinguished_name.push(DnType::CommonName, "Oberih e2e CA");
    let ca = p.self_signed(&ca_key).unwrap();

    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_string()]).unwrap()
        .signed_by(&leaf_key, &ca, &ca_key).unwrap();
    (ca.pem(), leaf.der().to_vec(), leaf_key.serialize_der())
}

/// TLS-сервер, що обслуговує `conns` з'єднань підряд. Повертає порт.
fn serve(cert: Vec<u8>, key: Vec<u8>, conns: usize) -> u16 {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(cert)], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))
        .unwrap();
    let cfg = Arc::new(cfg);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for _ in 0..conns {
            let Ok((mut tcp, _)) = listener.accept() else { return };
            tcp.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
            let Ok(mut conn) = rustls::ServerConnection::new(cfg.clone()) else { continue };
            let mut tls = rustls::Stream::new(&mut conn, &mut tcp);
            let mut got = Vec::new();
            let mut buf = [0u8; 1024];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match tls.read(&mut buf) { Ok(0) | Err(_) => break, Ok(n) => got.extend_from_slice(&buf[..n]) }
            }
            if got.windows(4).any(|w| w == b"\r\n\r\n") {
                let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        }
    });
    port
}

fn script(port: u16) -> std::path::PathBuf {
    let src = format!(
        "fn main() -> Number {{\n    let r = httpGet(\"https://localhost:{port}/\", 10)\n    let resp = match r {{\n        Ok(data) => data,\n        Err(e) => {{\n            println(\"ERR \" + toString(e))\n            exit(3)\n            return 3\n        }}\n    }}\n    println(\"STATUS \" + toString(resp.status) + \" BODY \" + resp.body)\n    return 0\n}}\n");
    let path = std::env::temp_dir().join(format!("oberih_e2e_{}_{}.obh", std::process::id(), port));
    std::fs::write(&path, src).unwrap();
    path
}

fn run(script: &std::path::Path, ca_file: Option<&std::path::Path>) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oberih"));
    cmd.arg("run").arg(script).env_remove("SSL_CERT_FILE");
    if let Some(f) = ca_file { cmd.env("SSL_CERT_FILE", f); }
    let out = cmd.output().expect("не вдалось запустити oberih");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn real_binary_speaks_https_to_a_server_with_a_trusted_ca() {
    let (ca_pem, cert, key) = pki();
    let port = serve(cert, key, 1);
    let ca_file = std::env::temp_dir().join(format!("oberih_e2e_ca_ok_{}.pem", std::process::id()));
    std::fs::write(&ca_file, ca_pem).unwrap();
    let sc = script(port);
    let (code, out) = run(&sc, Some(&ca_file));
    std::fs::remove_file(&ca_file).ok();
    std::fs::remove_file(&sc).ok();
    assert_eq!(code, 0, "бінарник мав успішно виконати HTTPS-запит. Вивід:\n{out}");
    assert!(out.contains("STATUS 200 BODY ok"), "вивід:\n{out}");
}

#[test]
fn real_binary_rejects_the_same_server_without_the_ca() {
    // Контроль до попереднього тесту: той самий сервер, але CA не довірений → відмова, а не
    // паніка (101) чи мовчазне прийняття.
    let (_ca_pem, cert, key) = pki();
    let port = serve(cert, key, 1);
    let sc = script(port);
    let (code, out) = run(&sc, None);
    std::fs::remove_file(&sc).ok();
    assert_eq!(code, 3, "очікувалась контрольована відмова (exit 3). Вивід:\n{out}");
    assert!(out.contains("ERR") && out.contains("UnknownIssuer"), "вивід:\n{out}");
}
