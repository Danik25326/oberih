//! Метрики у текстовому форматі Prometheus (exposition format 0.0.4).
//!
//! Один глобальний реєстр на процес. Сервер сам рахує:
//!   * `oberih_http_requests_total{method,status}` — оброблені запити;
//!   * `oberih_http_request_duration_seconds` — гістограма тривалості обробника;
//!   * `oberih_http_in_flight_requests` — запити, що виконуються зараз;
//!   * `oberih_http_rejected_total{status}` — відхилені ще до обробника (400/413/431/503...).
//! Користувач додає свої лічильники: `metricInc("jobs_done")`, `metricAdd("bytes", 512)`.
//! Віддати метрики можна з будь-якого маршруту: `return Response(200, metrics(), {...})`.
//!
//! Кардинальність обмежено навмисно: мітки лише `method` (відомі методи, решта — `OTHER`) і
//! `status`; шлях у мітки НЕ потрапляє (інакше довільні URL розпухли б реєстр), а кількість
//! користувацьких лічильників обмежена `MAX_USER_COUNTERS`.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

/// Верхні межі відер гістограми тривалості (секунди).
pub const BUCKETS: [f64; 11] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];
/// Скільки різних користувацьких лічильників дозволено.
pub const MAX_USER_COUNTERS: usize = 1000;

pub struct Registry {
    started:          Instant,
    http_requests:    BTreeMap<(&'static str, u16), u64>,
    /// Кількість спостережень у кожному відрі (НЕ кумулятивно; кумулюємо при виведенні).
    duration_buckets: [u64; 11],
    duration_sum:     f64,
    duration_count:   u64,
    in_flight:        i64,
    rejected:         BTreeMap<u16, u64>,
    user:             BTreeMap<String, f64>,
}

/// Відомі методи лишаємо як є, усе інше зводимо до `OTHER` (захист від розпухання міток).
pub fn normalize_method(m: &str) -> &'static str {
    match m {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "DELETE" => "DELETE",
        "PATCH" => "PATCH",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => "OTHER",
    }
}

