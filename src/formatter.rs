/// Форматер коду Oberih — `oberih fmt`.
/// Читає .obh файл, форматує і записує назад (або виводить в stdout).
/// Детерміністичний — той самий код завжди дає той самий результат.

use crate::parser::ast::*;

pub struct Formatter {
    output: String,
    indent: usize,
}

impl Formatter {
    pub fn new() -> Self {
        Formatter { output: String::new(), indent: 0 }
    }

    pub fn format_program(mut self, program: &Program) -> String {
        let mut first = true;
        for item in &program.items {
            if !first { self.output.push('\n'); }
            self.fmt_item(item);
            first = false;
        }
        self.output
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
        self.pushln(&format!("{}struct {}{} {{", priv_kw, s.name, generics));
        self.indent += 1;
        for (i, field) in s.fields.iter().enumerate() {
            let comma = if i + 1 < s.fields.len() { "," } else { "" };
            self.line(&format!("{}: {}{}", field.name, fmt_type(&field.ty), comma));
        }
        self.indent -= 1;
        self.pushln("}");
    }

    // --- Enum ---

    fn fmt_enum(&mut self, e: &EnumDecl) {
        self.pushln(&format!("enum {} {{", e.name));
        self.indent += 1;
        for (i, v) in e.variants.iter().enumerate() {
            let comma = if i + 1 < e.variants.len() { "," } else { "" };
            self.line(&format!("{}{}", v, comma));
        }
        self.indent -= 1;
        self.pushln("}");
    }

    // --- Fn ---

    fn fmt_fn(&mut self, f: &FnDecl) {
        let priv_kw = if f.is_private  { "private "  } else { "" };
        let res_kw  = if f.is_resilient { "resilient " } else { "" };
        let generics = fmt_type_params(&f.type_params);

        let params: Vec<String> = f.params.iter()
            .map(|p| format!("{}: {}", p.name, fmt_type(&p.ty)))
            .collect();

        self.push(&format!(
            "{}{}fn {}{} ({}) -> {}",
            priv_kw, res_kw,
            f.name, generics,
            params.join(", "),
            fmt_type(&f.return_type),
        ));
        self.output.push('\n');

        // Модифікатори
        self.indent += 1;
        for m in &f.modifiers {
            self.line(&fmt_modifier(m));
        }
        self.indent -= 1;

        self.pushln("{");
        self.indent += 1;
        for stmt in &f.body {
            self.fmt_stmt(stmt);
        }
        self.indent -= 1;
        self.pushln("}");
    }

    // --- Stmt ---

    fn fmt_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let e = self.fmt_expr(value);
                self.line(&format!("let {} = {}", name, e));
            }
            Stmt::Return { value, .. } => {
                let e = self.fmt_expr(value);
                self.line(&format!("return {}", e));
            }
            Stmt::Assign { target, value, .. } => {
                let t = self.fmt_expr(target);
                let v = self.fmt_expr(value);
                self.line(&format!("{} = {}", t, v));
            }
            Stmt::If { cond, then_body, else_body, .. } => {
                let c = self.fmt_expr(cond);
                self.line(&format!("if ({}) {{", c));
                self.indent += 1;
                for s in then_body { self.fmt_stmt(s); }
                self.indent -= 1;
                if let Some(else_b) = else_body {
                    self.line("} else {");
                    self.indent += 1;
                    for s in else_b { self.fmt_stmt(s); }
                    self.indent -= 1;
                }
                self.line("}");
            }
            Stmt::While { cond, body, .. } => {
                let c = self.fmt_expr(cond);
                self.line(&format!("while ({}) {{", c));
                self.indent += 1;
                for s in body { self.fmt_stmt(s); }
                self.indent -= 1;
                self.line("}");
            }
            Stmt::For { var, iter, body, .. } => {
                let i = self.fmt_expr(iter);
                self.line(&format!("for ({} in {}) {{", var, i));
                self.indent += 1;
                for s in body { self.fmt_stmt(s); }
                self.indent -= 1;
                self.line("}");
            }
            Stmt::Expr(e) => {
                let s = self.fmt_expr(e);
                self.line(&s);
            }
        }
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
                let l = self.fmt_expr(left);
                let r = self.fmt_expr(right);
                let op_str = match op {
                    BinOp::Add   => "+",  BinOp::Sub   => "-",
                    BinOp::Mul   => "*",  BinOp::Div   => "/",
                    BinOp::Eq    => "==", BinOp::NotEq => "!=",
                    BinOp::Lt    => "<",  BinOp::Gt    => ">",
                    BinOp::LtEq  => "<=", BinOp::GtEq  => ">=",
                };
                format!("{} {} {}", l, op_str, r)
            }

            Expr::Neg { expr, .. } => format!("-{}", self.fmt_expr(expr)),

            Expr::Try { expr, .. } => format!("{}?", self.fmt_expr(expr)),

            Expr::Field { object, field, .. } => {
                format!("{}.{}", self.fmt_expr(object), field)
            }

            Expr::MethodCall { object, method, args, .. } => {
                let obj  = self.fmt_expr(object);
                let args = args.iter().map(|a| self.fmt_expr(a)).collect::<Vec<_>>().join(", ");
                format!("{}.{}({})", obj, method, args)
            }

            Expr::Call { callee, args, .. } => {
                let callee = self.fmt_expr(callee);
                let args   = args.iter().map(|a| self.fmt_expr(a)).collect::<Vec<_>>().join(", ");
                format!("{}({})", callee, args)
            }

            Expr::Index { object, index, .. } => {
                format!("{}[{}]", self.fmt_expr(object), self.fmt_expr(index))
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
}

// ---------------------------------------------------------------------------
// Хелпери
// ---------------------------------------------------------------------------

fn fmt_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Simple(n)      => n.clone(),
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

fn fmt_modifier(m: &Modifier) -> String {
    match m {
        Modifier::Deadline(d)    => format!("deadline({})", fmt_dur(d)),
        Modifier::RetryBudget(n) => format!("retryBudget({})", n),
        Modifier::Retries(n)     => format!("retries({})", n),
        Modifier::Fallback(e)    => {
            // Fallback — просто ім'я функції
            if let Expr::Ident(name, _) = e.as_ref() {
                format!("fallback({})", name)
            } else {
                "fallback(...)".to_string()
            }
        }
        Modifier::Timeout(d)     => format!("timeout({})", fmt_dur(d)),
        Modifier::CircuitBreaker { fail_threshold, cooldown } => {
            format!("circuitBreaker(failThreshold: {}, cooldown: {})", fail_threshold, fmt_dur(cooldown))
        }
        Modifier::Idempotent { .. }   => "idempotent(key: ...)".to_string(),
        Modifier::Cache { ttl }       => format!("cache(ttl: {})", fmt_dur(ttl)),
        Modifier::EmergencyFallback(_) => "emergencyFallback(...)".to_string(),
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
}
