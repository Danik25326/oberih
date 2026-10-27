pub mod bytecode;
pub mod struct_table;

use std::collections::HashMap;
use crate::parser::ast::*;
use bytecode::*;
use struct_table::StructTable;

#[derive(Debug)]
pub struct CompileError {
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Помилка компіляції: {}", self.message)
    }
}

type CR<T> = Result<T, CompileError>;

fn err(msg: impl Into<String>) -> CompileError {
    CompileError { message: msg.into() }
}

/// Контекст компіляції однієї функції.
struct FnCtx {
    code:       Vec<Instr>,
    locals:     Vec<String>,  // ім'я → індекс = позиція в Vec
    resilience: ResilienceMeta,
}

impl FnCtx {
    fn new() -> Self {
        FnCtx {
            code:       Vec::new(),
            locals:     Vec::new(),
            resilience: ResilienceMeta::default(),
        }
    }

    fn emit(&mut self, instr: Instr) -> usize {
        let pos = self.code.len();
        self.code.push(instr);
        pos
    }

    /// Резервуємо місце для jump — повертаємо позицію для патчу.
    fn emit_jump_placeholder(&mut self, instr: Instr) -> usize {
        self.emit(instr)
    }

    fn patch_jump(&mut self, pos: usize) {
        let target = self.code.len();
        match &mut self.code[pos] {
            Instr::Jump(a) | Instr::JumpIfFalse(a) | Instr::JumpIfTrue(a) => *a = target,
            _ => panic!("patch_jump: не jump на позиції {}", pos),
        }
    }

    fn local_slot(&self, name: &str) -> Option<usize> {
        self.locals.iter().rposition(|n| n == name)
    }

    fn alloc_local(&mut self, name: &str) -> usize {
        let slot = self.locals.len();
        self.locals.push(name.to_string());
        slot
    }

    fn local_count(&self) -> usize {
        self.locals.len()
    }
}

pub struct Compiler {
    compiled:     Vec<CompiledFn>,
    struct_table: StructTable,
    enum_table:   HashMap<String, Vec<String>>,
    lambda_count: usize,
}

impl Compiler {
    pub fn new() -> Self {
        Compiler {
            compiled:     Vec::new(),
            struct_table: StructTable::new(),
            enum_table:   HashMap::new(),
            lambda_count: 0,
        }
    }

    /// Починає нумерацію лямбд з `base` (REPL: щоб імена не збігались між рядками).
    pub fn with_lambda_base(mut self, base: usize) -> Self {
        self.lambda_count = base;
        self
    }

    pub fn compile_program(mut self, program: &Program) -> CR<Module> {
        // Прохід 1: реєструємо всі struct і enum типи
        for item in &program.items {
            match item {
                Item::Struct(s) => {
                    let fields: Vec<String> = s.fields.iter().map(|f| f.name.clone()).collect();
                    self.struct_table.register(&s.name, fields);
                }
                Item::Enum(e) => {
                    self.enum_table.insert(e.name.clone(), e.variants.clone());
                }
                _ => {}
            }
        }
        // Прохід 2: компілюємо функції
        for item in &program.items {
            match item {
                Item::Fn(f)     => self.compile_fn(f)?,
                Item::Struct(_) => {}
                Item::Enum(_)   => {}
                Item::Import(_) => {}
            }
        }
        Ok(Module {
            functions:    self.compiled,
            struct_table: self.struct_table,
        })
    }