/// Коректне ім'я метрики Prometheus: `[a-zA-Z_:][a-zA-Z0-9_:]*`.
pub fn valid_metric_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == ':' => {}
        _ => return false,
    }
    name.len() <= 200 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn fmt_value(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

impl Registry {
    pub fn new() -> Registry {
        Registry {
            started: Instant::now(),
            http_requests: BTreeMap::new(),
            duration_buckets: [0; 11],
            duration_sum: 0.0,
            duration_count: 0,
            in_flight: 0,
            rejected: BTreeMap::new(),
            user: BTreeMap::new(),
        }
    }

    pub fn http_started(&mut self) {
        self.in_flight += 1;
    }

    pub fn http_finished(&mut self, method: &str, status: u16, secs: f64) {
        self.in_flight -= 1;
        *self.http_requests.entry((normalize_method(method), status)).or_insert(0) += 1;
        let secs = if secs.is_finite() && secs >= 0.0 { secs } else { 0.0 };
        if let Some(i) = BUCKETS.iter().position(|b| secs <= *b) {
            self.duration_buckets[i] += 1;
        }
        self.duration_sum += secs;
        self.duration_count += 1;
    }

    pub fn http_rejected(&mut self, status: u16) {
        *self.rejected.entry(status).or_insert(0) += 1;
    }

    /// Додає до користувацького лічильника (створює його, якщо ще немає).
    pub fn counter_add(&mut self, name: &str, value: f64) -> Result<(), String> {
        if !valid_metric_name(name) {
            return Err(format!("некоректне ім'я метрики '{}': дозволені літери, цифри, '_' і ':', без цифри на початку", name));
        }
        if name.starts_with("oberih_") {
            return Err("префікс 'oberih_' зарезервовано для вбудованих метрик".to_string());
        }
        if !value.is_finite() || value < 0.0 {
            return Err(format!("лічильник можна лише збільшувати скінченним невід'ємним числом, отримано {}", value));
        }
        if !self.user.contains_key(name) && self.user.len() >= MAX_USER_COUNTERS {
            return Err(format!("забагато різних метрик (максимум {})", MAX_USER_COUNTERS));
        }
        *self.user.entry(name.to_string()).or_insert(0.0) += value;
        Ok(())
    }

    pub fn render(&self) -> String {
        let mut out = String::with_capacity(2048);

        out.push_str("# HELP oberih_build_info Версія середовища виконання Oberih.\n");
        out.push_str("# TYPE oberih_build_info gauge\n");
        out.push_str(&format!("oberih_build_info{{version=\"{}\"}} 1\n", env!("CARGO_PKG_VERSION")));

        out.push_str("# HELP oberih_uptime_seconds Час роботи процесу.\n");
        out.push_str("# TYPE oberih_uptime_seconds gauge\n");
        out.push_str(&format!("oberih_uptime_seconds {}\n", fmt_value(self.started.elapsed().as_secs_f64().floor())));

        out.push_str("# HELP oberih_http_requests_total Кількість оброблених HTTP-запитів.\n");
        out.push_str("# TYPE oberih_http_requests_total counter\n");
        for ((method, status), n) in &self.http_requests {
            out.push_str(&format!("oberih_http_requests_total{{method=\"{}\",status=\"{}\"}} {}\n", method, status, n));
        }

        out.push_str("# HELP oberih_http_request_duration_seconds Тривалість обробки запиту обробником.\n");
        out.push_str("# TYPE oberih_http_request_duration_seconds histogram\n");
        let mut cumulative = 0u64;
        for (i, bound) in BUCKETS.iter().enumerate() {
            cumulative += self.duration_buckets[i];
            out.push_str(&format!("oberih_http_request_duration_seconds_bucket{{le=\"{}\"}} {}\n", fmt_value(*bound), cumulative));
        }
        out.push_str(&format!("oberih_http_request_duration_seconds_bucket{{le=\"+Inf\"}} {}\n", self.duration_count));
        out.push_str(&format!("oberih_http_request_duration_seconds_sum {}\n", self.duration_sum));
        out.push_str(&format!("oberih_http_request_duration_seconds_count {}\n", self.duration_count));

        out.push_str("# HELP oberih_http_in_flight_requests Запити, що виконуються просто зараз.\n");
        out.push_str("# TYPE oberih_http_in_flight_requests gauge\n");
        out.push_str(&format!("oberih_http_in_flight_requests {}\n", self.in_flight.max(0)));

        out.push_str("# HELP oberih_http_rejected_total Запити, відхилені ще до обробника (некоректні, завеликі, перевантаження).\n");
        out.push_str("# TYPE oberih_http_rejected_total counter\n");
        for (status, n) in &self.rejected {
            out.push_str(&format!("oberih_http_rejected_total{{status=\"{}\"}} {}\n", status, n));
        }

        for (name, value) in &self.user {
            out.push_str(&format!("# TYPE {} counter\n", name));
            out.push_str(&format!("{} {}\n", name, fmt_value(*value)));
        }
        out
    }
}

// --- глобальний реєстр процесу ---

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn global() -> MutexGuard<'static, Registry> {
    REGISTRY
        .get_or_init(|| Mutex::new(Registry::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub fn http_started() { global().http_started(); }
pub fn http_finished(method: &str, status: u16, secs: f64) { global().http_finished(method, status, secs); }
pub fn http_rejected(status: u16) { global().http_rejected(status); }
pub fn counter_add(name: &str, value: f64) -> Result<(), String> { global().counter_add(name, value) }
pub fn render() -> String { global().render() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_requests_by_method_and_status() {
        let mut r = Registry::new();
        r.http_started(); r.http_finished("GET", 200, 0.001);
        r.http_started(); r.http_finished("GET", 200, 0.002);
        r.http_started(); r.http_finished("POST", 500, 0.003);
        let text = r.render();
        assert!(text.contains("oberih_http_requests_total{method=\"GET\",status=\"200\"} 2\n"), "{}", text);
        assert!(text.contains("oberih_http_requests_total{method=\"POST\",status=\"500\"} 1\n"), "{}", text);
        assert!(text.contains("oberih_http_in_flight_requests 0\n"), "{}", text);
    }

    #[test]
    fn unknown_methods_are_collapsed_to_other() {
        let mut r = Registry::new();
        for m in ["AAAA", "BBBB", "CCCC"] {
            r.http_started();
            r.http_finished(m, 404, 0.0);
        }
        let text = r.render();
        assert!(text.contains("method=\"OTHER\",status=\"404\"} 3\n"), "{}", text);
        assert!(!text.contains("AAAA"), "{}", text);
    }

    #[test]
    fn in_flight_gauge_tracks_running_requests() {
        let mut r = Registry::new();
        r.http_started();
        r.http_started();
        assert!(r.render().contains("oberih_http_in_flight_requests 2\n"));
        r.http_finished("GET", 200, 0.0);
        assert!(r.render().contains("oberih_http_in_flight_requests 1\n"));
    }

    #[test]
    fn histogram_is_cumulative_and_consistent() {
        let mut r = Registry::new();
        // 0.004 -> відро 0.005; 0.04 -> відро 0.05; 0.3 -> відро 0.5; 99 -> лише +Inf
        for secs in [0.004, 0.04, 0.3, 99.0] {
            r.http_started();
            r.http_finished("GET", 200, secs);
        }
        let t = r.render();
        assert!(t.contains("_bucket{le=\"0.005\"} 1\n"), "{}", t);
        assert!(t.contains("_bucket{le=\"0.05\"} 2\n"), "{}", t);
        assert!(t.contains("_bucket{le=\"0.5\"} 3\n"), "{}", t);
        assert!(t.contains("_bucket{le=\"10\"} 3\n"), "{}", t);
        assert!(t.contains("_bucket{le=\"+Inf\"} 4\n"), "{}", t);
        assert!(t.contains("oberih_http_request_duration_seconds_count 4\n"), "{}", t);
        // сума = 0.004 + 0.04 + 0.3 + 99
        assert!(t.contains("oberih_http_request_duration_seconds_sum 99.344"), "{}", t);
    }

    #[test]
    fn rejected_requests_are_counted_separately() {
        let mut r = Registry::new();
        r.http_rejected(400);
        r.http_rejected(400);
        r.http_rejected(503);
        let t = r.render();
        assert!(t.contains("oberih_http_rejected_total{status=\"400\"} 2\n"), "{}", t);
        assert!(t.contains("oberih_http_rejected_total{status=\"503\"} 1\n"), "{}", t);
    }

    #[test]
    fn user_counters_accumulate_and_render() {
        let mut r = Registry::new();
        r.counter_add("jobs_done", 1.0).unwrap();
        r.counter_add("jobs_done", 2.0).unwrap();
        r.counter_add("bytes_total", 512.5).unwrap();
        let t = r.render();
        assert!(t.contains("# TYPE jobs_done counter\njobs_done 3\n"), "{}", t);
        assert!(t.contains("bytes_total 512.5\n"), "{}", t);
    }

    #[test]
    fn user_counter_names_and_values_are_validated() {
        let mut r = Registry::new();
        assert!(r.counter_add("", 1.0).is_err());
        assert!(r.counter_add("1abc", 1.0).is_err());
        assert!(r.counter_add("has space", 1.0).is_err());
        assert!(r.counter_add("with\"quote", 1.0).is_err());
        assert!(r.counter_add("with\nnewline", 1.0).is_err());
        assert!(r.counter_add("oberih_http_requests_total", 1.0).is_err());
        assert!(r.counter_add("ok", -1.0).is_err());
        assert!(r.counter_add("ok", f64::NAN).is_err());
        assert!(r.counter_add("ok", f64::INFINITY).is_err());
        assert!(r.counter_add("db:query_count", 1.0).is_ok());
        assert!(valid_metric_name("a_b:c9"));
    }

    #[test]
    fn number_of_user_counters_is_capped() {
        let mut r = Registry::new();
        for i in 0..MAX_USER_COUNTERS {
            r.counter_add(&format!("m{}", i), 1.0).unwrap();
        }
        assert!(r.counter_add("one_too_many", 1.0).is_err());
        // але вже наявний лічильник можна й далі збільшувати
        assert!(r.counter_add("m0", 1.0).is_ok());
    }

    #[test]
    fn output_has_help_and_type_for_builtin_metrics() {
        let t = Registry::new().render();
        for name in ["oberih_build_info", "oberih_uptime_seconds", "oberih_http_requests_total",
                     "oberih_http_request_duration_seconds", "oberih_http_in_flight_requests",
                     "oberih_http_rejected_total"] {
            assert!(t.contains(&format!("# HELP {} ", name)), "немає HELP для {}", name);
            assert!(t.contains(&format!("# TYPE {} ", name)), "немає TYPE для {}", name);
        }
    }
}
