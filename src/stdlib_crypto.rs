//! Примітиви хешування, підписів і кодування для stdlib: те, без чого неможливі типові
//! бізнес-інтеграції (перевірка підпису вебхука Stripe/GitHub = HMAC-SHA256 + порівняння за
//! сталий час; JWT HS256 = HMAC + base64url; AWS SigV4 = ланцюжок HMAC-SHA256 над сирими
//! байтами; ідентифікатори = UUID).
//!
//! Без нових залежностей: SHA-256 і HMAC реалізовано тут і перевірено офіційними векторами
//! (NIST FIPS 180-4, RFC 4231); випадкові байти беруться з криптографічного генератора
//! `rustls` (ring), який уже є в збірці.
//!
//! Чесні межі: це НЕ криптографічна бібліотека загального призначення. Немає шифрування
//! (AES/ChaCha), асиметричних підписів (RSA/Ed25519), KDF (PBKDF2/Argon2/bcrypt) — паролі
//! цим модулем безпечно зберігати не можна. Усі функції працюють із рядками UTF-8; сирі
//! байти (для ланцюжків HMAC) передаються як hex-рядки (`hmacSha256Hex`).

use crate::vm::{RuntimeError, Value};

type VR<T> = Result<T, RuntimeError>;

fn rt_err(msg: impl Into<String>) -> RuntimeError { RuntimeError::General(msg.into()) }
fn ok(v: Value) -> Value { Value::Ok(Box::new(v)) }
fn err(msg: impl Into<String>) -> Value { Value::Err(Box::new(Value::Str(msg.into()))) }

pub fn is_ext(name: &str) -> bool {
    matches!(name,
        "sha256" | "hmacSha256" | "hmacSha256Hex" | "constantTimeEq" |
        "hexEncode" | "hexDecode" |
        "base64Encode" | "base64Decode" | "base64UrlEncode" | "base64UrlDecode" |
        "base64EncodeHex" | "base64UrlEncodeHex" | "base64DecodeToHex" | "base64UrlDecodeToHex" |
        "uuid" | "randomHex"
    )
}

fn need_str(args: &[Value], i: usize, f: &str) -> VR<String> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(o) => Err(rt_err(format!("{}: аргумент {} має бути String, отримано {}", f, i + 1, o))),
        None    => Err(rt_err(format!("{}: потрібен аргумент {}", f, i + 1))),
    }
}

// ------------------------------------------------------------------ SHA-256 (FIPS 180-4)
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    // Доповнення: 0x80, нулі, довжина в бітах (u64 big-endian) — до кратності 64 байтам.
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 { w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]); }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1); d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) { *x = x.wrapping_add(y); }
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() { out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes()); }
    out
}

/// HMAC-SHA256 (RFC 2104): ключ довший за блок (64 Б) спершу хешується.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 { k[..32].copy_from_slice(&sha256(key)); } else { k[..key.len()].copy_from_slice(key); }
    let mut inner = Vec::with_capacity(64 + msg.len());
    inner.extend(k.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(msg);
    let ih = sha256(&inner);
    let mut outer = Vec::with_capacity(96);
    outer.extend(k.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&ih);
    sha256(&outer)
}

// ------------------------------------------------------------------ hex і base64
pub fn hex_encode(b: &[u8]) -> String { b.iter().map(|x| format!("{:02x}", x)).collect() }

pub fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 { return Err("hexDecode: непарна кількість символів".into()); }
    let val = |c: u8| -> Result<u8, String> {
        match c { b'0'..=b'9' => Ok(c - b'0'), b'a'..=b'f' => Ok(c - b'a' + 10), b'A'..=b'F' => Ok(c - b'A' + 10),
                  _ => Err(format!("hexDecode: недопустимий символ '{}'", c as char)) }
    };
    s.as_bytes().chunks(2).map(|p| Ok(val(p[0])? << 4 | val(p[1])?)).collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64_encode(data: &[u8], url: bool) -> String {
    let t = if url { B64URL } else { B64 };
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(t[(n >> 18) as usize & 63] as char);
        out.push(t[(n >> 12) as usize & 63] as char);
        if c.len() > 1 { out.push(t[(n >> 6) as usize & 63] as char); } else if !url { out.push('='); }
        if c.len() > 2 { out.push(t[n as usize & 63] as char); } else if !url { out.push('='); }
    }
    out
}