    /// Лямбда стає синтетичною функцією `__lambda_N(захоплені..., параметри...)`.
    /// Змінні охоплюючої функції, які лямбда використовує, копіюються в момент
    /// створення (`MakeClosure`). Без захоплень значення — звичайне посилання на функцію.
    fn compile_lambda(&mut self, params: &[LambdaParam], body: &LambdaBody, outer: &mut FnCtx) -> CR<()> {
        // 1. Які імена лямбда використовує і які з них — локальні змінні ОХОПЛЮЮЧОЇ функції?
        let mut used = Vec::new();
        match body {
            LambdaBody::Expr(e)   => crate::module_loader::collect_expr_refs(e, &mut used),
            LambdaBody::Block(bs) => for s in bs { crate::module_loader::collect_stmt_refs(s, &mut used); },
        }
        let mut captured: Vec<String> = Vec::new();
        for (name, _, _) in used {
            let is_param = params.iter().any(|p| p.name == name);
            if !is_param && outer.local_slot(&name).is_some() && !captured.contains(&name) {
                captured.push(name);
            }
        }

        // 2. Компілюємо тіло в окрему функцію: [захоплені..., параметри...]
        self.lambda_count += 1;
        let fn_name = format!("__lambda_{}", self.lambda_count);
        let mut ctx = FnCtx::new();
        for name in &captured { ctx.alloc_local(name); }
        for p in params { ctx.alloc_local(&p.name); }
        match body {
            LambdaBody::Expr(e) => {
                self.compile_expr(e, &mut ctx)?;
                ctx.emit(Instr::Return);
            }
            LambdaBody::Block(stmts) => {
                for s in stmts { self.compile_stmt(s, &mut ctx)?; }
                ctx.emit(Instr::PushNil);
                ctx.emit(Instr::Return);
            }
        }
        let local_count = ctx.local_count();
        self.compiled.push(CompiledFn {
            name: fn_name.clone(),
            code: ctx.code,
            local_count,
            resilience: ctx.resilience,
        });

        // 3. У охоплюючій функції: значення захоплених змінних -> замикання
        if captured.is_empty() {
            outer.emit(Instr::LoadGlobal(fn_name));
        } else {
            for name in &captured {
                let slot = outer.local_slot(name).expect("захоплена змінна має бути локальною");
                outer.emit(Instr::LoadLocal(slot));
            }
            outer.emit(Instr::MakeClosure(fn_name, captured.len()));
        }
        Ok(())
    }

    fn compile_fn(&mut self, f: &FnDecl) -> CR<()> {
        let mut ctx = FnCtx::new();

        // Параметри — перші локальні слоти
        for p in &f.params {
            ctx.alloc_local(&p.name);
        }

        // Resilience метадані
        self.compile_modifiers(&f.modifiers, &mut ctx)?;

        // Тіло
        for stmt in &f.body {
            self.compile_stmt(stmt, &mut ctx)?;
        }

        // Неявний return nil якщо функція не повертає явно
        ctx.emit(Instr::PushNil);
        ctx.emit(Instr::Return);

        let local_count = ctx.local_count();
        self.compiled.push(CompiledFn {
            name:        f.name.clone(),
            code:        ctx.code,
            local_count,
            resilience:  ctx.resilience,
        });
        Ok(())
    }

    fn compile_modifiers(&mut self, mods: &[Modifier], ctx: &mut FnCtx) -> CR<()> {
        use Modifier::*;
        for m in mods {
            match m {
                Deadline(d)         => ctx.resilience.deadline_secs = Some(d.to_secs()),
                RetryBudget(n)      => ctx.resilience.retry_budget   = Some(*n),
                Retries(n)          => ctx.resilience.retries         = Some(*n),
                Fallback(e)         => {
                    // Fallback — ім'я функції або рядковий літерал
                    if let Expr::Ident(name, _) = e.as_ref() {
                        ctx.resilience.fallback_fn = Some(name.clone());
                    }
                }
                Timeout(d)          => ctx.resilience.timeout_secs    = Some(d.to_secs()),
                CircuitBreaker { fail_threshold, cooldown } => {
                    ctx.resilience.circuit_breaker = Some(CircuitBreakerMeta {
                        fail_threshold: *fail_threshold,
                        cooldown_secs:  cooldown.to_secs(),
                    });
                }
                Idempotent { .. }   => ctx.resilience.is_idempotent   = true,
                Cache { ttl }       => ctx.resilience.cache_ttl_secs   = Some(ttl.to_secs()),
                EmergencyFallback(_) => {}   // TODO: схоже на Fallback
                RateLimit { n, per } => {
                    ctx.resilience.rate_limit = Some(RateLimitMeta {
                        n:        *n,
                        per_secs: per.to_secs(),
                    });
                }
                Bulkhead { max_concurrent } => ctx.resilience.bulkhead_max = Some(*max_concurrent),
                Hedging { after }   => ctx.resilience.hedging_after_secs = Some(after.to_secs()),
                Durable             => ctx.resilience.is_durable = true,
                Traced              => ctx.resilience.is_traced  = true,
                Budget { tokens, cost } => {
                    ctx.resilience.budget_tokens = *tokens;
                    ctx.resilience.budget_cost   = *cost;
                }
            }
        }
        Ok(())
    }

