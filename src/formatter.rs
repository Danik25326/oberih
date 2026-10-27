/// Форматер коду Oberih — `oberih fmt`.
/// Читає .obh файл, форматує і записує назад (або виводить в stdout).
/// Детерміністичний — той самий код завжди дає той самий результат.

use crate::parser::ast::*;
use crate::lexer::Comment;

pub struct Formatter {
    output: String,
    indent: usize,
    /// Коментарі (рядок, текст, чи хвостовий), відсортовані за рядком — так, як їх
    /// віддає лексер. `comment_idx` — курсор: усі коментарі до нього вже
    /// виведені.
    comments: Vec<Comment>,
    comment_idx: usize,
}

impl Formatter {
    pub fn new() -> Self {
        Formatter { output: String::new(), indent: 0, comments: Vec::new(), comment_idx: 0 }
    }

    pub fn format_program(mut self, program: &Program) -> String {
        self.format_body(program)
    }

    /// Як `format_program`, але вставляє назад `//` коментарі, зібрані
    /// лексером (`Lexer::tokenize_with_comments`), перед тим вузлом AST,
    /// що йде за ними у вихідному файлі. Без цього `oberih fmt` мовчки
    /// видаляв усі коментарі — вони існують лише в потоці токенів лексера,
    /// а не в AST, тому Formatter (який працює тільки з AST) раніше про
    /// них нічого не знав.
    pub fn format_program_with_comments(mut self, program: &Program, comments: Vec<Comment>) -> String {
        self.comments = comments;
        self.format_body(program)
    }

    fn format_body(mut self, program: &Program) -> String {
        let mut first = true;
        for item in &program.items {
            self.emit_comments_before(item_line(item));
            if !first { self.output.push('\n'); }
            self.fmt_item(item);
            first = false;
        }
        // Коментарі в кінці файлу (після останнього item) або всередині
        // останнього блоку, які не вдалось прив'язати до конкретного вузла.
        self.flush_remaining_comments();
        self.output
    }

    /// Виводить (з поточним відступом) усі ще не виведені коментарі, що
    /// стоять у вихідному файлі СТРОГО до рядка `line`.
    fn emit_comments_before(&mut self, line: usize) {
        while self.comment_idx < self.comments.len() && self.comments[self.comment_idx].line < line {
            let text = self.comments[self.comment_idx].text.clone();
            self.line(&text);
            self.comment_idx += 1;
        }
    }

    /// Якщо наступний коментар — «хвостовий» і стоїть на рядку `line`
    /// вихідного файлу, дописує його в кінець щойно виведеного рядка.
    fn attach_trailing(&mut self, line: usize) {
        if let Some(c) = self.comments.get(self.comment_idx) {
            if c.trailing && c.line == line && self.output.ends_with('\n') {
                let text = c.text.clone();
                self.output.pop();
                self.output.push_str("  ");
                self.output.push_str(&text);
                self.output.push('\n');
                self.comment_idx += 1;
            }
        }
    }

    fn flush_remaining_comments(&mut self) {
        while self.comment_idx < self.comments.len() {
            let text = self.comments[self.comment_idx].text.clone();
            self.line(&text);
            self.comment_idx += 1;
        }
    }