pub fn base64_decode(s: &str, url: bool) -> Result<Vec<u8>, String> {
    let t = if url { B64URL } else { B64 };
    let body = s.trim_end_matches('=');
    if s.len() - body.len() > 2 { return Err("base64: надто багато '='".into()); }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in body.bytes() {
        let v = t.iter().position(|&x| x == ch).ok_or_else(|| format!("base64: недопустимий символ '{}'", ch as char))? as u32;
        acc = acc << 6 | v; bits += 6;
        if bits >= 8 { bits -= 8; out.push((acc >> bits) as u8); acc &= (1 << bits) - 1; }
    }
    if body.len() % 4 == 1 { return Err("base64: некоректна довжина".into()); }
    Ok(out)
}

// ------------------------------------------------------------------ випадкові байти
fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; n];
    rustls::crypto::ring::default_provider().secure_random.fill(&mut buf)
        .map_err(|_| "не вдалось отримати випадкові байти від ОС".to_string())?;
    Ok(buf)
}

fn uuid_v4() -> Result<String, String> {
    let mut b = random_bytes(16)?;
    b[6] = (b[6] & 0x0f) | 0x40; // версія 4
    b[8] = (b[8] & 0x3f) | 0x80; // варіант RFC 4122
    let h = hex_encode(&b);
    Ok(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}

/// Порівняння за сталий час (не залежить від позиції першої відмінності): для підписів і токенів.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ------------------------------------------------------------------ диспетчер
pub fn call_ext(name: &str, args: Vec<Value>) -> VR<Value> {
    match name {
        "sha256"          => Ok(Value::Str(hex_encode(&sha256(need_str(&args, 0, name)?.as_bytes())))),
        "hmacSha256"      => Ok(Value::Str(hex_encode(&hmac_sha256(need_str(&args, 0, name)?.as_bytes(), need_str(&args, 1, name)?.as_bytes())))),
        "hmacSha256Hex"   => match hex_decode(&need_str(&args, 0, name)?) {
            Ok(key) => Ok(ok(Value::Str(hex_encode(&hmac_sha256(&key, need_str(&args, 1, name)?.as_bytes()))))),
            Err(e)  => Ok(err(format!("hmacSha256Hex: ключ — не hex ({})", e))),
        },
        "constantTimeEq"  => Ok(Value::Bool(constant_time_eq(need_str(&args, 0, name)?.as_bytes(), need_str(&args, 1, name)?.as_bytes()))),
        "hexEncode"       => Ok(Value::Str(hex_encode(need_str(&args, 0, name)?.as_bytes()))),
        "hexDecode"       => Ok(match hex_decode(&need_str(&args, 0, name)?) {
            Ok(b)  => match String::from_utf8(b) { Ok(s) => ok(Value::Str(s)), Err(_) => err("hexDecode: результат не є коректним UTF-8") },
            Err(e) => err(e),
        }),
        "base64Encode"    => Ok(Value::Str(base64_encode(need_str(&args, 0, name)?.as_bytes(), false))),
        "base64UrlEncode" => Ok(Value::Str(base64_encode(need_str(&args, 0, name)?.as_bytes(), true))),
        "base64Decode" | "base64UrlDecode" => Ok(match base64_decode(&need_str(&args, 0, name)?, name == "base64UrlDecode") {
            Ok(b)  => match String::from_utf8(b) { Ok(s) => ok(Value::Str(s)), Err(_) => err("base64: результат не є коректним UTF-8") },
            Err(e) => err(e),
        }),
        // Міст hex <-> base64 для СИРИХ байтів (підпис JWT = base64url від байтів HMAC, а вони
        // не є коректним UTF-8, тож через рядкове API інакше не передати).
        "base64EncodeHex" | "base64UrlEncodeHex" => Ok(match hex_decode(&need_str(&args, 0, name)?) {
            Ok(b)  => ok(Value::Str(base64_encode(&b, name == "base64UrlEncodeHex"))),
            Err(e) => err(e),
        }),
        "base64DecodeToHex" | "base64UrlDecodeToHex" => Ok(match base64_decode(&need_str(&args, 0, name)?, name == "base64UrlDecodeToHex") {
            Ok(b)  => ok(Value::Str(hex_encode(&b))),
            Err(e) => err(e),
        }),
        "uuid"            => match uuid_v4() { Ok(u) => Ok(Value::Str(u)), Err(e) => Err(rt_err(e)) },
        "randomHex"       => {
            let n = match args.get(0) { Some(Value::Num(n)) if n.fract() == 0.0 && *n >= 1.0 && *n <= 1024.0 => *n as usize,
                _ => return Err(rt_err("randomHex: кількість байтів має бути цілим числом 1..=1024")) };
            match random_bytes(n) { Ok(b) => Ok(Value::Str(hex_encode(&b))), Err(e) => Err(rt_err(e)) }
        }
        _ => Err(rt_err(format!("невідома функція '{}'", name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> String { hex_encode(&sha256(s.as_bytes())) }

    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(h(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(h("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(h("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
                   "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        // Межі доповнення: 55, 56, 63, 64, 65 байт (де найчастіше помиляються).
        for (n, want) in [(55, "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318"),
                          (56, "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"),
                          (64, "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb")] {
            assert_eq!(h(&"a".repeat(n)), want, "довжина {n}");
        }
        assert_eq!(hex_encode(&sha256(&vec![b'a'; 1_000_000])), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn hmac_sha256_matches_rfc4231() {
        let t = |key: &[u8], data: &[u8]| hex_encode(&hmac_sha256(key, data));
        assert_eq!(t(&[0x0b; 20], b"Hi There"), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        assert_eq!(t(b"Jefe", b"what do ya want for nothing?"), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(t(&[0xaa; 20], &[0xdd; 50]), "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe");
        // ключ довший за блок хешується спершу (випадок 6)
        assert_eq!(t(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First"),
                   "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn base64_matches_rfc4648_and_roundtrips() {
        for (plain, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64_encode(plain.as_bytes(), false), enc);
            assert_eq!(base64_decode(enc, false).unwrap(), plain.as_bytes());
            assert_eq!(base64_encode(plain.as_bytes(), true), enc.trim_end_matches('='));
            assert_eq!(base64_decode(enc.trim_end_matches('='), true).unwrap(), plain.as_bytes(), "url без паддінгу: {plain}");
        }
        // base64url замінює + і /
        assert_eq!(base64_encode(&[0xfb, 0xff, 0xfe], false), "+//+");
        assert_eq!(base64_encode(&[0xfb, 0xff, 0xfe], true), "-__-");
        // некоректне
        for bad in ["Zm9v!", "Z", "Zg===", "Zm 9v"] { assert!(base64_decode(bad, false).is_err(), "{bad}"); }
        // довільні байти туди й назад
        let data: Vec<u8> = (0..=255u8).collect();
        assert_eq!(base64_decode(&base64_encode(&data, false), false).unwrap(), data);
        assert_eq!(base64_decode(&base64_encode(&data, true), true).unwrap(), data);
    }

    #[test]
    fn hex_roundtrips_and_rejects_garbage() {
        assert_eq!(hex_encode(b"\x00\xffAz"), "00ff417a");
        assert_eq!(hex_decode("00FF417a").unwrap(), b"\x00\xffAz");
        for bad in ["abc", "zz", "0g"] { assert!(hex_decode(bad).is_err(), "{bad}"); }
    }

    #[test]
    fn constant_time_eq_is_correct() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn uuid_and_random_have_the_right_shape_and_are_not_repeated() {
        let u = uuid_v4().unwrap();
        let p: Vec<&str> = u.split('-').collect();
        assert_eq!(p.iter().map(|x| x.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12], "{u}");
        assert!(u.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert_eq!(&p[2][..1], "4", "версія 4: {u}");
        assert!("89ab".contains(&p[3][..1]), "варіант RFC 4122: {u}");
        let set: std::collections::HashSet<String> = (0..200).map(|_| uuid_v4().unwrap()).collect();
        assert_eq!(set.len(), 200, "UUID не мають повторюватись");
        assert_eq!(random_bytes(32).unwrap().len(), 32);
    }

    #[test]
    fn builtins_through_the_dispatcher_and_chained_hmac_for_sigv4_style_use() {
        let call = |n: &str, a: Vec<Value>| call_ext(n, a).unwrap();
        let s = |x: &str| Value::Str(x.to_string());
        assert_eq!(call("sha256", vec![s("abc")]), s("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert_eq!(call("base64Encode", vec![s("foobar")]), s("Zm9vYmFy"));
        // ланцюжок: k2 = HMAC(k1_bytes, msg) без втрати сирих байтів (hex-ключ)
        let k1 = call("hmacSha256", vec![s("AWS4secret"), s("20260101")]);
        let k2 = match call("hmacSha256Hex", vec![k1.clone(), s("eu-west-1")]) { Value::Ok(v) => *v, o => panic!("{o:?}") };
        let manual = hex_encode(&hmac_sha256(&hmac_sha256(b"AWS4secret", b"20260101"), b"eu-west-1"));
        assert_eq!(k2, Value::Str(manual));
        // помилки: не hex-ключ і не-String
        assert!(matches!(call("hmacSha256Hex", vec![s("not-hex"), s("x")]), Value::Err(_)));
        assert!(call_ext("sha256", vec![Value::Num(1.0)]).is_err());
        assert!(call_ext("randomHex", vec![Value::Num(0.0)]).is_err());
        assert!(matches!(call("base64Decode", vec![s("!!")]), Value::Err(_)));
        assert!(matches!(call("hexDecode", vec![s("ff")]), Value::Err(_)), "0xff — не коректний UTF-8");
    }

    #[test]
    fn jwt_hs256_from_jwt_io_is_reproduced_exactly() {
        let s = |x: &str| Value::Str(x.to_string());
        let unwrap = |v: Value| match v { Value::Ok(b) => *b, o => panic!("{o:?}") };
        let header = base64_encode(br#"{"alg":"HS256","typ":"JWT"}"#, true);
        let payload = base64_encode(br#"{"sub":"1234567890","name":"John Doe","iat":1516239022}"#, true);
        let input = format!("{header}.{payload}");
        let mac_hex = call_ext("hmacSha256", vec![s("your-256-bit-secret"), s(&input)]).unwrap();
        let sig = unwrap(call_ext("base64UrlEncodeHex", vec![mac_hex.clone()]).unwrap());
        assert_eq!(format!("{input}.{}", match &sig { Value::Str(x) => x.clone(), _ => unreachable!() }), "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c");
        // і назад: підпис із токена -> hex, збігається з обчисленим (перевірка за сталий час)
        let back = unwrap(call_ext("base64UrlDecodeToHex", vec![sig]).unwrap());
        assert_eq!(back, mac_hex);
        assert!(matches!(call_ext("base64UrlEncodeHex", vec![s("zz")]).unwrap(), Value::Err(_)));
        assert!(matches!(call_ext("base64DecodeToHex", vec![s("!!")]).unwrap(), Value::Err(_)));
        let std = unwrap(call_ext("base64EncodeHex", vec![s("fbfffe")]).unwrap());
        assert_eq!(std, s("+//+"));
    }
}
