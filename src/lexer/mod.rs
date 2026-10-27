/// Лексер Oberih.
/// Вхід: &str з вихідним кодом.
/// Вихід: Vec<Token> або LexError.
/// Нуль алокацій крім самого Vec — всі рядки беруться як зрізи (&str).

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Літерали
    Number(f64),
    StringLit(String),
    Bool(bool),

    // Ідентифікатори і ключові слова
    Ident(String),

    // Ключові слова
    Fn,
    Resilient,
    Struct,
    Enum,
    Let,
    Return,
    If,
    Else,
    While,
    For,
    In,
    Match,
    Spawn,
    Import,
    Private,

    // Resilience модифікатори (ключові слова рівня мови)
    Deadline,
    RetryBudget,
    Retries,
    Fallback,
    Timeout,
    CircuitBreaker,
    Idempotent,
    Cache,
    EmergencyFallback,
    RateLimit,
    Bulkhead,
    Hedging,
    Durable,
    Traced,
    Budget,

    // Вбудовані типи
    Ok,
    Err,

    // Логічні оператори
    And,        // &&
    Or,         // ||
    Not,        // !

    // Бітові оператори. `>>` НАВМИСНО не тут — див. коментар при парсингу
    // виразів: окремий токен для `>>` зламав би вкладені дженерики
    // (`Map<String, List<Number>>` закінчується на `>>`, що парситься як
    // ДВІ закриваючі `>`, не одна). Тому `>>` розпізнається на рівні
    // ПАРСЕРА як два сусідні токени Gt, а не тут, на рівні лексера.
    Amp,        // &  (бітове AND)
    Pipe,       // |  (бітове OR)
    Caret,      // ^  (бітове XOR)
    Tilde,      // ~  (бітове NOT)
    Shl,        // << (зсув вліво)

    // Арифметичні оператори
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    Eq,         // ==
    NotEq,      // !=
    Lt,         // <
    Gt,         // >
    LtEq,       // <=
    GtEq,       // >=
    Assign,     // =
    Question,   // ?
    Arrow,      // ->
    FatArrow,   // =>
    Dot,        // .

    // Розділювачі
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LBracket,   // [
    RBracket,   // ]
    Comma,      // ,
    Colon,      // :
    Semicolon,  // ;

    // Кінець файлу
    Eof,
}

#[derive(Debug, Clone)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct Tok {
    pub kind: Token,
    pub span: Span,
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Лексична помилка на {}:{}: {}", self.line, self.col, self.message)
    }
}

/// `//` коментар зі службовою інформацією для форматера.
#[derive(Debug, Clone)]
pub struct Comment {
    pub line: usize,
    /// Повний текст, включно з `//`.
    pub text: String,
    /// true — перед коментарем на тому ж рядку є код (`let x = 1 // ...`).
    pub trailing: bool,
}

