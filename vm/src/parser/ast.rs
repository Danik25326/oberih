/// AST Oberih.
/// Кожен вузол несе Span для діагностики.

#[derive(Debug, Clone)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

// ---------------------------------------------------------------------------
// Програма
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Import(ImportDecl),
    Fn(FnDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub path: String,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Функція
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub is_private:   bool,
    pub is_resilient: bool,
    pub name:         String,
    pub method_of:    Option<String>,   // fn Point.distance -> Some("Point")
    pub type_params:  Vec<String>,      // generics <T, U>
    pub params:       Vec<Param>,
    pub return_type:  TypeExpr,
    pub modifiers:    Vec<Modifier>,
    pub body:         Block,
    pub span:         Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty:   TypeExpr,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Resilience модифікатори
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Modifier {
    Deadline(Duration),
    RetryBudget(u32),
    Retries(u32),
    Fallback(Box<Expr>),
    Timeout(Duration),
    CircuitBreaker { fail_threshold: u32, cooldown: Duration },
    Idempotent { key: Box<Expr> },
    Cache { ttl: Duration },
    EmergencyFallback(Box<Expr>),
    RateLimit { n: u32, per: Duration },
    Bulkhead { max_concurrent: u32 },
    Hedging { after: Duration },
    Durable,
    Traced,
    Budget { tokens: Option<f64>, cost: Option<f64> },
}

#[derive(Debug, Clone)]
pub struct Duration {
    pub value: f64,
    pub unit:  TimeUnit,
}

impl Duration {
    pub fn to_secs(&self) -> f64 {
        match self.unit {
            TimeUnit::Ms => self.value / 1000.0,
            TimeUnit::S  => self.value,
            TimeUnit::M  => self.value * 60.0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TimeUnit { Ms, S, M }

// ---------------------------------------------------------------------------
// Struct / Enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub is_private:  bool,
    pub name:        String,
    pub type_params: Vec<String>,
    pub fields:      Vec<StructField>,
    pub span:        Span,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub ty:   TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name:     String,
    pub variants: Vec<String>,
    pub span:     Span,
}

// ---------------------------------------------------------------------------
// Типи
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum TypeExpr {
    Simple(String),
    Generic(String, Vec<TypeExpr>),  // Result<T, E>, List<T>
}

impl TypeExpr {
    pub fn name(&self) -> &str {
        match self {
            TypeExpr::Simple(n)     => n,
            TypeExpr::Generic(n, _) => n,
        }
    }
}

// ---------------------------------------------------------------------------
// Блок і інструкції
// ---------------------------------------------------------------------------

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name:  String,
        value: Expr,
        span:  Span,
    },
    Return {
        value: Expr,
        span:  Span,
    },
    If {
        cond:      Expr,
        then_body: Block,
        else_body: Option<Block>,
        span:      Span,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    For {
        var:  String,
        iter: Expr,
        body: Block,
        span: Span,
    },
    Expr(Expr),  // виклик або присвоєння як інструкція
    Assign {
        target: Expr,  // lvalue: x, x.f, x[i]
        value:  Expr,
        span:   Span,
    },
}

// ---------------------------------------------------------------------------
// Вирази
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Expr {
    // Літерали
    Number(f64, Span),
    StringLit(String, Span),
    Bool(bool, Span),

    // Змінна або ім'я функції
    Ident(String, Span),

    // Ok(...) / Err(...)
    ResultCtor { variant: ResultVariant, value: Box<Expr>, span: Span },

    // Арифметика і порівняння
    BinOp { op: BinOp, left: Box<Expr>, right: Box<Expr>, span: Span },

    // Унарний мінус
    Neg { expr: Box<Expr>, span: Span },

    // ? оператор
    Try { expr: Box<Expr>, span: Span },

    // Виклик функції: name(args)
    Call { callee: Box<Expr>, args: Vec<Expr>, span: Span },

    // Доступ до поля: expr.field
    Field { object: Box<Expr>, field: String, span: Span },

    // Виклик методу: expr.method(args)
    MethodCall { object: Box<Expr>, method: String, args: Vec<Expr>, span: Span },

    // Індекс: expr[i]
    Index { object: Box<Expr>, index: Box<Expr>, span: Span },

    // match expr { ... }
    Match { scrutinee: Box<Expr>, arms: Vec<MatchArm>, span: Span },

    // spawn fn(args)
    Spawn { fn_name: String, args: Vec<Expr>, span: Span },

    // Колекції
    List(Vec<Expr>, Span),
    Map(Vec<(Expr, Expr)>, Span),
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::Number(_, s)       => s,
            Expr::StringLit(_, s)    => s,
            Expr::Bool(_, s)         => s,
            Expr::Ident(_, s)        => s,
            Expr::ResultCtor { span, .. } => span,
            Expr::BinOp { span, .. } => span,
            Expr::Neg { span, .. }   => span,
            Expr::Try { span, .. }   => span,
            Expr::Call { span, .. }  => span,
            Expr::Field { span, .. } => span,
            Expr::MethodCall { span, .. } => span,
            Expr::Index { span, .. } => span,
            Expr::Match { span, .. } => span,
            Expr::Spawn { span, .. } => span,
            Expr::List(_, s)         => s,
            Expr::Map(_, s)          => s,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div,
    Eq, NotEq, Lt, Gt, LtEq, GtEq,
}

#[derive(Debug, Clone)]
pub enum ResultVariant { Ok, Err }

// ---------------------------------------------------------------------------
// Match
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body:    Expr,
    pub span:    Span,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,                          // _
    Literal(LiteralPat),               // 42, "str", true
    Variant(String),                   // Active, Inactive
    Ctor(String, String),              // Ok(v), Err(e)
}

#[derive(Debug, Clone)]
pub enum LiteralPat {
    Number(f64),
    Str(String),
    Bool(bool),
}
