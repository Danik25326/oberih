pub mod ast;

use crate::lexer::{Token, Tok};
use ast::*;

pub struct Parser {
    tokens: Vec<Tok>,
    pos:    usize,
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub line:    usize,
    pub col:     usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Синтаксична помилка на {}:{}: {}", self.line, self.col, self.message)
    }
}

type PR<T> = Result<T, ParseError>;

impl Parser {
    pub fn new(tokens: Vec<Tok>) -> Self {
        Parser { tokens, pos: 0 }
    }

    // --- утиліти ---

    fn peek(&self) -> &Token {
        &self.tokens[self.pos].kind
    }

    fn peek_tok(&self) -> &Tok {
        &self.tokens[self.pos]
    }

    fn span(&self) -> Span {
        let s = &self.tokens[self.pos].span;
        Span { line: s.line, col: s.col }
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        &tok.kind
    }

    fn expect(&mut self, expected: &Token) -> PR<()> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            let s = self.peek_tok();
            Err(ParseError {
                message: format!("Очікувалось {:?}, отримано {:?}", expected, self.peek()),
                line: s.span.line,
                col:  s.span.col,
            })
        }
    }

    fn expect_ident(&mut self) -> PR<String> {
        if let Token::Ident(name) = self.peek().clone() {
            self.advance();
            Ok(name)
        } else {
            let s = self.peek_tok();
            Err(ParseError {
                message: format!("Очікувався ідентифікатор, отримано {:?}", self.peek()),
                line: s.span.line,
                col:  s.span.col,
            })
        }
    }

    fn check(&self, tok: &Token) -> bool {
        self.peek() == tok
    }

    fn eat(&mut self, tok: &Token) -> bool {
        if self.peek() == tok {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error(&self, msg: impl Into<String>) -> ParseError {
        let s = self.peek_tok();
        ParseError { message: msg.into(), line: s.span.line, col: s.span.col }
    }

    // --- точка входу ---

    pub fn parse_program(&mut self) -> PR<Program> {
        let mut items = Vec::new();
        while !self.check(&Token::Eof) {
            items.push(self.parse_item()?);
        }
        Ok(Program { items })
    }

    fn parse_item(&mut self) -> PR<Item> {
        match self.peek().clone() {
            Token::Import   => Ok(Item::Import(self.parse_import()?)),
            Token::Struct   => Ok(Item::Struct(self.parse_struct(false)?)),
            Token::Enum     => Ok(Item::Enum(self.parse_enum()?)),
            Token::Private  => {
                self.advance();
                match self.peek().clone() {
                    Token::Struct => Ok(Item::Struct(self.parse_struct(true)?)),
                    Token::Fn     => Ok(Item::Fn(self.parse_fn(true, false)?)),
                    _ => Err(self.error("після 'private' очікується 'fn' або 'struct'")),
                }
            }
            Token::Resilient => {
                self.advance();
                self.expect(&Token::Fn)?;
                Ok(Item::Fn(self.parse_fn(false, true)?))
            }
            Token::Fn => {
                self.advance();
                Ok(Item::Fn(self.parse_fn(false, false)?))
            }
            _ => Err(self.error(format!("Неочікуваний токен {:?} на рівні програми", self.peek()))),
        }
    }

    // --- import ---

    fn parse_import(&mut self) -> PR<ImportDecl> {
        let span = self.span();
        self.expect(&Token::Import)?;
        if let Token::StringLit(path) = self.peek().clone() {
            self.advance();
            Ok(ImportDecl { path, span })
        } else {
            Err(self.error("після 'import' очікується рядковий шлях"))
        }
    }

    // --- struct ---

    fn parse_struct(&mut self, is_private: bool) -> PR<StructDecl> {
        let span = self.span();
        self.expect(&Token::Struct)?;
        let name = self.expect_ident()?;
        let type_params = self.parse_type_params()?;
        self.expect(&Token::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&Token::RBrace) {
            let fspan = self.span();
            let fname = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let ty = self.parse_type_expr()?;
            fields.push(StructField { name: fname, ty, span: fspan });
            self.eat(&Token::Comma);
        }
        self.expect(&Token::RBrace)?;
        Ok(StructDecl { is_private, name, type_params, fields, span })
    }

    // --- enum ---

    fn parse_enum(&mut self) -> PR<EnumDecl> {
        let span = self.span();
        self.expect(&Token::Enum)?;
        let name = self.expect_ident()?;
        self.expect(&Token::LBrace)?;
        let mut variants = Vec::new();
        while !self.check(&Token::RBrace) {
            variants.push(self.expect_ident()?);
            self.eat(&Token::Comma);
        }
        self.expect(&Token::RBrace)?;
        Ok(EnumDecl { name, variants, span })
    }

    // --- fn ---

    fn parse_fn(&mut self, is_private: bool, is_resilient: bool) -> PR<FnDecl> {
        let span = self.span();
        let name = self.expect_ident()?;

        // fn Point.distance — метод
        let method_of = if self.eat(&Token::Dot) {
            let method_name = self.expect_ident()?;
            let struct_name = name.clone();
            // переносимо ім'я: struct_name + "." + method_name
            return self.parse_fn_rest(
                is_private, is_resilient,
                format!("{}.{}", struct_name, method_name),
                Some(struct_name),
                span,
            );
        } else {
            None
        };

        self.parse_fn_rest(is_private, is_resilient, name, method_of, span)
    }

    fn parse_fn_rest(
        &mut self,
        is_private: bool,
        is_resilient: bool,
        name: String,
        method_of: Option<String>,
        span: Span,
    ) -> PR<FnDecl> {
        let type_params = self.parse_type_params()?;
        self.expect(&Token::LParen)?;
        let params = self.parse_params()?;
        self.expect(&Token::RParen)?;
        let return_type = if self.eat(&Token::Arrow) {
            self.parse_type_expr()?
        } else {
            TypeExpr::Simple("Nil".to_string())
        };

        // Модифікатори стійкості
        let mut modifiers = Vec::new();
        while self.is_modifier_start() {
            modifiers.push(self.parse_modifier()?);
        }

        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;

        Ok(FnDecl {
            is_private, is_resilient, name, method_of,
            type_params, params, return_type, modifiers, body, span,
        })
    }

    fn is_modifier_start(&self) -> bool {
        matches!(self.peek(),
            Token::Deadline | Token::RetryBudget | Token::Retries |
            Token::Fallback | Token::Timeout | Token::CircuitBreaker |
            Token::Idempotent | Token::Cache | Token::EmergencyFallback |
            Token::RateLimit | Token::Bulkhead | Token::Hedging |
            Token::Durable | Token::Traced | Token::Budget
        ) || matches!(self.peek(), Token::Ident(n) if matches!(n.as_str(),
            "deadline" | "retryBudget" | "retries" | "fallback" | "timeout" |
            "circuitBreaker" | "idempotent" | "cache" | "emergencyFallback" |
            "rateLimit" | "bulkhead" | "hedging" | "durable" | "traced" | "budget"
        ))
    }

    fn parse_modifier(&mut self) -> PR<Modifier> {
        // Нормалізуємо токен — Ident("deadline") або Token::Deadline однакові
        let name = match self.peek().clone() {
            Token::Ident(n) => { self.advance(); n }
            Token::Deadline        => { self.advance(); "deadline".to_string() }
            Token::RetryBudget     => { self.advance(); "retryBudget".to_string() }
            Token::Retries         => { self.advance(); "retries".to_string() }
            Token::Fallback        => { self.advance(); "fallback".to_string() }
            Token::Timeout         => { self.advance(); "timeout".to_string() }
            Token::CircuitBreaker  => { self.advance(); "circuitBreaker".to_string() }
            Token::Idempotent      => { self.advance(); "idempotent".to_string() }
            Token::Cache           => { self.advance(); "cache".to_string() }
            Token::EmergencyFallback => { self.advance(); "emergencyFallback".to_string() }
            Token::RateLimit       => { self.advance(); "rateLimit".to_string() }
            Token::Bulkhead        => { self.advance(); "bulkhead".to_string() }
            Token::Hedging         => { self.advance(); "hedging".to_string() }
            Token::Durable         => { self.advance(); return Ok(Modifier::Durable) }
            Token::Traced          => { self.advance(); return Ok(Modifier::Traced) }
            Token::Budget          => { self.advance(); "budget".to_string() }
            _ => return Err(self.error("Очікувався модифікатор")),
        };

        // durable і traced без дужок
        if name == "durable" { return Ok(Modifier::Durable); }
        if name == "traced"  { return Ok(Modifier::Traced); }

        self.expect(&Token::LParen)?;
        let m = match name.as_str() {
            "deadline" => Modifier::Deadline(self.parse_duration()?),
            "retryBudget" => Modifier::RetryBudget(self.expect_u32()?),
            "retries"     => Modifier::Retries(self.expect_u32()?),
            "fallback"    => Modifier::Fallback(Box::new(self.parse_expr()?)),
            "timeout"     => Modifier::Timeout(self.parse_duration()?),
            "circuitBreaker" => {
                self.expect_named("failThreshold")?;
                let ft = self.expect_u32()?;
                self.expect(&Token::Comma)?;
                self.expect_named("cooldown")?;
                let cd = self.parse_duration()?;
                Modifier::CircuitBreaker { fail_threshold: ft, cooldown: cd }
            }
            "idempotent" => {
                self.expect_named("key")?;
                Modifier::Idempotent { key: Box::new(self.parse_expr()?) }
            }
            "cache" => {
                self.expect_named("ttl")?;
                Modifier::Cache { ttl: self.parse_duration()? }
            }
            "emergencyFallback" => Modifier::EmergencyFallback(Box::new(self.parse_expr()?)),
            "rateLimit" => {
                let n = self.expect_u32()?;
                self.expect(&Token::Comma)?;
                self.expect_named("per")?;
                let per = self.parse_duration()?;
                Modifier::RateLimit { n, per }
            }
            "bulkhead" => {
                self.expect_named("maxConcurrent")?;
                Modifier::Bulkhead { max_concurrent: self.expect_u32()? }
            }
            "hedging" => {
                self.expect_named("after")?;
                Modifier::Hedging { after: self.parse_duration()? }
            }
            "budget" => {
                let mut tokens_val = None;
                let mut cost_val   = None;
                while !self.check(&Token::RParen) {
                    let key = self.expect_ident()?;
                    self.expect(&Token::Colon)?;
                    let v = self.expect_number()?;
                    match key.as_str() {
                        "tokens" => tokens_val = Some(v),
                        "cost"   => cost_val   = Some(v),
                        _ => {}
                    }
                    self.eat(&Token::Comma);
                }
                Modifier::Budget { tokens: tokens_val, cost: cost_val }
            }
            _ => return Err(self.error(format!("Невідомий модифікатор '{}'", name))),
        };
        self.expect(&Token::RParen)?;
        Ok(m)
    }

    fn expect_named(&mut self, name: &str) -> PR<()> {
        let ident = self.expect_ident()?;
        if ident != name {
            return Err(self.error(format!("Очікувався параметр '{}'", name)));
        }
        self.expect(&Token::Colon)
    }

    fn expect_u32(&mut self) -> PR<u32> {
        if let Token::Number(n) = self.peek().clone() {
            self.advance();
            Ok(n as u32)
        } else {
            Err(self.error("Очікувалось ціле число"))
        }
    }

    fn expect_number(&mut self) -> PR<f64> {
        if let Token::Number(n) = self.peek().clone() {
            self.advance();
            Ok(n)
        } else {
            Err(self.error("Очікувалось число"))
        }
    }

    fn parse_duration(&mut self) -> PR<Duration> {
        let value = self.expect_number()?;
        let unit = match self.peek().clone() {
            Token::Ident(u) => {
                self.advance();
                match u.as_str() {
                    "ms" => TimeUnit::Ms,
                    "s"  => TimeUnit::S,
                    "m"  => TimeUnit::M,
                    _ => return Err(self.error(format!("Невідома одиниця часу '{}'", u))),
                }
            }
            _ => return Err(self.error("Очікувалась одиниця часу: ms, s, m")),
        };
        Ok(Duration { value, unit })
    }

    // --- type params <T, U> ---

    fn parse_type_params(&mut self) -> PR<Vec<String>> {
        if !self.check(&Token::Lt) {
            return Ok(vec![]);
        }
        self.advance();
        let mut params = Vec::new();
        while !self.check(&Token::Gt) {
            params.push(self.expect_ident()?);
            self.eat(&Token::Comma);
        }
        self.expect(&Token::Gt)?;
        Ok(params)
    }

    // --- params ---

    fn parse_params(&mut self) -> PR<Vec<Param>> {
        let mut params = Vec::new();
        while !self.check(&Token::RParen) {
            let span = self.span();
            let name = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let ty = self.parse_type_expr()?;
            params.push(Param { name, ty, span });
            self.eat(&Token::Comma);
        }
        Ok(params)
    }

    // --- type expr ---

    fn parse_type_expr(&mut self) -> PR<TypeExpr> {
        let name = self.expect_ident()?;
        if self.check(&Token::Lt) {
            self.advance();
            let mut args = Vec::new();
            while !self.check(&Token::Gt) {
                args.push(self.parse_type_expr()?);
                self.eat(&Token::Comma);
            }
            self.expect(&Token::Gt)?;
            Ok(TypeExpr::Generic(name, args))
        } else {
            Ok(TypeExpr::Simple(name))
        }
    }

    // --- block ---

    fn parse_block(&mut self) -> PR<Block> {
        let mut stmts = Vec::new();
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    // --- stmt ---

    fn parse_stmt(&mut self) -> PR<Stmt> {
        match self.peek().clone() {
            Token::Let    => self.parse_let(),
            Token::Return => self.parse_return(),
            Token::If     => self.parse_if(),
            Token::While  => self.parse_while(),
            Token::For    => self.parse_for(),
            _             => self.parse_expr_or_assign(),
        }
    }

    fn parse_let(&mut self) -> PR<Stmt> {
        let span = self.span();
        self.expect(&Token::Let)?;
        let name = self.expect_ident()?;
        self.expect(&Token::Assign)?;
        let value = self.parse_expr()?;
        Ok(Stmt::Let { name, value, span })
    }

    fn parse_return(&mut self) -> PR<Stmt> {
        let span = self.span();
        self.expect(&Token::Return)?;
        let value = self.parse_expr()?;
        Ok(Stmt::Return { value, span })
    }

    fn parse_if(&mut self) -> PR<Stmt> {
        let span = self.span();
        self.expect(&Token::If)?;
        self.expect(&Token::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::RParen)?;
        self.expect(&Token::LBrace)?;
        let then_body = self.parse_block()?;
        self.expect(&Token::RBrace)?;
        let else_body = if self.eat(&Token::Else) {
            self.expect(&Token::LBrace)?;
            let b = self.parse_block()?;
            self.expect(&Token::RBrace)?;
            Some(b)
        } else {
            None
        };
        Ok(Stmt::If { cond, then_body, else_body, span })
    }

    fn parse_while(&mut self) -> PR<Stmt> {
        let span = self.span();
        self.expect(&Token::While)?;
        self.expect(&Token::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::RParen)?;
        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;
        Ok(Stmt::While { cond, body, span })
    }

    fn parse_for(&mut self) -> PR<Stmt> {
        let span = self.span();
        self.expect(&Token::For)?;
        self.expect(&Token::LParen)?;
        let var = self.expect_ident()?;
        self.expect(&Token::In)?;
        let iter = self.parse_expr()?;
        self.expect(&Token::RParen)?;
        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;
        Ok(Stmt::For { var, iter, body, span })
    }

    fn parse_expr_or_assign(&mut self) -> PR<Stmt> {
        let span = self.span();
        let expr = self.parse_expr()?;
        if self.eat(&Token::Assign) {
            let value = self.parse_expr()?;
            Ok(Stmt::Assign { target: expr, value, span })
        } else {
            Ok(Stmt::Expr(expr))
        }
    }

    // --- expr (Pratt parser) ---

    fn parse_expr(&mut self) -> PR<Expr> {
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> PR<Expr> {
        let span = self.span();
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Token::Eq    => BinOp::Eq,
                Token::NotEq => BinOp::NotEq,
                Token::Lt    => BinOp::Lt,
                Token::Gt    => BinOp::Gt,
                Token::LtEq  => BinOp::LtEq,
                Token::GtEq  => BinOp::GtEq,
                _            => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::BinOp {
                op,
                left:  Box::new(left),
                right: Box::new(right),
                span:  span.clone(),
            };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> PR<Expr> {
        let span = self.span();
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Token::Plus  => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _            => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            left = Expr::BinOp {
                op,
                left:  Box::new(left),
                right: Box::new(right),
                span:  span.clone(),
            };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> PR<Expr> {
        let span = self.span();
        let mut left = self.parse_try()?;
        loop {
            let op = match self.peek() {
                Token::Star  => BinOp::Mul,
                Token::Slash => BinOp::Div,
                _            => break,
            };
            self.advance();
            let right = self.parse_try()?;
            left = Expr::BinOp {
                op,
                left:  Box::new(left),
                right: Box::new(right),
                span:  span.clone(),
            };
        }
        Ok(left)
    }

    fn parse_try(&mut self) -> PR<Expr> {
        let span = self.span();
        let mut expr = self.parse_postfix()?;
        while self.eat(&Token::Question) {
            expr = Expr::Try { expr: Box::new(expr), span: span.clone() };
        }
        Ok(expr)
    }

    fn parse_postfix(&mut self) -> PR<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            let span = self.span();
            if self.eat(&Token::Dot) {
                let field = self.expect_ident()?;
                if self.check(&Token::LParen) {
                    // method call
                    self.advance();
                    let args = self.parse_args()?;
                    self.expect(&Token::RParen)?;
                    expr = Expr::MethodCall {
                        object: Box::new(expr),
                        method: field,
                        args,
                        span,
                    };
                } else {
                    expr = Expr::Field { object: Box::new(expr), field, span };
                }
            } else if self.check(&Token::LParen) {
                self.advance();
                let args = self.parse_args()?;
                self.expect(&Token::RParen)?;
                expr = Expr::Call { callee: Box::new(expr), args, span };
            } else if self.check(&Token::LBracket) {
                self.advance();
                let index = self.parse_expr()?;
                self.expect(&Token::RBracket)?;
                expr = Expr::Index { object: Box::new(expr), index: Box::new(index), span };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> PR<Expr> {
        let span = self.span();
        match self.peek().clone() {
            Token::Number(n) => {
                self.advance();
                Ok(Expr::Number(n, span))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Expr::StringLit(s, span))
            }
            Token::Bool(b) => {
                self.advance();
                Ok(Expr::Bool(b, span))
            }
            Token::Ok => {
                self.advance();
                self.expect(&Token::LParen)?;
                let val = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(Expr::ResultCtor { variant: ResultVariant::Ok, value: Box::new(val), span })
            }
            Token::Err => {
                self.advance();
                self.expect(&Token::LParen)?;
                let val = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(Expr::ResultCtor { variant: ResultVariant::Err, value: Box::new(val), span })
            }
            Token::Spawn => {
                self.advance();
                let fn_name = self.expect_ident()?;
                self.expect(&Token::LParen)?;
                let args = self.parse_args()?;
                self.expect(&Token::RParen)?;
                Ok(Expr::Spawn { fn_name, args, span })
            }
            Token::Match => {
                self.advance();
                let scrutinee = self.parse_expr()?;
                self.expect(&Token::LBrace)?;
                let mut arms = Vec::new();
                while !self.check(&Token::RBrace) {
                    let arm_span = self.span();
                    let pattern = self.parse_pattern()?;
                    self.expect(&Token::FatArrow)?;
                    let body = self.parse_expr()?;
                    arms.push(MatchArm { pattern, body, span: arm_span });
                    self.eat(&Token::Comma);
                }
                self.expect(&Token::RBrace)?;
                Ok(Expr::Match { scrutinee: Box::new(scrutinee), arms, span })
            }
            Token::If => {
                self.advance();
                self.expect(&Token::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                self.expect(&Token::LBrace)?;
                let then_stmts = self.parse_block()?;
                self.expect(&Token::RBrace)?;
                let then_expr = block_to_expr(then_stmts, &span)?;
                let else_expr = if self.eat(&Token::Else) {
                    self.expect(&Token::LBrace)?;
                    let else_stmts = self.parse_block()?;
                    self.expect(&Token::RBrace)?;
                    Box::new(block_to_expr(else_stmts, &span)?)
                } else {
                    Box::new(Expr::Bool(false, span.clone()))
                };
                Ok(Expr::Match {
                    scrutinee: Box::new(cond),
                    arms: vec![
                        MatchArm {
                            pattern: Pattern::Literal(LiteralPat::Bool(true)),
                            body:    then_expr,
                            span:    span.clone(),
                        },
                        MatchArm {
                            pattern: Pattern::Wildcard,
                            body:    *else_expr,
                            span:    span.clone(),
                        },
                    ],
                    span,
                })
            }
            Token::Minus => {
                self.advance();
                let expr = self.parse_primary()?;
                Ok(Expr::Neg { expr: Box::new(expr), span })
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            Token::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                while !self.check(&Token::RBracket) {
                    elems.push(self.parse_expr()?);
                    self.eat(&Token::Comma);
                }
                self.expect(&Token::RBracket)?;
                Ok(Expr::List(elems, span))
            }
            Token::LBrace => {
                self.advance();
                let mut entries = Vec::new();
                while !self.check(&Token::RBrace) {
                    let key = self.parse_expr()?;
                    self.expect(&Token::Colon)?;
                    let val = self.parse_expr()?;
                    entries.push((key, val));
                    self.eat(&Token::Comma);
                }
                self.expect(&Token::RBrace)?;
                Ok(Expr::Map(entries, span))
            }
            Token::Ident(name) => {
                self.advance();
                Ok(Expr::Ident(name, span))
            }
            other => Err(self.error(format!("Неочікуваний токен у виразі: {:?}", other))),
        }
    }

    fn parse_args(&mut self) -> PR<Vec<Expr>> {
        let mut args = Vec::new();
        while !self.check(&Token::RParen) {
            args.push(self.parse_expr()?);
            self.eat(&Token::Comma);
        }
        Ok(args)
    }

    fn parse_pattern(&mut self) -> PR<Pattern> {
        match self.peek().clone() {
            Token::Ident(name) if name == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            Token::Number(n) => {
                self.advance();
                Ok(Pattern::Literal(LiteralPat::Number(n)))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Pattern::Literal(LiteralPat::Str(s)))
            }
            Token::Bool(b) => {
                self.advance();
                Ok(Pattern::Literal(LiteralPat::Bool(b)))
            }
            Token::Ok => {
                self.advance();
                self.expect(&Token::LParen)?;
                let bind = self.expect_ident()?;
                self.expect(&Token::RParen)?;
                Ok(Pattern::Ctor("Ok".into(), bind))
            }
            Token::Err => {
                self.advance();
                self.expect(&Token::LParen)?;
                let bind = self.expect_ident()?;
                self.expect(&Token::RParen)?;
                Ok(Pattern::Ctor("Err".into(), bind))
            }
            Token::Ident(name) => {
                self.advance();
                if self.check(&Token::LParen) {
                    self.advance();
                    let bind = self.expect_ident()?;
                    self.expect(&Token::RParen)?;
                    Ok(Pattern::Ctor(name, bind))
                } else {
                    Ok(Pattern::Variant(name))
                }
            }
            _ => Err(self.error("Невалідний патерн у match")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src).tokenize().unwrap();
        Parser::new(tokens).parse_program().unwrap()
    }

    #[test]
    fn test_simple_fn() {
        let p = parse("fn add(a: Number, b: Number) -> Number { return a }");
        assert_eq!(p.items.len(), 1);
        if let Item::Fn(f) = &p.items[0] {
            assert_eq!(f.name, "add");
            assert_eq!(f.params.len(), 2);
            assert!(!f.is_resilient);
        } else {
            panic!("Очікувалась FnDecl");
        }
    }

    #[test]
    fn test_resilient_fn_with_modifiers() {
        let src = r#"
resilient fn getOrder(id: String) -> String
    deadline(2s)
    retryBudget(3)
    fallback("cached")
{
    return id
}
"#;
        let p = parse(src);
        if let Item::Fn(f) = &p.items[0] {
            assert!(f.is_resilient);
            assert_eq!(f.modifiers.len(), 3);
            assert!(matches!(f.modifiers[0], Modifier::Deadline(_)));
            assert!(matches!(f.modifiers[1], Modifier::RetryBudget(3)));
            assert!(matches!(f.modifiers[2], Modifier::Fallback(_)));
        }
    }

    #[test]
    fn test_struct() {
        let p = parse("struct Point { x: Number, y: Number }");
        if let Item::Struct(s) = &p.items[0] {
            assert_eq!(s.name, "Point");
            assert_eq!(s.fields.len(), 2);
        }
    }

    #[test]
    fn test_enum() {
        let p = parse("enum Status { Active, Inactive, Pending }");
        if let Item::Enum(e) = &p.items[0] {
            assert_eq!(e.name, "Status");
            assert_eq!(e.variants, vec!["Active", "Inactive", "Pending"]);
        }
    }

    #[test]
    fn test_match_expr() {
        let src = r#"
fn test() -> String {
    return match x {
        Ok(v) => v,
        Err(e) => "err",
        _ => "other"
    }
}
"#;
        let p = parse(src);
        if let Item::Fn(f) = &p.items[0] {
            if let Stmt::Return { value: Expr::Match { arms, .. }, .. } = &f.body[0] {
                assert_eq!(arms.len(), 3);
                assert!(matches!(arms[0].pattern, Pattern::Ctor(_, _)));
                assert!(matches!(arms[2].pattern, Pattern::Wildcard));
            } else {
                panic!("Очікувався match у return");
            }
        }
    }

    #[test]
    fn test_spawn() {
        let src = r#"
fn main() -> Number {
    let h = spawn fetchData("url")
    return 0
}
"#;
        let p = parse(src);
        if let Item::Fn(f) = &p.items[0] {
            if let Stmt::Let { value: Expr::Spawn { fn_name, .. }, .. } = &f.body[0] {
                assert_eq!(fn_name, "fetchData");
            } else {
                panic!("Очікувався spawn");
            }
        }
    }
}

/// Перетворює блок інструкцій на вираз.
/// Бере останній `return expr` або `expr` як значення блоку.
fn block_to_expr(stmts: Vec<ast::Stmt>, span: &ast::Span) -> Result<ast::Expr, ParseError> {
    for stmt in stmts.into_iter().rev() {
        match stmt {
            ast::Stmt::Return { value, .. } => return Ok(value),
            ast::Stmt::Expr(e)              => return Ok(e),
            ast::Stmt::Let { .. }           => continue,
            _                               => continue,
        }
    }
    Ok(ast::Expr::Bool(false, span.clone()))
}