    fn push(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn pushln(&mut self, s: &str) {
        self.output.push_str(s);
        self.output.push('\n');
    }

    fn indent(&self) -> String {
        "    ".repeat(self.indent)
    }

    fn line(&mut self, s: &str) {
        let ind = self.indent();
        self.output.push_str(&ind);
        self.output.push_str(s);
        self.output.push('\n');
    }

    // --- Item ---

    fn fmt_item(&mut self, item: &Item) {
        match item {
            Item::Import(i) => {
                self.pushln(&format!("import \"{}\"", i.path));
            }
            Item::Struct(s) => self.fmt_struct(s),
            Item::Enum(e)   => self.fmt_enum(e),
            Item::Fn(f)     => self.fmt_fn(f),
        }
    }

    // --- Struct ---

    fn fmt_struct(&mut self, s: &StructDecl) {
        let priv_kw = if s.is_private { "private " } else { "" };
        let generics = fmt_type_params(&s.type_params);
        self.line(&format!("{}struct {}{} {{", priv_kw, s.name, generics));
        self.attach_trailing(s.span.line);
        self.indent += 1;
        for (i, field) in s.fields.iter().enumerate() {
            self.emit_comments_before(field.span.line);
            let comma = if i + 1 < s.fields.len() { "," } else { "" };
            self.line(&format!("{}: {}{}", field.name, fmt_type(&field.ty), comma));
            self.attach_trailing(field.span.line);
        }
        self.emit_comments_before(s.end_line);
        self.indent -= 1;
        self.line("}");
        self.attach_trailing(s.end_line);
    }

    // --- Enum ---

    fn fmt_enum(&mut self, e: &EnumDecl) {
        self.line(&format!("enum {} {{", e.name));
        self.attach_trailing(e.span.line);
        self.indent += 1;
        for (i, v) in e.variants.iter().enumerate() {
            let vline = e.variant_lines.get(i).copied().unwrap_or(e.span.line);
            self.emit_comments_before(vline);
            let comma = if i + 1 < e.variants.len() { "," } else { "" };
            self.line(&format!("{}{}", v, comma));
            self.attach_trailing(vline);
        }
        self.emit_comments_before(e.end_line);
        self.indent -= 1;
        self.line("}");
        self.attach_trailing(e.end_line);
    }

    // --- Fn ---

    fn fmt_fn(&mut self, f: &FnDecl) {
        let priv_kw = if f.is_private  { "private "  } else { "" };
        let res_kw  = if f.is_resilient { "resilient " } else { "" };
        let generics = fmt_type_params(&f.type_params);

        let params: Vec<String> = f.params.iter()
            .map(|p| format!("{}: {}", p.name, fmt_type(&p.ty)))
            .collect();

        let header = format!(
            "{}{}fn {}{}({}) -> {}",
            priv_kw, res_kw,
            f.name, generics,
            params.join(", "),
            fmt_type(&f.return_type),
        );

        if f.modifiers.is_empty() {
            // fn name(params) -> T {
            self.line(&format!("{} {{", header));
            self.attach_trailing(f.span.line);
        } else {
            // З модифікаторами стійкості: кожен на своєму рядку,
            // а `{` — окремим рядком (так написані всі приклади).
            self.line(&header);
            self.attach_trailing(f.span.line);
            self.indent += 1;
            for m in &f.modifiers {
                let text = fmt_modifier(self, m);
                self.line(&text);
            }
            self.indent -= 1;
            self.line("{");
        }

        self.indent += 1;
        for stmt in &f.body {
            self.emit_comments_before(stmt_line(stmt));
            self.fmt_stmt(stmt);
        }
        self.emit_comments_before(f.end_line);
        self.indent -= 1;
        self.line("}");
        self.attach_trailing(f.end_line);
    }

    // --- Stmt ---

    fn fmt_stmt(&mut self, stmt: &Stmt) {
        let sline = stmt_line(stmt);
        match stmt {
            Stmt::Let { name, ty, value, .. } => {
                let e = self.fmt_expr(value);
                match ty {
                    Some(t) => self.line(&format!("let {}: {} = {}", name, fmt_type(t), e)),
                    None    => self.line(&format!("let {} = {}", name, e)),
                }
                self.attach_trailing(sline);
            }
            Stmt::Return { value, .. } => {
                let e = self.fmt_expr(value);
                self.line(&format!("return {}", e));
                self.attach_trailing(sline);
            }
            Stmt::Assign { target, value, .. } => {
                let t = self.fmt_expr(target);
                let v = self.fmt_expr(value);
                self.line(&format!("{} = {}", t, v));
                self.attach_trailing(sline);
            }
            Stmt::If { cond, then_body, else_body, then_end, end_line, .. } => {
                let c = self.fmt_expr(cond);
                self.line(&format!("if ({}) {{", c));
                self.attach_trailing(sline);
                self.fmt_block(then_body, *then_end);
                if let Some(else_b) = else_body {
                    self.line("} else {");
                    self.attach_trailing(*then_end);
                    self.fmt_block(else_b, *end_line);
                }
                self.line("}");
                self.attach_trailing(*end_line);
            }
            Stmt::While { cond, body, end_line, .. } => {
                let c = self.fmt_expr(cond);
                self.line(&format!("while ({}) {{", c));
                self.attach_trailing(sline);
                self.fmt_block(body, *end_line);
                self.line("}");
                self.attach_trailing(*end_line);
            }
            Stmt::For { var, iter, body, end_line, .. } => {
                let i = self.fmt_expr(iter);
                self.line(&format!("for ({} in {}) {{", var, i));
                self.attach_trailing(sline);
                self.fmt_block(body, *end_line);
                self.line("}");
                self.attach_trailing(*end_line);
            }
            Stmt::Expr(e) => {
                let s = self.fmt_expr(e);
                self.line(&s);
                self.attach_trailing(sline);
            }
        }
    }

    /// Тіло блоку: інструкції з відступом, а також усі коментарі, що стоять
    /// у файлі перед закриваючою `}` (на рядку `close_line`).
    fn fmt_block(&mut self, body: &[Stmt], close_line: usize) {
        self.indent += 1;
        for s in body {
            self.emit_comments_before(stmt_line(s));
            self.fmt_stmt(s);
        }
        self.emit_comments_before(close_line);
        self.indent -= 1;
    }

    // --- Expr ---

    fn fmt_expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::Number(n, _) => {
                if n.fract() == 0.0 { format!("{}", *n as i64) } else { format!("{}", n) }
            }
            Expr::StringLit(s, _) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            Expr::Bool(b, _)      => b.to_string(),
            Expr::Ident(n, _)     => n.clone(),

            Expr::ResultCtor { variant, value, .. } => {
                let inner = self.fmt_expr(value);
                match variant {
                    ResultVariant::Ok  => format!("Ok({})", inner),
                    ResultVariant::Err => format!("Err({})", inner),
                }
            }

            Expr::BinOp { op, left, right, .. } => {
                // Ліві операнди лівоасоціативні: `a - (b - c)` потребує дужок, `(a - b) - c` — ні.
                let lvl = binop_prec(op);
                let l = self.fmt_prec(left, lvl);
                let r = self.fmt_prec(right, lvl + 1);
                let op_str = match op {
                    BinOp::Add   => "+",  BinOp::Sub   => "-",
                    BinOp::Mul   => "*",  BinOp::Div   => "/",
                    BinOp::Eq    => "==", BinOp::NotEq => "!=",
                    BinOp::Lt    => "<",  BinOp::Gt    => ">",
                    BinOp::LtEq  => "<=", BinOp::GtEq  => ">=",
                    BinOp::And   => "&&", BinOp::Or    => "||",
                    BinOp::BitAnd => "&", BinOp::BitOr => "|", BinOp::BitXor => "^",
                    // `>>` друкуємо з пробілом від правого операнда, якщо той починається
                    // з `=`, щоб ніколи ненавмисно не вийшло `>>=` (в мові такого токена
                    // немає, але краще не покладатись на це при читанні згенерованого коду).
                    BinOp::Shl => "<<", BinOp::Shr => ">>",
                };
                format!("{} {} {}", l, op_str, r)
            }

            Expr::Lambda { params, ret, body, .. } => {
                let ps: Vec<String> = params.iter().map(|p| match &p.ty {
                    Some(t) => format!("{}: {}", p.name, fmt_type(t)),
                    None    => p.name.clone(),
                }).collect();
                match body {
                    LambdaBody::Expr(e) => format!("fn({}) => {}", ps.join(", "), self.fmt_expr(e)),
                    LambdaBody::Block(stmts) => {
                        // Тіло форматуємо підформатером із відступом на рівень глибше.
                        // (Коментарі всередині лямбда-блоку підформатеру недоступні —
                        // вони виводяться перед наступною інструкцією охоплюючого блоку.)
                        let mut sub = Formatter::new();
                        sub.indent = self.indent + 1;
                        for s in stmts { sub.fmt_stmt(s); }
                        let ret_s = match ret { Some(t) => format!(" -> {}", fmt_type(t)), None => String::new() };
                        format!("fn({}){} {{\n{}{}}}", ps.join(", "), ret_s, sub.output, "    ".repeat(self.indent))
                    }
                }
            }
            Expr::Neg { expr, .. } => {
                // `- -x` не можна друкувати як `--x` (це стосується і від'ємних літералів)
                let inner = self.fmt_prec(expr, PREC_UNARY);
                if inner.starts_with('-') { format!("-({})", inner) } else { format!("-{}", inner) }
            }
            Expr::Not { expr, .. } => format!("!{}", self.fmt_prec(expr, PREC_UNARY)),
            Expr::BitNot { expr, .. } => format!("~{}", self.fmt_prec(expr, PREC_UNARY)),

            Expr::Try { expr, .. } => format!("{}?", self.fmt_prec(expr, PREC_TRY)),

            Expr::Field { object, field, .. } => {
                format!("{}.{}", self.fmt_prec(object, PREC_POSTFIX), field)
            }

            Expr::MethodCall { object, method, args, .. } => {
                let obj  = self.fmt_prec(object, PREC_POSTFIX);
                let args = args.iter().map(|a| self.fmt_expr(a)).collect::<Vec<_>>().join(", ");
                format!("{}.{}({})", obj, method, args)
            }

            Expr::Call { callee, args, .. } => {
                let callee = self.fmt_prec(callee, PREC_POSTFIX);
                let args   = args.iter().map(|a| self.fmt_expr(a)).collect::<Vec<_>>().join(", ");
                format!("{}({})", callee, args)
            }

            Expr::Index { object, index, .. } => {
                format!("{}[{}]", self.fmt_prec(object, PREC_POSTFIX), self.fmt_expr(index))
            }

            Expr::Match { scrutinee, arms, .. } => {
                let mut s = format!("match {} {{\n", self.fmt_expr(scrutinee));
                let ind = "    ".repeat(self.indent + 1);
                for arm in arms {
                    let pat = fmt_pattern(&arm.pattern);
                    let body = self.fmt_expr(&arm.body);
                    s.push_str(&format!("{}{} => {},\n", ind, pat, body));
                }
                s.push_str(&format!("{}}}", "    ".repeat(self.indent)));
                s
            }

            Expr::Spawn { fn_name, args, .. } => {
                let args = args.iter().map(|a| self.fmt_expr(a)).collect::<Vec<_>>().join(", ");
                format!("spawn {}({})", fn_name, args)
            }

            Expr::List(elems, _) => {
                let items = elems.iter().map(|e| self.fmt_expr(e)).collect::<Vec<_>>().join(", ");
                format!("[{}]", items)
            }

            Expr::Map(entries, _) => {
                let items: Vec<String> = entries.iter()
                    .map(|(k, v)| format!("{}: {}", self.fmt_expr(k), self.fmt_expr(v)))
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
        }
    }

