/// Bytecode інструкції Oberih VM.
/// Stack-based: кожна інструкція працює з вершиною стеку.

#[derive(Debug, Clone)]
pub enum Instr {
    // --- Константи ---
    PushNum(f64),
    PushStr(String),
    PushBool(bool),
    PushNil,

    // --- Змінні ---
    LoadLocal(usize),   // завантажити зі слоту локальної змінної
    StoreLocal(usize),  // зберегти в слот
    LoadGlobal(String), // завантажити глобальну функцію/константу
    LoadField(String),  // pop struct -> push struct.field
    StoreField(String), // pop value, pop struct -> struct.field = value

    // --- Арифметика ---
    Add,
    Sub,
    Mul,
    Div,
    Neg,

    // --- Порівняння ---
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,

    // --- Result<T,E> ---
    MakeOk,     // pop value -> push Ok(value)
    MakeErr,    // pop value -> push Err(value)
    TryUnwrap,  // pop Result: якщо Ok -> push value; якщо Err -> PropagateErr

    // --- Struct ---
    MakeStruct { name: String, field_count: usize }, // pop N fields -> push Struct
    CopyStruct,  // pop struct -> push глибока копія

    // --- Список ---
    MakeList(usize),    // pop N елементів -> push List
    LoadIndex,          // pop index, pop list -> push list[index]
    StoreIndex,         // pop value, pop index, pop list -> list[index] = value

    // --- Виклики ---
    Call(usize),         // pop N args + callee -> push result
    CallMethod { name: String, arg_count: usize }, // pop N args + receiver -> push result
    Return,

    // --- Jumps ---
    Jump(usize),         // безумовний стрибок на адресу
    JumpIfFalse(usize),  // pop bool -> стрибок якщо false
    JumpIfTrue(usize),   // pop bool -> стрибок якщо true

    // --- Match ---
    MatchVariant(String),       // peek Result: якщо variant == name -> push true, інакше false
    MatchLitNum(f64),
    MatchLitStr(String),
    MatchLitBool(bool),
    MatchWildcard,              // завжди true
    UnwrapVariantInto(usize),   // pop Result -> push value -> StoreLocal(slot)

    // --- Spawn ---
    Spawn { fn_name: String, arg_count: usize }, // pop N args -> push SpawnHandle

    // --- Дублювання / drop ---
    Dup,    // дублювати вершину стеку
    Pop,    // викинути вершину стеку
    Swap,   // поміняти два верхні елементи

    // --- Діагностика ---
    Nop,
}

/// Скомпільована функція — ім'я + список інструкцій + кількість локальних слотів.
#[derive(Debug, Clone)]
pub struct CompiledFn {
    pub name:       String,
    pub code:       Vec<Instr>,
    pub local_count: usize,
    /// Resilience метадані — зберігаються для VM щоб застосувати під час виклику.
    pub resilience: ResilienceMeta,
}

/// Resilience параметри скомпільованої функції.
#[derive(Debug, Clone, Default)]
pub struct ResilienceMeta {
    pub deadline_secs:     Option<f64>,
    pub retry_budget:      Option<u32>,
    pub retries:           Option<u32>,
    pub fallback_fn:       Option<String>,
    pub timeout_secs:      Option<f64>,
    pub circuit_breaker:   Option<CircuitBreakerMeta>,
    pub rate_limit:        Option<RateLimitMeta>,
    pub bulkhead_max:      Option<u32>,
    pub hedging_after_secs: Option<f64>,
    pub is_idempotent:     bool,
    pub cache_ttl_secs:    Option<f64>,
    pub is_durable:        bool,
    pub is_traced:         bool,
    pub budget_tokens:     Option<f64>,
    pub budget_cost:       Option<f64>,
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerMeta {
    pub fail_threshold: u32,
    pub cooldown_secs:  f64,
}

#[derive(Debug, Clone)]
pub struct RateLimitMeta {
    pub n:        u32,
    pub per_secs: f64,
}

/// Весь скомпільований модуль.
#[derive(Debug, Clone)]
pub struct Module {
    pub functions: Vec<CompiledFn>,
}

impl Module {
    pub fn find_fn(&self, name: &str) -> Option<&CompiledFn> {
        self.functions.iter().find(|f| f.name == name)
    }
}