    // --- Stmt ---

    fn compile_stmt(&mut self, stmt: &Stmt, ctx: &mut FnCtx) -> CR<()> {
        match stmt {
            Stmt::Let { name, value, .. } => {
                self.compile_expr(value, ctx)?;
                let slot = ctx.alloc_local(name);
                ctx.emit(Instr::StoreLocal(slot));
            }

            Stmt::Return { value, .. } => {
                self.compile_expr(value, ctx)?;
                ctx.emit(Instr::Return);
            }

            Stmt::Assign { target, value, .. } => {
                self.compile_assign(target, value, ctx)?;
            }

            Stmt::If { cond, then_body, else_body, .. } => {
                self.compile_expr(cond, ctx)?;
                let jf = ctx.emit_jump_placeholder(Instr::JumpIfFalse(0));

                for s in then_body {
                    self.compile_stmt(s, ctx)?;
                }

                if let Some(else_b) = else_body {
                    let je = ctx.emit_jump_placeholder(Instr::Jump(0));
                    ctx.patch_jump(jf);
                    for s in else_b {
                        self.compile_stmt(s, ctx)?;
                    }
                    ctx.patch_jump(je);
                } else {
                    ctx.patch_jump(jf);
                }
            }

            Stmt::While { cond, body, .. } => {
                let loop_start = ctx.code.len();
                self.compile_expr(cond, ctx)?;
                let jf = ctx.emit_jump_placeholder(Instr::JumpIfFalse(0));
                for s in body {
                    self.compile_stmt(s, ctx)?;
                }
                ctx.emit(Instr::Jump(loop_start));
                ctx.patch_jump(jf);
            }

            Stmt::For { var, iter, body, .. } => {
                // for (x in list) -> ітерація по індексах
                // Компілюємо як:
                //   let __iter = iter
                //   let __i = 0
                //   while (__i < len(__iter)) { let x = __iter[__i]; body; __i = __i + 1 }
                // __iterable(iter): List -> як є, Map -> знімок ключів, String -> символи.
                ctx.emit(Instr::LoadGlobal("__iterable".into()));
                self.compile_expr(iter, ctx)?;
                ctx.emit(Instr::Call(1));
                let iter_slot = ctx.alloc_local("__iter");
                ctx.emit(Instr::StoreLocal(iter_slot));

                ctx.emit(Instr::PushNum(0.0));
                let i_slot = ctx.alloc_local("__i");
                ctx.emit(Instr::StoreLocal(i_slot));

                let loop_start = ctx.code.len();

                // умова: __i < len(__iter)
                ctx.emit(Instr::LoadLocal(i_slot));
                // Конвенція виклику: спочатку callee, потім аргументи.
                ctx.emit(Instr::LoadGlobal("len".into()));
                ctx.emit(Instr::LoadLocal(iter_slot));
                ctx.emit(Instr::Call(1));
                ctx.emit(Instr::Lt);
                let jf = ctx.emit_jump_placeholder(Instr::JumpIfFalse(0));

                // let x = __iter[__i]
                ctx.emit(Instr::LoadLocal(iter_slot));
                ctx.emit(Instr::LoadLocal(i_slot));
                ctx.emit(Instr::LoadIndex);
                let var_slot = ctx.alloc_local(var);
                ctx.emit(Instr::StoreLocal(var_slot));

                for s in body {
                    self.compile_stmt(s, ctx)?;
                }

                // __i = __i + 1
                ctx.emit(Instr::LoadLocal(i_slot));
                ctx.emit(Instr::PushNum(1.0));
                ctx.emit(Instr::Add);
                ctx.emit(Instr::StoreLocal(i_slot));

                ctx.emit(Instr::Jump(loop_start));
                ctx.patch_jump(jf);
            }

            Stmt::Expr(e) => {
                self.compile_expr(e, ctx)?;
                ctx.emit(Instr::Pop);  // результат не потрібен
            }
        }
        Ok(())
    }

