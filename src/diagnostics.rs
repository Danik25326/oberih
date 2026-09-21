/// Людські повідомлення про помилки з номером рядка і підказками.
/// Замість "RuntimeError: ..." — конкретне пояснення що пішло не так.

use std::fmt;

// ---------------------------------------------------------------------------
// Діагностичне повідомлення
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message:  String,
    pub line:     usize,
    pub col:      usize,
    pub hint:     Option<String>,
    pub source:   Option<String>,  // фрагмент вихідного коду
}

impl Diagnostic {
    pub fn error(msg: impl Into<String>, line: usize, col: usize) -> Self {
        Diagnostic {
            severity: Severity::Error,
            message:  msg.into(),
            line,
            col,
            hint:     None,
            source:   None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_source(mut self, src: &str) -> Self {
        if self.line > 0 {
            let lines: Vec<&str> = src.lines().collect();
            if let Some(line) = lines.get(self.line - 1) {
                self.source = Some(line.to_string());
            }
        }
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let label = match self.severity {
            Severity::Error   => "\x1b[31mПомилка\x1b[0m",
            Severity::Warning => "\x1b[33mПопередження\x1b[0m",
            Severity::Note    => "\x1b[34mПримітка\x1b[0m",
        };

        writeln!(f, "{} на рядку {}:{}", label, self.line, self.col)?;
        writeln!(f, "  {}", self.message)?;

        if let Some(src) = &self.source {
            writeln!(f, "")?;
            writeln!(f, "  {:>4} │ {}", self.line, src)?;
            if self.col > 0 {
                let spaces = " ".repeat(self.col + 6);
                writeln!(f, "{}^", spaces)?;
            }
        }

        if let Some(hint) = &self.hint {
            writeln!(f, "")?;
            write!(f, "  \x1b[32mПідказка:\x1b[0m {}", hint)?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Перетворення типових помилок на людські повідомлення
// ---------------------------------------------------------------------------

pub fn explain_runtime_error(msg: &str, line: usize, col: usize) -> Diagnostic {
    // Ділення на нуль
    if msg.contains("нуль") || msg.contains("zero") || msg.contains("division") {
        return Diagnostic::error("Ділення на нуль", line, col)
            .with_hint("Перевір дільник перед операцією: if (b != 0) { ... }");
    }

    // Stack underflow
    if msg.contains("underflow") || msg.contains("Stack") {
        return Diagnostic::error("Внутрішня помилка VM: порожній стек", line, col)
            .with_hint("Це помилка компілятора — повідом про неї");
    }

    // Поле не існує
    if msg.contains("поле") && msg.contains("не існує") {
        return Diagnostic::error(msg, line, col)
            .with_hint("Перевір назву поля — вона має збігатись з оголошенням struct");
    }

    // Невідома функція
    if msg.contains("не знайдена") || msg.contains("не знайдено") {
        return Diagnostic::error(msg, line, col)
            .with_hint("Перевір що функція оголошена і правильно написана назва");
    }

    // Типова помилка
    if msg.contains("тип") || msg.contains("Number") || msg.contains("String") {
        return Diagnostic::error(msg, line, col)
            .with_hint("Перевір типи аргументів — Oberih строго типізована");
    }

    // Timeout
    if msg.contains("timeout") {
        return Diagnostic::error("Функція перевищила deadline", line, col)
            .with_hint("Збільш deadline або оптимізуй функцію. \
                        Додай fallback для graceful degradation");
    }

    // Circuit breaker
    if msg.contains("circuit open") {
        return Diagnostic::error("Circuit breaker відкритий", line, col)
            .with_hint("Сервіс тимчасово недоступний. \
                        Circuit breaker відкрився після серії збоїв — \
                        спробуй пізніше або використай fallback");
    }

    // Rate limit
    if msg.contains("rate limited") {
        return Diagnostic::error("Перевищено rate limit", line, col)
            .with_hint("Забагато запитів за одиницю часу. \
                        Додай затримку між викликами або збільш ліміт");
    }

    // Bulkhead
    if msg.contains("bulkhead full") {
        return Diagnostic::error("Bulkhead заповнений", line, col)
            .with_hint("Досягнуто максимум паралельних викликів. \
                        Збільш maxConcurrent або зменш паралельне навантаження");
    }

    // Assert
    if msg.contains("assert") {
        return Diagnostic::error(msg, line, col)
            .with_hint("Перевірка не пройшла — перегляньте умову assert");
    }

    // Panic
    if msg.contains("panic") {
        return Diagnostic::error(msg, line, col)
            .with_hint("panic() зупиняє програму. \
                        Використовуй Err(...) замість panic для обробних помилок");
    }

    // Загальна
    Diagnostic::error(msg, line, col)
}

/// Виводить список типових помилок красиво.
pub fn print_diagnostics(diagnostics: &[Diagnostic]) {
    for d in diagnostics {
        eprintln!("{}", d);
        eprintln!();
    }
    let errors = diagnostics.iter()
        .filter(|d| matches!(d.severity, Severity::Error))
        .count();
    if errors > 0 {
        eprintln!("\x1b[31m{} помилок\x1b[0m", errors);
    }
}
