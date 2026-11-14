/// Bytecode інструкції Oberih VM.
/// Stack-based: кожна інструкція працює з вершиною стеку.

#[derive(Debug, Clone)]
pub enum Instr {
    // --- Константи ---
    PushNum(f64),
    PushStr(String),
    PushBool(bool),
    PushEnum(String, String),  // (ім'я enum типу, ім'я варіанту)
    PushNil,

    // --- Змінні ---
    LoadLocal(usize),   // завантажити зі слоту локальної змінної
    StoreLocal(usize),  // зберегти в слот
    LoadGlobal(String), // завантажити глобальну функцію/константу
    /// Виклик глобальної (користувацької або вбудованої) функції за ім'ям —
    /// об'єднання `LoadGlobal(name) + Call(argc)` без проміжного `Value::Fn`
    /// (економить виділення рядка та повторний пошук функції на кожен виклик).
    /// Аргументи вже лежать на стеку (перший — найглибше).
    CallGlobal { name: String, argc: usize },
    LoadField(String),  // pop struct -> push struct.field
    StoreField(String), // pop value, pop struct -> struct.field = value

    // --- Арифметика ---
    Add,
    Sub,
    Mul,
    Div,
    Neg,

    // --- Бітові (операнди — Number, що представляє 32-бітне ціле зі знаком) ---
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Shl,
    Shr,

    // --- Логічні ---
    And,
    Or,
    Not,

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
    MakeStruct { name: String, field_names: Vec<String> }, // pop N fields -> push Struct
    CopyStruct,  // pop struct -> push глибока копія

    // --- Список ---
    MakeList(usize),    // pop N елементів -> push List
    MakeMap(usize),     // pop 2N (key, value, ...) -> push Map
    MakeClosure(String, usize), // pop N захоплених значень -> push Closure(fn, captured)
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

impl ResilienceMeta {
    /// true, якщо ЖОДЕН модифікатор не впливає на виконання (`call_compiled`
    /// звелася б до звичайного виклику). Такі функції можна викликати через
    /// швидкий шлях явного стека кадрів (без рекурсії Rust) у VM — див.
    /// vm::exec_frame_inner. Навмисно перелічені лише поля, які ДІЙСНО щось
    /// роблять у call_compiled/exec_with_resilience (звірено з їхнім кодом);
    /// інші поля (idempotent/durable/traced/cache/hedging/budget_*) наразі
    /// ніде не читаються VM, тож не впливають на коректність цієї перевірки —
    /// але якщо колись стануть щось робити, цей список треба буде доповнити.
    pub fn is_trivial(&self) -> bool {
        self.deadline_secs.is_none()
            && self.retry_budget.is_none()
            && self.retries.is_none()
            && self.fallback_fn.is_none()
            && self.timeout_secs.is_none()
            && self.circuit_breaker.is_none()
            && self.rate_limit.is_none()
            && self.bulkhead_max.is_none()
    }
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
    pub functions:    Vec<CompiledFn>,
    pub struct_table: crate::compiler::struct_table::StructTable,
    /// Підказка ім'я -> індекс у `functions` (O(1) замість лінійного пошуку).
    /// Лише підказка: `functions` лишається публічним (REPL додає в нього
    /// функції напряму), тому кожне влучання перевіряється, а при промаху
    /// `find_fn_index` падає назад на лінійний пошук — результат завжди такий
    /// самий, як у старого `iter().position(..)` (перша функція з цим іменем).
    fn_hint: std::collections::HashMap<String, usize, FnvBuild>,
    /// Скільки функцій було в `functions`, коли будувалась підказка. REPL лише
    /// ДОДАЄ функції в кінець, тож на промаху достатньо переглянути хвіст
    /// `functions[hinted_len..]` — а не всі функції (це робило кожен виклик
    /// вбудованої функції O(кількість користувацьких функцій)).
    hinted_len: usize,
}

impl Module {
    pub fn new(functions: Vec<CompiledFn>, struct_table: crate::compiler::struct_table::StructTable) -> Self {
        let mut fn_hint: std::collections::HashMap<String, usize, FnvBuild> =
            std::collections::HashMap::with_capacity_and_hasher(functions.len(), FnvBuild::default());
        for (i, f) in functions.iter().enumerate() {
            fn_hint.entry(f.name.clone()).or_insert(i); // перше входження, як у position()
        }
        let hinted_len = functions.len();
        Module { functions, struct_table, fn_hint, hinted_len }
    }

    pub fn find_fn_index(&self, name: &str) -> Option<usize> {
        match self.fn_hint.get(name) {
            Some(&i) => {
                if self.functions.get(i).map_or(false, |f| f.name == name) {
                    return Some(i);
                }
                // Підказка влучила, але не підтвердилась (ім'я змінили на місці) — повний пошук.
                self.functions.iter().position(|f| f.name == name)
            }
            None => {
                // Промах підказки: нові функції могли бути додані в кінець після побудови
                // (REPL лише додає) — дивимось тільки на хвіст.
                let start = self.hinted_len.min(self.functions.len());
                self.functions[start..].iter().position(|f| f.name == name).map(|i| i + start)
            }
        }
    }

    pub fn find_fn(&self, name: &str) -> Option<&CompiledFn> {
        self.find_fn_index(name).map(|i| &self.functions[i])
    }
}

/// Простий швидкий хешер FNV-1a для коротких ключів (імена функцій).
/// Стандартний SipHash захищає від hash-flooding, тут ключі — імена з
/// власної програми, тож ця властивість не потрібна, а швидкість важлива:
/// пошук функції виконується на КОЖЕН виклик.
#[derive(Debug, Clone, Copy)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self { Fnv(0xcbf29ce484222325) }
}

impl std::hash::Hasher for Fnv {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

pub type FnvBuild = std::hash::BuildHasherDefault<Fnv>;

/// Вбудовані функції, які обробляє `Instr::Call` напряму (призупиняють задачу чи
/// керують планувальником): sleep, неблокуючі HTTP-виклики, serve/stopServer.
/// Їх обробляє `Instr::Call` (з `FrameOutcome::Suspend`), тож компілятор НЕ має
/// замінювати такі виклики швидкою інструкцією `CallGlobal`, яка цієї логіки не має.
pub fn is_suspending_builtin(name: &str) -> bool {
    matches!(name, "sleep" | "httpGet" | "httpPost" | "httpPut" | "httpDelete" | "serve" | "stopServer")
}