    /// Форматує вираз і бере в дужки, якщо його пріоритет нижчий за `min`.
    /// Без цього `(2 + 3) * 4` друкувалось як `2 + 3 * 4` — форматер міняв
    /// результат програми.
    fn fmt_prec(&self, expr: &Expr, min: u8) -> String {
        let s = self.fmt_expr(expr);
        if expr_prec(expr) < min { format!("({})", s) } else { s }
    }
}

// ---------------------------------------------------------------------------
// Хелпери
// ---------------------------------------------------------------------------

// Рівні пріоритету — дзеркало парсера:
// || < && < порівняння < | < ^ < & < зсуви < + - < * / < унарні < `?` < postfix.
// (Бітові `| ^ &` тісніші за порівняння й слабші за зсуви — як у Python; див.
// коментар біля parse_bit_or у parser/mod.rs.)
const PREC_UNARY: u8 = 10;
const PREC_TRY: u8 = 11;
const PREC_POSTFIX: u8 = 12;

fn binop_prec(op: &BinOp) -> u8 {
    match op {
        BinOp::Or    => 1,
        BinOp::And   => 2,
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => 3,
        BinOp::BitOr  => 4,
        BinOp::BitXor => 5,
        BinOp::BitAnd => 6,
        BinOp::Shl | BinOp::Shr => 7,
        BinOp::Add | BinOp::Sub => 8,
        BinOp::Mul | BinOp::Div => 9,
    }
}