    fn compile_assign(&mut self, target: &Expr, value: &Expr, ctx: &mut FnCtx) -> CR<()> {
        match target {
            Expr::Ident(name, _) => {
                self.compile_expr(value, ctx)?;
                let slot = ctx.local_slot(name)
                    .ok_or_else(|| err(format!("Невідома змінна '{}'", name)))?;
                ctx.emit(Instr::StoreLocal(slot));
            }
            Expr::Field { object, field, .. } => {
                // object.field = value
                // -> compile object (референція), compile value, StoreField
                self.compile_expr(object, ctx)?;
                self.compile_expr(value, ctx)?;
                ctx.emit(Instr::StoreField(field.clone()));
            }
            Expr::Index { object, index, .. } => {
                self.compile_expr(object, ctx)?;
                self.compile_expr(index, ctx)?;
                self.compile_expr(value, ctx)?;
                ctx.emit(Instr::StoreIndex);
            }
            _ => return Err(err("Невалідна ліва частина присвоєння")),
        }
        Ok(())
    }

    // --- Expr ---

    fn compile_expr(&mut self, expr: &Expr, ctx: &mut FnCtx) -> CR<()> {
        match expr {
            Expr::Number(n, _)    => { ctx.emit(Instr::PushNum(*n)); }
            Expr::StringLit(s, _) => { ctx.emit(Instr::PushStr(s.clone())); }
            Expr::Bool(b, _)      => { ctx.emit(Instr::PushBool(*b)); }

            Expr::Ident(name, _) => {
                if let Some(slot) = ctx.local_slot(name) {
                    ctx.emit(Instr::LoadLocal(slot));
                } else {
                    ctx.emit(Instr::LoadGlobal(name.clone()));
                }
            }

            Expr::ResultCtor { variant, value, .. } => {
                self.compile_expr(value, ctx)?;
                match variant {
                    ResultVariant::Ok  => ctx.emit(Instr::MakeOk),
                    ResultVariant::Err => ctx.emit(Instr::MakeErr),
                };
            }

            Expr::BinOp { op, left, right, .. } => {
                self.compile_expr(left, ctx)?;
                self.compile_expr(right, ctx)?;
                let instr = match op {
                    BinOp::Add   => Instr::Add,
                    BinOp::Sub   => Instr::Sub,
                    BinOp::Mul   => Instr::Mul,
                    BinOp::Div   => Instr::Div,
                    BinOp::Eq    => Instr::Eq,
                    BinOp::NotEq => Instr::NotEq,
                    BinOp::Lt    => Instr::Lt,
                    BinOp::Gt    => Instr::Gt,
                    BinOp::LtEq  => Instr::LtEq,
                    BinOp::GtEq  => Instr::GtEq,
                    BinOp::And   => Instr::And,
                    BinOp::Or    => Instr::Or,
                    BinOp::BitAnd => Instr::BitAnd,
                    BinOp::BitOr  => Instr::BitOr,
                    BinOp::BitXor => Instr::BitXor,
                    BinOp::Shl    => Instr::Shl,
                    BinOp::Shr    => Instr::Shr,
                };
                ctx.emit(instr);
            }

            Expr::Neg { expr, .. } => {
                self.compile_expr(expr, ctx)?;
                ctx.emit(Instr::Neg);
            }

            Expr::BitNot { expr, .. } => {
                self.compile_expr(expr, ctx)?;
                ctx.emit(Instr::BitNot);
            }

            Expr::Not { expr, .. } => {
                self.compile_expr(expr, ctx)?;
                ctx.emit(Instr::Not);
            }

            Expr::Try { expr, .. } => {
                self.compile_expr(expr, ctx)?;
                ctx.emit(Instr::TryUnwrap);
            }

            Expr::Lambda { params, body, .. } => {
                self.compile_lambda(params, body, ctx)?;
            }

            Expr::Field { object, field, .. } => {
                // Enum.Variant — ім'я зліва це enum тип, а не змінна.
                // Локальна змінна з тим самим ім'ям має пріоритет (тінюємо enum).
                if let Expr::Ident(name, _) = object.as_ref() {
                    if ctx.local_slot(name).is_none() {
                        if let Some(variants) = self.enum_table.get(name.as_str()) {
                            if !variants.iter().any(|v| v == field) {
                                return Err(err(format!(
                                    "Enum '{}' не має варіанту '{}'",
                                    name, field
                                )));
                            }
                            ctx.emit(Instr::PushEnum(name.clone(), field.clone()));
                            return Ok(());
                        }
                    }
                }
                self.compile_expr(object, ctx)?;
                ctx.emit(Instr::LoadField(field.clone()));
            }

            Expr::Index { object, index, .. } => {
                self.compile_expr(object, ctx)?;
                self.compile_expr(index, ctx)?;
                ctx.emit(Instr::LoadIndex);
            }

            Expr::Call { callee, args, .. } => {
                // Перевіряємо чи це конструктор struct
                if let Expr::Ident(name, _) = callee.as_ref() {
                    if let Some(field_names) = self.struct_table.fields(name).cloned() {
                        if field_names.len() != args.len() {
                            return Err(err(format!(
                                "Struct '{}' потребує {} полів, передано {}",
                                name, field_names.len(), args.len()
                            )));
                        }
                        for a in args {
                            self.compile_expr(a, ctx)?;
                        }
                        ctx.emit(Instr::MakeStruct {
                            name:        name.clone(),
                            field_names: field_names.clone(),
                        });
                        return Ok(());
                    }
                }
                // Звичайний виклик функції
                self.compile_expr(callee, ctx)?;
                let n = args.len();
                for a in args {
                    self.compile_expr(a, ctx)?;
                }
                ctx.emit(Instr::Call(n));
            }

            Expr::MethodCall { object, method, args, .. } => {
                self.compile_expr(object, ctx)?;
                let n = args.len();
                for a in args {
                    self.compile_expr(a, ctx)?;
                }
                ctx.emit(Instr::CallMethod { name: method.clone(), arg_count: n });
            }

            Expr::Match { scrutinee, arms, .. } => {
                self.compile_match(scrutinee, arms, ctx)?;
            }

            Expr::Spawn { fn_name, args, .. } => {
                let n = args.len();
                for a in args {
                    self.compile_expr(a, ctx)?;
                }
                ctx.emit(Instr::Spawn { fn_name: fn_name.clone(), arg_count: n });
            }

            Expr::List(elems, _) => {
                let n = elems.len();
                for e in elems {
                    self.compile_expr(e, ctx)?;
                }
                ctx.emit(Instr::MakeList(n));
            }

            Expr::Map(entries, _) => {
                for (k, v) in entries {
                    self.compile_expr(k, ctx)?;
                    self.compile_expr(v, ctx)?;
                }
                ctx.emit(Instr::MakeMap(entries.len()));
            }
        }
        Ok(())
    }

