/// Typechecker Oberih.
/// Перевіряє програму до виконання і дає людські повідомлення про помилки.

use std::collections::HashMap;
use crate::parser::ast::*;

#[derive(Debug, Clone)]
pub struct TypeError {
    pub message: String,
    pub line:    usize,
    pub col:     usize,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Помилка типу на {}:{}: {}", self.line, self.col, self.message)
    }
}

type TR<T> = Result<T, Vec<TypeError>>;

// ---------------------------------------------------------------------------
// Типи в typechecker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Number,
    Str,
    Bool,
    Nil,
    Result(Box<Ty>, Box<Ty>),
    List(Box<Ty>),
    Struct(String),
    Fn(Vec<Ty>, Box<Ty>),
    Generic(String),   // T, U — незв'язаний параметр
    Unknown,           // для виразів що не вдалось вивести
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Ty::Number       => write!(f, "Number"),
            Ty::Str          => write!(f, "String"),
            Ty::Bool         => write!(f, "Bool"),
            Ty::Nil          => write!(f, "Nil"),
            Ty::Result(ok, err) => write!(f, "Result<{}, {}>", ok, err),
            Ty::List(t)      => write!(f, "List<{}>", t),
            Ty::Struct(n)    => write!(f, "{}", n),
            Ty::Fn(ps, r)    => {
                let ps: Vec<String> = ps.iter().map(|p| p.to_string()).collect();
                write!(f, "fn({}) -> {}", ps.join(", "), r)
            }
            Ty::Generic(n)   => write!(f, "{}", n),
            Ty::Unknown      => write!(f, "?"),
        }
    }
}

fn ty_from_ast(te: &TypeExpr) -> Ty {
    match te {
        TypeExpr::Simple(n) => match n.as_str() {
            "Number" => Ty::Number,
            "String" => Ty::Str,
            "Bool"   => Ty::Bool,
            "Nil"    => Ty::Nil,
            n if n.len() == 1 && n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) => {
                Ty::Generic(n.to_string())
            }
            n        => Ty::Struct(n.to_string()),
        },
        TypeExpr::Generic(name, args) => match name.as_str() {
            "Result" => {
                let ok  = args.get(0).map(ty_from_ast).unwrap_or(Ty::Unknown);
                let err = args.get(1).map(ty_from_ast).unwrap_or(Ty::Unknown);
                Ty::Result(Box::new(ok), Box::new(err))
            }
            "List" => {
                let inner = args.get(0).map(ty_from_ast).unwrap_or(Ty::Unknown);
                Ty::List(Box::new(inner))
            }
            n => Ty::Struct(n.to_string()),
        },
    }
}

// ---------------------------------------------------------------------------
// Середовище типів
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct TyEnv {
    scopes: Vec<HashMap<String, Ty>>,
}

impl TyEnv {
    fn new() -> Self {
        TyEnv { scopes: vec![HashMap::new()] }
    }

    fn push(&mut self) { self.scopes.push(HashMap::new()); }
    fn pop(&mut self)  { self.scopes.pop(); }

    fn define(&mut self, name: &str, ty: Ty) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn lookup(&self, name: &str) -> Option<&Ty> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Typechecker
// ---------------------------------------------------------------------------

pub struct Typechecker {
    fn_sigs:        HashMap<String, (Vec<Ty>, Ty)>,
    struct_fields:  HashMap<String, HashMap<String, Ty>>,
    enum_variants:  HashMap<String, Vec<String>>,
    /// struct_type_params: struct name -> [param names]
    struct_type_params: HashMap<String, Vec<String>>,
    /// var_generic_map: variable name -> {T -> concrete type}
    var_generic_map: HashMap<String, HashMap<String, Ty>>,
    errors:         Vec<TypeError>,
}

impl Typechecker {
    pub fn new() -> Self {
        Typechecker {
            fn_sigs:            HashMap::new(),
            struct_fields:      HashMap::new(),
            enum_variants:      HashMap::new(),
            struct_type_params: HashMap::new(),
            var_generic_map:    HashMap::new(),
            errors:             Vec::new(),
        }
    }

    fn error(&mut self, msg: impl Into<String>, span: &Span) {
        self.errors.push(TypeError {
            message: msg.into(),
            line:    span.line,
            col:     span.col,
        });
    }