pub struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
    col: usize,
    /// Зібрані `//` коментарі: (номер рядка, текст коментаря включно з `//`).
    /// Токенайзер їх пропускає (вони не потрібні парсеру), але форматер
    /// (`oberih fmt`) використовує їх, щоб не губити коментарі при переформатуванні.
    comments: Vec<Comment>,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer { src, pos: 0, line: 1, col: 1, comments: Vec::new() }
    }

    pub fn tokenize(self) -> Result<Vec<Tok>, LexError> {
        let (tokens, _) = self.tokenize_with_comments()?;
        Ok(tokens)
    }

    /// Як `tokenize`, але додатково повертає всі `//` коментарі із файлу
    /// разом з номером рядка, на якому вони починаються.
    pub fn tokenize_with_comments(mut self) -> Result<(Vec<Tok>, Vec<Comment>), LexError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            if self.pos >= self.src.len() {
                tokens.push(Tok { kind: Token::Eof, span: self.span() });
                break;
            }
            let tok = self.next_token()?;
            tokens.push(tok);
        }
        Ok((tokens, self.comments))
    }

    fn span(&self) -> Span {
        Span { line: self.line, col: self.col }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        let mut chars = self.src[self.pos..].chars();
        chars.next();
        chars.next()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.src[self.pos..].chars().next()?;
        self.pos += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // пробіли
            while self.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
                self.advance();
            }
            // // коментарі
            if self.peek() == Some('/') && self.peek2() == Some('/') {
                let start_line = self.line;
                let start_pos  = self.pos;
                while self.peek().map(|c| c != '\n').unwrap_or(false) {
                    self.advance();
                }
                let text = self.src[start_pos..self.pos].to_string();
                let line_start = self.src[..start_pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let trailing = !self.src[line_start..start_pos].trim().is_empty();
                self.comments.push(Comment { line: start_line, text, trailing });
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<Tok, LexError> {
        let span = self.span();
        let c = self.peek().unwrap();

        // Числа
        if c.is_ascii_digit() || (c == '-' && self.peek2().map(|d| d.is_ascii_digit()).unwrap_or(false)) {
            return Ok(Tok { kind: self.lex_number()?, span });
        }

        // Рядки
        if c == '"' {
            return Ok(Tok { kind: self.lex_string()?, span });
        }

        // Ідентифікатори і ключові слова
        if c.is_alphabetic() || c == '_' {
            return Ok(Tok { kind: self.lex_ident_or_keyword(), span });
        }

        // Оператори і розділювачі
        self.advance();
        let kind = match c {
            '+' => Token::Plus,
            '*' => Token::Star,
            '/' => Token::Slash,
            '.' => Token::Dot,
            '(' => Token::LParen,
            ')' => Token::RParen,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            ',' => Token::Comma,
            ':' => Token::Colon,
            ';' => Token::Semicolon,
            '?' => Token::Question,
            '-' => {
                if self.peek() == Some('>') {
                    self.advance();
                    Token::Arrow
                } else {
                    Token::Minus
                }
            }
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::Eq
                } else if self.peek() == Some('>') {
                    self.advance();
                    Token::FatArrow
                } else {
                    Token::Assign
                }
            }
            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    Token::And
                } else {
                    Token::Amp
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    Token::Or
                } else {
                    Token::Pipe
                }
            }
            '^' => Token::Caret,
            '~' => Token::Tilde,
            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::NotEq
                } else {
                    Token::Not
                }
            }
            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::LtEq
                } else if self.peek() == Some('<') {
                    self.advance();
                    Token::Shl
                } else {
                    Token::Lt
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::GtEq
                } else {
                    Token::Gt
                }
            }
            other => {
                return Err(LexError {
                    message: format!("Неочікуваний символ '{}'", other),
                    line: span.line,
                    col: span.col,
                });
            }
        };
        Ok(Tok { kind, span })
    }

    /// `Result`, а не голий `Token`: попри те, що виклик завжди відбувається
    /// після перевірки "це цифра" (див. виклик у `next_token`), явна обробка
    /// помилки парсингу надійніша за `.unwrap()` — і не залежить від того,
    /// чи лишиться це інваріантом після майбутніх змін лексера.
    fn lex_number(&mut self) -> Result<Token, LexError> {
        let span = self.span();
        let start = self.pos;
        if self.peek() == Some('-') {
            self.advance();
        }
        while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            self.advance();
        }
        if self.peek() == Some('.') && self.peek2().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            self.advance();
            while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                self.advance();
            }
        }
        let s = &self.src[start..self.pos];
        s.parse().map(Token::Number).map_err(|_| LexError {
            message: format!("Не вдалось розпізнати число '{}'", s),
            line: span.line, col: span.col,
        })
    }

    fn lex_string(&mut self) -> Result<Token, LexError> {
        let span = self.span();
        self.advance(); // "
        let mut s = String::new();
        loop {
            match self.advance() {
                None => return Err(LexError {
                    message: "Незакритий рядковий літерал".into(),
                    line: span.line,
                    col: span.col,
                }),
                Some('"') => break,
                Some('\\') => {
                    match self.advance() {
                        Some('n')  => s.push('\n'),
                        Some('t')  => s.push('\t'),
                        Some('"')  => s.push('"'),
                        Some('\\') => s.push('\\'),
                        Some('r')  => s.push('\r'),
                        Some(c) => s.push(c),
                        None => return Err(LexError {
                            message: "Незакритий escape в рядку".into(),
                            line: span.line,
                            col: span.col,
                        }),
                    }
                }
                Some(c) => s.push(c),
            }
        }
        Ok(Token::StringLit(s))
    }

    fn lex_ident_or_keyword(&mut self) -> Token {
        let start = self.pos;
        while self.peek().map(|c| c.is_alphanumeric() || c == '_').unwrap_or(false) {
            self.advance();
        }
        let word = &self.src[start..self.pos];
        match word {
            "fn"               => Token::Fn,
            "resilient"        => Token::Resilient,
            "struct"           => Token::Struct,
            "enum"             => Token::Enum,
            "let"              => Token::Let,
            "return"           => Token::Return,
            "if"               => Token::If,
            "else"             => Token::Else,
            "while"            => Token::While,
            "for"              => Token::For,
            "in"               => Token::In,
            "match"            => Token::Match,
            "spawn"            => Token::Spawn,
            "import"           => Token::Import,
            "private"          => Token::Private,
            "true"             => Token::Bool(true),
            "false"            => Token::Bool(false),
            "Ok"               => Token::Ok,
            "Err"              => Token::Err,
            // Resilience модифікатори — залишаємо як Ident
            // щоб їх можна було використовувати як імена полів і змінних.
            // Парсер розпізнає їх за контекстом після оголошення fn.
            _                  => Token::Ident(word.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Vec<Token> {
        Lexer::new(src).tokenize().unwrap()
            .into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn test_keywords() {
        let toks = lex("fn resilient struct let return");
        assert_eq!(toks[0], Token::Fn);
        assert_eq!(toks[1], Token::Resilient);
        assert_eq!(toks[2], Token::Struct);
        assert_eq!(toks[3], Token::Let);
        assert_eq!(toks[4], Token::Return);
    }

    #[test]
    fn test_number() {
        let toks = lex("42 3.14 -7");
        assert_eq!(toks[0], Token::Number(42.0));
        assert_eq!(toks[1], Token::Number(3.14));
        assert_eq!(toks[2], Token::Number(-7.0));
    }

    #[test]
    fn test_string() {
        let toks = lex(r#""привіт" "world\n""#);
        assert_eq!(toks[0], Token::StringLit("привіт".into()));
        assert_eq!(toks[1], Token::StringLit("world\n".into()));
    }

    #[test]
    fn test_operators() {
        let toks = lex("-> => == != <= >= =");
        assert_eq!(toks[0], Token::Arrow);
        assert_eq!(toks[1], Token::FatArrow);
        assert_eq!(toks[2], Token::Eq);
        assert_eq!(toks[3], Token::NotEq);
        assert_eq!(toks[4], Token::LtEq);
        assert_eq!(toks[5], Token::GtEq);
        assert_eq!(toks[6], Token::Assign);
    }

    #[test]
    fn test_comment_skip() {
        let toks = lex("let // це коментар\n x = 1");
        assert_eq!(toks[0], Token::Let);
        assert_eq!(toks[1], Token::Ident("x".into()));
        assert_eq!(toks[2], Token::Assign);
        assert_eq!(toks[3], Token::Number(1.0));
    }

    #[test]
    fn test_resilience_modifiers() {
        // Лексер навмисно НЕ перетворює ці слова в окремі токени — вони
        // залишаються звичайними Ident, щоб `deadline`/`retries`/... не були
        // зарезервованими словами і могли, наприклад, використовуватись як
        // звичайні імена змінних/функцій. Розпізнавання відбувається
        // контекстно в парсері (див. parser/mod.rs, parse_modifiers) — там
        // парсер приймає і Token::Ident("deadline"), і, про всяк випадок,
        // застарілий Token::Deadline. Цей тест раніше перевіряв саме
        // застарілий шлях і тому завжди падав — виправлено на актуальну
        // поведінку лексера.
        let toks = lex("deadline retryBudget fallback circuitBreaker");
        assert_eq!(toks[0], Token::Ident("deadline".into()));
        assert_eq!(toks[1], Token::Ident("retryBudget".into()));
        assert_eq!(toks[2], Token::Ident("fallback".into()));
        assert_eq!(toks[3], Token::Ident("circuitBreaker".into()));
    }

    #[test]
    fn test_real_snippet() {
        let src = r#"
resilient fn getOrder(id: String) -> String
    deadline(2s)
    retryBudget(3)
{
    let result = fetchOrder(id)?
    return result
}
"#;
        let toks = lex(src);
        assert_eq!(toks[0], Token::Resilient);
        assert_eq!(toks[1], Token::Fn);
        assert_eq!(toks[2], Token::Ident("getOrder".into()));
    }
}