    fn compile_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], ctx: &mut FnCtx) -> CR<()> {
        // Алгоритм:
        //   compile scrutinee
        //   для кожного arm:
        //     Dup scrutinee
        //     compile pattern check -> bool на стеку
        //     JumpIfFalse next_arm
        //     [якщо Ctor: UnwrapVariantInto(slot)]
        //     compile body
        //     Jump end
        //   next_arm:
        //   ...
        //   end: Pop (scrutinee що залишився)

        self.compile_expr(scrutinee, ctx)?;

        let mut end_jumps: Vec<usize> = Vec::new();

        for arm in arms {
            // Дублюємо scrutinee для перевірки
            ctx.emit(Instr::Dup);

            let skip = match &arm.pattern {
                Pattern::Wildcard => {
                    // завжди match — не потрібна перевірка
                    ctx.emit(Instr::Pop); // pop dup
                    None
                }
                Pattern::Literal(lit) => {
                    match lit {
                        LiteralPat::Number(n) => ctx.emit(Instr::MatchLitNum(*n)),
                        LiteralPat::Str(s)    => ctx.emit(Instr::MatchLitStr(s.clone())),
                        LiteralPat::Bool(b)   => ctx.emit(Instr::MatchLitBool(*b)),
                    };
                    let jf = ctx.emit_jump_placeholder(Instr::JumpIfFalse(0));
                    Some(jf)
                }
                Pattern::Variant(name) => {
                    ctx.emit(Instr::MatchVariant(name.clone()));
                    let jf = ctx.emit_jump_placeholder(Instr::JumpIfFalse(0));
                    Some(jf)
                }
                Pattern::Ctor(ctor, bind) => {
                    ctx.emit(Instr::MatchVariant(ctor.clone()));
                    let jf = ctx.emit_jump_placeholder(Instr::JumpIfFalse(0));
                    // Розпаковуємо значення в локальну змінну
                    let slot = ctx.alloc_local(bind);
                    ctx.emit(Instr::UnwrapVariantInto(slot));
                    Some(jf)
                }
            };

            // Pop dup для Ctor вже зроблено через UnwrapVariantInto
            // Для решти — dup вже поп'нуто match-інструкцією або явно

            // Тіло arm
            self.compile_expr(&arm.body, ctx)?;
            let je = ctx.emit_jump_placeholder(Instr::Jump(0));
            end_jumps.push(je);

            if let Some(jf) = skip {
                ctx.patch_jump(jf);
            }
        }

        // Pop scrutinee що залишився якщо жоден arm не спрацював
        ctx.emit(Instr::PushNil); // результат якщо нічого не match

        // Патчимо всі end_jumps
        for je in end_jumps {
            ctx.patch_jump(je);
        }

        // Pop scrutinee
        ctx.emit(Instr::Swap);
        ctx.emit(Instr::Pop);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn compile(src: &str) -> Module {
        let tokens = Lexer::new(src).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        Compiler::new().compile_program(&program).unwrap()
    }

    #[test]
    fn test_compile_simple_fn() {
        let m = compile("fn add(a: Number, b: Number) -> Number { return a }");
        let f = m.find_fn("add").unwrap();
        assert!(!f.code.is_empty());
        // перші два слоти — параметри
        assert!(f.local_count >= 2);
    }

    #[test]
    fn test_compile_resilience_meta() {
        let src = r#"
resilient fn getOrder(id: String) -> String
    deadline(2s)
    retryBudget(3)
    timeout(500ms)
{
    return id
}
"#;
        let m = compile(src);
        let f = m.find_fn("getOrder").unwrap();
        assert_eq!(f.resilience.deadline_secs, Some(2.0));
        assert_eq!(f.resilience.retry_budget,  Some(3));
        assert_eq!(f.resilience.timeout_secs,  Some(0.5));
    }

    #[test]
    fn test_compile_if() {
        let src = r#"
fn check(x: Number) -> Number {
    if (x > 0) {
        return x
    } else {
        return 0
    }
}
"#;
        let m = compile(src);
        let f = m.find_fn("check").unwrap();
        // має містити JumpIfFalse і Jump
        let has_jf = f.code.iter().any(|i| matches!(i, Instr::JumpIfFalse(_)));
        let has_j  = f.code.iter().any(|i| matches!(i, Instr::Jump(_)));
        assert!(has_jf, "if має генерувати JumpIfFalse");
        assert!(has_j,  "else має генерувати Jump");
    }

    #[test]
    fn test_compile_result_ctor() {
        let src = "fn wrap(x: Number) -> String { return Ok(x) }";
        let m = compile(src);
        let f = m.find_fn("wrap").unwrap();
        let has_make_ok = f.code.iter().any(|i| matches!(i, Instr::MakeOk));
        assert!(has_make_ok, "Ok(x) має генерувати MakeOk");
    }

    #[test]
    fn test_compile_spawn() {
        let src = r#"
fn main() -> Number {
    let h = spawn fetchData("url")
    return 0
}
fn fetchData(url: String) -> String { return url }
"#;
        let m = compile(src);
        let f = m.find_fn("main").unwrap();
        let has_spawn = f.code.iter().any(|i| matches!(i, Instr::Spawn { .. }));
        assert!(has_spawn, "spawn має генерувати Instr::Spawn");
    }
}