    pub fn check(mut self, program: &Program) -> TR<()> {
        // Прохід 1: реєструємо сигнатури
        for item in &program.items {
            match item {
                Item::Fn(f) => {
                    let params: Vec<Ty> = f.params.iter().map(|p| ty_from_ast(&p.ty)).collect();
                    let ret = ty_from_ast(&f.return_type);
                    self.fn_sigs.insert(f.name.clone(), (params, ret));
                }
                Item::Struct(s) => {
                    let mut fields = HashMap::new();
                    for field in &s.fields {
                        fields.insert(field.name.clone(), ty_from_ast(&field.ty));
                    }
                    self.struct_fields.insert(s.name.clone(), fields);
                    // Реєструємо generic параметри struct
                    if !s.type_params.is_empty() {
                        self.struct_type_params.insert(s.name.clone(), s.type_params.clone());
                    }
                }
                Item::Enum(e) => {
                    self.enum_variants.insert(e.name.clone(), e.variants.clone());
                }
                Item::Import(_) => {}
            }
        }

        // Прохід 2: перевіряємо тіла функцій
        for item in &program.items {
            if let Item::Fn(f) = item {
                self.check_fn(f);
            }
        }

        // Перевірка resilience: resilient fn повинна мати deadline
        for item in &program.items {
            if let Item::Fn(f) = item {
                if f.is_resilient {
                    let has_deadline = f.modifiers.iter().any(|m| matches!(m, Modifier::Deadline(_)));
                    if !has_deadline {
                        self.errors.push(TypeError {
                            message: format!(
                                "resilient fn '{}': відсутній deadline. \
                                 Без deadline функція може висіти нескінченно.",
                                f.name
                            ),
                            line: f.span.line,
                            col:  f.span.col,
                        });
                    }
                }
            }
        }

        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors)
        }
    }

    fn check_fn(&mut self, f: &FnDecl) {
        let mut env = TyEnv::new();

        // Параметри в scope
        for p in &f.params {
            env.define(&p.name, ty_from_ast(&p.ty));
        }

        let expected_ret = ty_from_ast(&f.return_type);
        self.check_block(&f.body, &mut env, &expected_ret, &f.span);
    }

    fn check_block(&mut self, block: &Block, env: &mut TyEnv, ret_ty: &Ty, _span: &Span) {
        env.push();
        for stmt in block {
            self.check_stmt(stmt, env, ret_ty);
        }
        env.pop();
    }

    fn check_stmt(&mut self, stmt: &Stmt, env: &mut TyEnv, ret_ty: &Ty) {
        match stmt {
            Stmt::Let { name, value, span: _ } => {
                let ty = self.infer_expr(value, env);
                // Якщо це generic struct — зберігаємо прив'язку T -> конкретний тип
                if let Expr::Call { callee, args, .. } = value {
                    if let Expr::Ident(struct_name, _) = callee.as_ref() {
                        if let Some(type_params) = self.struct_type_params.get(struct_name.as_str()).cloned() {
                            let mut generic_map = HashMap::new();
                            for (param, arg) in type_params.iter().zip(args.iter()) {
                                let arg_ty = self.infer_expr(arg, env);
                                generic_map.insert(param.clone(), arg_ty);
                            }
                            self.var_generic_map.insert(name.clone(), generic_map);
                        }
                    }
                }
                env.define(name, ty);
            }

            Stmt::Return { value, span } => {
                let ty = self.infer_expr(value, env);
                if !self.types_compatible(&ty, ret_ty) {
                    self.errors.push(TypeError {
                        message: format!(
                            "return: очікується {}, отримано {}",
                            ret_ty, ty
                        ),
                        line: span.line,
                        col:  span.col,
                    });
                }
            }

            Stmt::If { cond, then_body, else_body, span } => {
                let cond_ty = self.infer_expr(cond, env);
                if !matches!(cond_ty, Ty::Bool | Ty::Unknown) {
                    self.errors.push(TypeError {
                        message: format!("умова if має бути Bool, отримано {}", cond_ty),
                        line: span.line,
                        col:  span.col,
                    });
                }
                self.check_block(then_body, env, ret_ty, span);
                if let Some(else_b) = else_body {
                    self.check_block(else_b, env, ret_ty, span);
                }
            }

            Stmt::While { cond, body, span } => {
                let cond_ty = self.infer_expr(cond, env);
                if !matches!(cond_ty, Ty::Bool | Ty::Unknown) {
                    self.errors.push(TypeError {
                        message: format!("умова while має бути Bool, отримано {}", cond_ty),
                        line: span.line,
                        col:  span.col,
                    });
                }
                self.check_block(body, env, ret_ty, span);
            }

            Stmt::For { var, iter, body, span } => {
                let iter_ty = self.infer_expr(iter, env);
                let elem_ty = match &iter_ty {
                    Ty::List(inner) => *inner.clone(),
                    Ty::Unknown     => Ty::Unknown,
                    other           => {
                        self.errors.push(TypeError {
                            message: format!("for: ітерувати можна тільки List, отримано {}", other),
                            line: span.line,
                            col:  span.col,
                        });
                        Ty::Unknown
                    }
                };
                env.push();
                env.define(var, elem_ty);
                self.check_block(body, env, ret_ty, span);
                env.pop();
            }

            Stmt::Assign { target, value, span } => {
                let val_ty = self.infer_expr(value, env);
                let tgt_ty = self.infer_expr(target, env);
                if !self.types_compatible(&val_ty, &tgt_ty) && !matches!(tgt_ty, Ty::Unknown) {
                    self.errors.push(TypeError {
                        message: format!(
                            "присвоєння: несумісні типи {} і {}",
                            tgt_ty, val_ty
                        ),
                        line: span.line,
                        col:  span.col,
                    });
                }
            }

            Stmt::Expr(e) => { self.infer_expr(e, env); }
        }
    }

    fn infer_expr(&mut self, expr: &Expr, env: &TyEnv) -> Ty {
        match expr {
            Expr::Number(_, _)    => Ty::Number,
            Expr::StringLit(_, _) => Ty::Str,
            Expr::Bool(_, _)      => Ty::Bool,

            Expr::Ident(name, _span) => {
                if let Some(ty) = env.lookup(name) {
                    ty.clone()
                } else if let Some((params, ret)) = self.fn_sigs.get(name).cloned() {
                    Ty::Fn(params, Box::new(ret))
                } else if self.struct_fields.contains_key(name.as_str()) {
                    // Конструктор struct
                    Ty::Struct(name.clone())
                } else {
                    // Не помилка — може бути enum варіант або вбудована
                    Ty::Unknown
                }
            }

            Expr::ResultCtor { variant, value, .. } => {
                let inner = self.infer_expr(value, env);
                match variant {
                    ResultVariant::Ok  => Ty::Result(Box::new(inner), Box::new(Ty::Unknown)),
                    ResultVariant::Err => Ty::Result(Box::new(Ty::Unknown), Box::new(inner)),
                }
            }

            Expr::BinOp { op, left, right, span } => {
                let l = self.infer_expr(left, env);
                let r = self.infer_expr(right, env);
                match op {
                    BinOp::Add => {
                        match (&l, &r) {
                            (Ty::Number, Ty::Number) => Ty::Number,
                            (Ty::Str, _) | (_, Ty::Str) => Ty::Str,
                            (Ty::Unknown, _) | (_, Ty::Unknown) => Ty::Unknown,
                            _ => {
                                self.errors.push(TypeError {
                                    message: format!("+ не підтримується між {} і {}", l, r),
                                    line: span.line, col: span.col,
                                });
                                Ty::Unknown
                            }
                        }
                    }
                    BinOp::Sub | BinOp::Mul | BinOp::Div => {
                        if !matches!((&l, &r), (Ty::Number, Ty::Number) | (Ty::Unknown, _) | (_, Ty::Unknown)) {
                            self.errors.push(TypeError {
                                message: format!("арифметика тільки для Number, отримано {} і {}", l, r),
                                line: span.line, col: span.col,
                            });
                        }
                        Ty::Number
                    }
                    BinOp::Eq | BinOp::NotEq => Ty::Bool,
                    BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                        if !matches!((&l, &r), (Ty::Number, Ty::Number) | (Ty::Unknown, _) | (_, Ty::Unknown)) {
                            self.errors.push(TypeError {
                                message: format!("порівняння тільки для Number, отримано {} і {}", l, r),
                                line: span.line, col: span.col,
                            });
                        }
                        Ty::Bool
                    }
                }
            }

            Expr::Neg { expr, span } => {
                let t = self.infer_expr(expr, env);
                if !matches!(t, Ty::Number | Ty::Unknown) {
                    self.errors.push(TypeError {
                        message: format!("унарний мінус тільки для Number, отримано {}", t),
                        line: span.line, col: span.col,
                    });
                }
                Ty::Number
            }

            Expr::Try { expr, span } => {
                let t = self.infer_expr(expr, env);
                match t {
                    Ty::Result(ok, _) => *ok,
                    Ty::Unknown       => Ty::Unknown,
                    other => {
                        self.errors.push(TypeError {
                            message: format!("? можна застосувати тільки до Result<T,E>, отримано {}", other),
                            line: span.line, col: span.col,
                        });
                        Ty::Unknown
                    }
                }
            }

            Expr::Field { object, field, span } => {
                let obj_ty = self.infer_expr(object, env);
                let var_name = if let Expr::Ident(n, _) = object.as_ref() {
                    Some(n.clone())
                } else { None };
                match &obj_ty {
                    Ty::Struct(name) => {
                        if let Some(fields) = self.struct_fields.get(name.as_str()) {
                            let field_ty = fields.get(field.as_str()).cloned().unwrap_or_else(|| {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "Struct '{}' не має поля '{}'",
                                        name, field
                                    ),
                                    line: span.line, col: span.col,
                                });
                                Ty::Unknown
                            });
                            if let Ty::Generic(ref param) = field_ty {
                                if let Some(ref var) = var_name {
                                    if let Some(gmap) = self.var_generic_map.get(var.as_str()) {
                                        if let Some(concrete) = gmap.get(param.as_str()) {
                                            return concrete.clone();
                                        }
                                    }
                                }
                            }
                            field_ty
                        } else {
                            Ty::Unknown
                        }
                    }
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.errors.push(TypeError {
                            message: format!("доступ до поля '{}' на не-struct {}", field, other),
                            line: span.line, col: span.col,
                        });
                        Ty::Unknown
                    }
                }
            }

            Expr::Call { callee, args, span } => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    // Конструктор struct
                    if self.struct_fields.contains_key(name.as_str()) {
                        let expected = self.struct_fields.get(name.as_str())
                            .map(|f| f.len()).unwrap_or(0);
                        if args.len() != expected {
                            self.errors.push(TypeError {
                                message: format!(
                                    "Struct '{}': очікується {} аргументів, передано {}",
                                    name, expected, args.len()
                                ),
                                line: span.line, col: span.col,
                            });
                        }
                        return Ty::Struct(name.clone());
                    }
                    // Звичайна функція
                    if let Some((param_tys, ret_ty)) = self.fn_sigs.get(name.as_str()).cloned() {
                        if args.len() != param_tys.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "fn '{}': очікується {} аргументів, передано {}",
                                    name, param_tys.len(), args.len()
                                ),
                                line: span.line, col: span.col,
                            });
                        }
                        for (arg, expected) in args.iter().zip(param_tys.iter()) {
                            let arg_ty = self.infer_expr(arg, env);
                            if !self.types_compatible(&arg_ty, expected) {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "fn '{}': аргумент має тип {}, очікується {}",
                                        name, arg_ty, expected
                                    ),
                                    line: span.line, col: span.col,
                                });
                            }
                        }
                        return ret_ty;
                    }
                }
                for a in args { self.infer_expr(a, env); }
                Ty::Unknown
            }

            Expr::MethodCall { object, method, args, span: _ } => {
                let obj_ty = self.infer_expr(object, env);
                for a in args { self.infer_expr(a, env); }
                match &obj_ty {
                    Ty::Struct(name) => {
                        let method_name = format!("{}.{}", name, method);
                        if let Some((_, ret)) = self.fn_sigs.get(&method_name).cloned() {
                            ret
                        } else {
                            Ty::Unknown
                        }
                    }
                    Ty::Str => match method.as_str() {
                        "len"        => Ty::Number,
                        "trim"       => Ty::Str,
                        "toUpper"    => Ty::Str,
                        "toLower"    => Ty::Str,
                        "contains"   => Ty::Bool,
                        "startsWith" => Ty::Bool,
                        "split"      => Ty::List(Box::new(Ty::Str)),
                        _ => Ty::Unknown,
                    },
                    Ty::List(_) => match method.as_str() {
                        "len"   => Ty::Number,
                        "push"  => obj_ty.clone(),
                        "pop"   => Ty::Unknown,
                        "first" => Ty::Unknown,
                        "last"  => Ty::Unknown,
                        _ => Ty::Unknown,
                    },
                    _ => Ty::Unknown,
                }
            }

            Expr::Match { scrutinee, arms, .. } => {
                self.infer_expr(scrutinee, env);
                let mut result_ty = Ty::Unknown;
                for arm in arms {
                    // Додаємо bind змінну якщо є
                    let mut arm_env = env.clone();
                    if let Pattern::Ctor(_, bind) = &arm.pattern {
                        arm_env.define(bind, Ty::Unknown);
                    }
                    let arm_ty = self.infer_expr(&arm.body, &arm_env);
                    if matches!(result_ty, Ty::Unknown) {
                        result_ty = arm_ty;
                    }
                }
                result_ty
            }

            Expr::Spawn { fn_name, args, span } => {
                if let Some((param_tys, _)) = self.fn_sigs.get(fn_name.as_str()).cloned() {
                    if args.len() != param_tys.len() {
                        self.errors.push(TypeError {
                            message: format!(
                                "spawn {}: очікується {} аргументів, передано {}",
                                fn_name, param_tys.len(), args.len()
                            ),
                            line: span.line, col: span.col,
                        });
                    }
                }
                for a in args { self.infer_expr(a, env); }
                Ty::Unknown // SpawnHandle — поки Unknown
            }

            Expr::Index { object, index, span } => {
                let obj_ty = self.infer_expr(object, env);
                let idx_ty = self.infer_expr(index, env);
                if !matches!(idx_ty, Ty::Number | Ty::Unknown) {
                    self.errors.push(TypeError {
                        message: format!("індекс має бути Number, отримано {}", idx_ty),
                        line: span.line, col: span.col,
                    });
                }
                match obj_ty {
                    Ty::List(inner) => *inner,
                    _               => Ty::Unknown,
                }
            }

            Expr::List(elems, _) => {
                let inner = elems.first().map(|e| self.infer_expr(e, env)).unwrap_or(Ty::Unknown);
                for e in elems.iter().skip(1) { self.infer_expr(e, env); }
                Ty::List(Box::new(inner))
            }

            Expr::Map(entries, _) => {
                for (k, v) in entries {
                    self.infer_expr(k, env);
                    self.infer_expr(v, env);
                }
                Ty::Unknown
            }
        }
    }

    fn types_compatible(&self, got: &Ty, expected: &Ty) -> bool {
        if matches!(got, Ty::Unknown) || matches!(expected, Ty::Unknown) {
            return true;
        }
        if matches!(got, Ty::Generic(_)) || matches!(expected, Ty::Generic(_)) {
            return true;
        }
        match (got, expected) {
            (Ty::Number, Ty::Number) => true,
            (Ty::Str,    Ty::Str)    => true,
            (Ty::Bool,   Ty::Bool)   => true,
            (Ty::Nil,    Ty::Nil)    => true,
            (Ty::Struct(a), Ty::Struct(b)) => a == b,
            (Ty::List(a),   Ty::List(b))   => self.types_compatible(a, b),
            (Ty::Result(ok1, err1), Ty::Result(ok2, err2)) => {
                self.types_compatible(ok1, ok2) && self.types_compatible(err1, err2)
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn check(src: &str) -> Result<(), Vec<TypeError>> {
        let tokens  = Lexer::new(src).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        Typechecker::new().check(&program)
    }

    #[test]
    fn test_valid_fn() {
        assert!(check("fn add(a: Number, b: Number) -> Number { return a }").is_ok());
    }

    #[test]
    fn test_missing_deadline() {
        let src = r#"
resilient fn getOrder(id: String) -> String
    retryBudget(3)
{
    return id
}
"#;
        let errs = check(src).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("deadline")));
    }

    #[test]
    fn test_wrong_arg_count() {
        let src = r#"
fn add(a: Number, b: Number) -> Number { return a }
fn main() -> Number { return add(1) }
"#;
        let errs = check(src).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("аргументів")));
    }

    #[test]
    fn test_struct_missing_field() {
        let src = r#"
struct Point { x: Number, y: Number }
fn test() -> Number {
    let p = Point(1, 2)
    return p.z
}
"#;
        let errs = check(src).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("поля")));
    }

    #[test]
    fn test_try_on_non_result() {
        let src = r#"
fn bad(x: Number) -> Number {
    return x?
}
"#;
        let errs = check(src).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("Result")));
    }
}