fn expr_prec(e: &Expr) -> u8 {
    match e {
        Expr::BinOp { op, .. }          => binop_prec(op),
        Expr::Neg { .. } | Expr::Not { .. } | Expr::BitNot { .. } => PREC_UNARY,
        Expr::Try { .. }                => PREC_TRY,
        Expr::Lambda { .. }             => 0, // тіло `=>` жадібне: у операндах потрібні дужки
        _                               => 13, // літерали, імена, виклики, поля, індекси, списки, Map, match…
    }
}

fn item_line(item: &Item) -> usize {
    match item {
        Item::Import(i) => i.span.line,
        Item::Fn(f)     => f.span.line,
        Item::Struct(s) => s.span.line,
        Item::Enum(e)   => e.span.line,
    }
}

fn stmt_line(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::Let { span, .. }    => span.line,
        Stmt::Return { span, .. } => span.line,
        Stmt::If { span, .. }     => span.line,
        Stmt::While { span, .. }  => span.line,
        Stmt::For { span, .. }    => span.line,
        Stmt::Assign { span, .. } => span.line,
        Stmt::Expr(e)             => e.span().line,
    }
}

fn fmt_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Simple(n)      => n.clone(),
        TypeExpr::Func(ps, ret)  => {
            let params: Vec<String> = ps.iter().map(fmt_type).collect();
            match ret {
                Some(r) => format!("Fn({}) -> {}", params.join(", "), fmt_type(r)),
                None    => format!("Fn({})", params.join(", ")),
            }
        }
        TypeExpr::Generic(n, args) => {
            let args: Vec<String> = args.iter().map(fmt_type).collect();
            format!("{}<{}>", n, args.join(", "))
        }
    }
}

fn fmt_type_params(params: &[String]) -> String {
    if params.is_empty() { String::new() }
    else { format!("<{}>", params.join(", ")) }
}

fn fmt_modifier(fmt: &Formatter, m: &Modifier) -> String {
    match m {
        Modifier::Deadline(d)    => format!("deadline({})", fmt_dur(d)),
        Modifier::RetryBudget(n) => format!("retryBudget({})", n),
        Modifier::Retries(n)     => format!("retries({})", n),
        Modifier::Fallback(e)    => format!("fallback({})", fmt.fmt_expr(e)),
        Modifier::Timeout(d)     => format!("timeout({})", fmt_dur(d)),
        Modifier::CircuitBreaker { fail_threshold, cooldown } => {
            format!("circuitBreaker(failThreshold: {}, cooldown: {})", fail_threshold, fmt_dur(cooldown))
        }
        Modifier::Idempotent { key }  => format!("idempotent(key: {})", fmt.fmt_expr(key)),
        Modifier::Cache { ttl }       => format!("cache(ttl: {})", fmt_dur(ttl)),
        Modifier::EmergencyFallback(e) => format!("emergencyFallback({})", fmt.fmt_expr(e)),
        Modifier::RateLimit { n, per } => format!("rateLimit({}, per: {})", n, fmt_dur(per)),
        Modifier::Bulkhead { max_concurrent } => format!("bulkhead(maxConcurrent: {})", max_concurrent),
        Modifier::Hedging { after }   => format!("hedging(after: {})", fmt_dur(after)),
        Modifier::Durable             => "durable".to_string(),
        Modifier::Traced              => "traced".to_string(),
        Modifier::Budget { tokens, cost } => {
            let mut parts = Vec::new();
            if let Some(t) = tokens { parts.push(format!("tokens: {}", t)); }
            if let Some(c) = cost   { parts.push(format!("cost: {}", c));   }
            format!("budget({})", parts.join(", "))
        }
    }
}

fn fmt_dur(d: &Duration) -> String {
    let unit = match d.unit {
        TimeUnit::Ms => "ms",
        TimeUnit::S  => "s",
        TimeUnit::M  => "m",
    };
    if d.value.fract() == 0.0 {
        format!("{}{}", d.value as i64, unit)
    } else {
        format!("{}{}", d.value, unit)
    }
}

fn fmt_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard        => "_".to_string(),
        Pattern::Literal(lit)    => match lit {
            LiteralPat::Number(n) => format!("{}", n),
            LiteralPat::Str(s)    => format!("\"{}\"", s),
            LiteralPat::Bool(b)   => b.to_string(),
        },
        Pattern::Variant(name)   => name.clone(),
        Pattern::Ctor(name, bind) => format!("{}({})", name, bind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn fmt(src: &str) -> String {
        let tokens  = Lexer::new(src).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        Formatter::new().format_program(&program)
    }

    fn roundtrip(src: &str) {
        // Форматуємо двічі — результат має бути ідентичним (ідемпотентність)
        let first  = fmt(src);
        let second = fmt(&first);
        assert_eq!(first, second, "Форматер не ідемпотентний");
    }

    #[test]
    fn test_simple_fn() {
        roundtrip("fn add(a: Number, b: Number) -> Number { return a }");
    }

    #[test]
    fn test_resilient_fn() {
        roundtrip(r#"
resilient fn getOrder (id: String) -> String
    deadline(2s)
    retryBudget(3)
{
    return id
}
"#);
    }

    #[test]
    fn test_struct() {
        roundtrip("struct Point { x: Number, y: Number }");
    }

    #[test]
    fn test_match() {
        roundtrip(r#"
fn test (x: String) -> String {
    return match x {
        Ok(v) => v,
        Err(e) => e,
        _ => "other"
    }
}
"#);
    }

    fn fmt_keep_comments(src: &str) -> String {
        let (tokens, comments) = Lexer::new(src).tokenize_with_comments().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        Formatter::new().format_program_with_comments(&program, comments)
    }

    #[test]
    fn test_comments_preserved() {
        let src = r#"
// header
fn main() -> Number {
    let x = 5
    // before print
    print(x)
    return 0
}
// trailing
"#;
        let out = fmt_keep_comments(src);
        assert!(out.contains("// header"), "втрачено коментар заголовка: {}", out);
        assert!(out.contains("// before print"), "втрачено коментар усередині тіла: {}", out);
        assert!(out.contains("// trailing"), "втрачено коментар в кінці файлу: {}", out);
        // Ідемпотентність: форматування вже відформатованого виводу без змін.
        let (tokens2, comments2) = Lexer::new(&out).tokenize_with_comments().unwrap();
        let program2 = Parser::new(tokens2).parse_program().unwrap();
        let out2 = Formatter::new().format_program_with_comments(&program2, comments2);
        assert_eq!(out, out2, "форматер з коментарями не ідемпотентний");
    }

    fn fmt_rt(src: &str) -> String {
        let (t, c) = Lexer::new(src).tokenize_with_comments().unwrap();
        let p = Parser::new(t).parse_program().unwrap();
        Formatter::new().format_program_with_comments(&p, c)
    }

    #[test]
    fn test_fn_header_style() {
        let out = fmt_rt("fn main() -> Number { return 0 }");
        assert!(out.starts_with("fn main() -> Number {\n"), "{}", out);
    }

    #[test]
    fn test_comments_before_closing_brace_and_in_bodies() {
        let src = "struct P {\n x: Number\n // в struct\n}\nenum E {\n A,\n B\n // в enum\n}\nfn f() -> Number {\n if (true) {\n  return 1\n  // в if\n }\n // в fn\n return 0\n}\n";
        let out = fmt_rt(src);
        let lines: Vec<&str> = out.lines().collect();
        let pos = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        // кожен коментар стоїть прямо перед своєю закриваючою `}` з правильним відступом
        assert_eq!(lines[pos("// в struct") + 1], "}");
        assert!(lines[pos("// в struct")].starts_with("    //"));
        assert_eq!(lines[pos("// в enum") + 1], "}");
        assert_eq!(lines[pos("// в if") + 1], "    }");
        assert!(lines[pos("// в if")].starts_with("        //"));
        assert_eq!(lines[pos("// в fn") + 2], "}");
        assert_eq!(fmt_rt(&out), out, "не ідемпотентно");
    }

    #[test]
    fn test_trailing_comments_stay_on_their_line() {
        let out = fmt_rt("fn main() -> Number {\n let x = 1 // один\n return x // кінець\n}\n");
        assert!(out.contains("let x = 1  // один"), "{}", out);
        assert!(out.contains("return x  // кінець"), "{}", out);
    }

    #[test]
    fn test_modifier_expressions_not_replaced_by_dots() {
        // Регресія: `fallback(<не ім'я>)` і `idempotent(key: ..)` друкувались як `...`,
        // після чого файл переставав парситись.
        let src = "resilient fn g(id: String) -> String\n    deadline(1s)\n    fallback(\"n/a\")\n    idempotent(key: id)\n{\n return id\n}\n";
        let out = fmt_rt(src);
        assert!(out.contains("fallback(\"n/a\")"), "{}", out);
        assert!(out.contains("idempotent(key: id)"), "{}", out);
        assert!(!out.contains("..."), "{}", out);
        assert_eq!(fmt_rt(&out), out);
    }

    #[test]
    fn test_parentheses_preserved_where_needed() {
        // Регресія: форматер губив дужки і міняв результат програми.
        let cases = [
            ("(2 + 3) * 4",       "(2 + 3) * 4"),
            ("10 - (3 - 1)",      "10 - (3 - 1)"),
            ("10 - 3 - 1",        "10 - 3 - 1"),
            ("2 + 3 * 4",         "2 + 3 * 4"),
            ("(2 + 3) * (4 - 1)", "(2 + 3) * (4 - 1)"),
            ("8 / (2 * 2)",       "8 / (2 * 2)"),
            ("!(1 > 2)",          "!(1 > 2)"),
            ("-(1 + 2)",          "-(1 + 2)"),
            ("-(-3)",             "-(-3)"),
            ("(1 < 2) == true",   "1 < 2 == true"),
            ("(f(1))",            "f(1)"),
            ("(a + b).c",         "(a + b).c"),
            ("(-a)[0]",           "(-a)[0]"),
            ("(f()?).x",          "(f()?).x"),
            ("(a || b) && c",     "(a || b) && c"),
            ("a || b && c",       "a || b && c"),
        ];
        for (input, expected) in cases {
            let src = format!("fn main() -> Number {{ return {} }}", input);
            let out = fmt_rt(&src);
            let line = out.lines().find(|l| l.trim_start().starts_with("return")).unwrap().trim();
            assert_eq!(line, format!("return {}", expected), "вхід: {}", input);
        }
    }

    /// Простий детермінований генератор для property-тесту.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    fn gen_expr(r: &mut Lcg, depth: u32) -> String {
        if depth == 0 || r.next() % 4 == 0 {
            return format!("{}", r.next() % 9 + 1);
        }
        match r.next() % 6 {
            0 => format!("(-{})", gen_expr(r, depth - 1)),
            1 => format!("({} / {})", gen_expr(r, depth - 1), r.next() % 9 + 1),
            n => {
                let op = ["+", "-", "*", "+", "-"][(n - 2) as usize % 5];
                format!("({} {} {})", gen_expr(r, depth - 1), op, gen_expr(r, depth - 1))
            }
        }
    }

    fn eval(src: &str) -> crate::vm::Value {
        let tokens  = Lexer::new(src).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let module  = crate::compiler::Compiler::new().compile_program(&program).unwrap();
        crate::vm::VM::new(module).run().unwrap()
    }

    #[test]
    fn test_format_preserves_semantics_property() {
        // 400 випадкових виразів із дужками: результат до і після форматування має збігатись,
        // а форматування — бути ідемпотентним.
        let mut r = Lcg(12345);
        for i in 0..400 {
            let e = gen_expr(&mut r, 4);
            let src = format!("fn main() -> Number {{ return {} }}", e);
            let formatted = fmt_rt(&src);
            assert_eq!(eval(&src), eval(&formatted), "випадок #{}: {}\n=> {}", i, e, formatted);
            assert_eq!(fmt_rt(&formatted), formatted, "не ідемпотентно, випадок #{}", i);
        }
    }

    #[test]
    fn test_lambda_and_annotation_formatting_roundtrip() {
        let src = r#"
fn ap(f: Fn(Number, String) -> Bool, g: Fn) -> Number {
    let xs: List<Number> = []
    let a = map([1, 2], fn(x) => x * 2)
    let b = fn(x: Number) -> Number {
        let y = x + 1
        return y
    }
    let c = (fn(x) => x)(1)
    let d = filter(xs, fn(x) => x > 1 && x < 9)
    return 0
}
"#;
        let out = fmt_rt(src);
        assert!(out.contains("f: Fn(Number, String) -> Bool"), "{}", out);
        assert!(out.contains("let xs: List<Number> = []"), "{}", out);
        assert!(out.contains("fn(x) => x * 2"), "{}", out);
        assert!(out.contains("(fn(x) => x)(1)"), "{}", out);
        assert_eq!(fmt_rt(&out), out, "не ідемпотентно:\n{}", out);
    }

    #[test]
    fn test_bitwise_formatting_and_precedence_parens() {
        let cases = [
            ("1 & 2", "1 & 2"),
            ("1 | 2 & 3", "1 | 2 & 3"),           // & тісніший за | — дужки не потрібні
            ("(1 | 2) & 3", "(1 | 2) & 3"),        // а тут навпаки — дужки ОБОВ'ЯЗКОВІ
            ("~5", "~5"),
            ("~(1 + 2)", "~(1 + 2)"),
            ("1 << 2 + 3", "1 << 2 + 3"),
            ("(1 << 2) + 3", "(1 << 2) + 3"),
        ];
        for (input, expected) in cases {
            let src = format!("fn main() -> Number {{ return {} }}", input);
            let out = fmt_rt(&src);
            let line = out.lines().find(|l| l.trim_start().starts_with("return")).unwrap().trim();
            assert_eq!(line, format!("return {}", expected), "вхід: {}", input);
        }
    }
}
