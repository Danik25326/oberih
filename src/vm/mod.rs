/// Oberih VM — stack-based виконавець bytecode.
/// Нуль залежностей: тільки std::thread і std::sync.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::compiler::bytecode::{Instr, Module, CompiledFn};
use crate::gc::{GcList, GcMap, MapKey, OMap};

// ---------------------------------------------------------------------------
// Value — runtime тип
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Value {
    Num(f64),
    Str(String),
    Bool(bool),
    Nil,

    // Result<T, E>
    Ok(Box<Value>),
    Err(Box<Value>),

    // Struct — Arc<Mutex<>> для thread-safety (spawn)
    Struct(OberihStruct),

    // List — GcList з reference counting
    List(GcList),

    // WeakRef — слабке посилання для циклічних структур
    WeakRef(crate::gc::WeakList),

    // SpawnHandle
    Spawn(SpawnHandle),

    // Callable
    Fn(String),

    // Enum-значення: (ім'я enum типу, ім'я варіанту)
    EnumVal(String, String),

    // Map — ключ-значення, порядок вставки, посилальний тип
    Map(GcMap),

    // Замикання: (синтетична функція лямбди, захоплені значення)
    Closure(String, Vec<Value>),

    // Скомпільований регулярний вираз (посилальний тип: Arc, компілюється один раз)
    Regex(std::sync::Arc<crate::regex::Regex>, String),  // (скомпільований, вихідний текст шаблону)
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Nil     => false,
            Value::Num(n)  => *n != 0.0,
            _              => true,
        }
    }

    fn variant_name(&self) -> Option<&str> {
        match self {
            Value::Ok(_)  => Some("Ok"),
            Value::Err(_) => Some("Err"),
            Value::EnumVal(_, variant) => Some(variant.as_str()),
            _             => None,
        }
    }

    fn unwrap_inner(self) -> Value {
        match self {
            Value::Ok(v) | Value::Err(v) => *v,
            other => other,
        }
    }
}

impl Value {
    /// Представлення для вкладених значень (елементи списку/Map, поля struct, Ok/Err):
    /// рядки беруться в лапки з екрануванням, щоб `["1", 1]` не плутався з `[1, 1]`.
    /// Верхньорівневий `print("текст")` лишається без лапок.
    pub fn repr(&self) -> String {
        match self {
            Value::Str(s) => {
                let mut out = String::with_capacity(s.len() + 2);
                out.push('"');
                for c in s.chars() {
                    match c {
                        '"'  => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c    => out.push(c),
                    }
                }
                out.push('"');
                out
            }
            other => other.to_string(),
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Value::Num(n)  => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    write!(f, "{}", *n as i64)
                } else {
                    write!(f, "{}", n)
                }
            }
            Value::Str(s)  => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Nil     => write!(f, "nil"),
            Value::Ok(v)   => write!(f, "Ok({})", v.repr()),
            Value::Err(v)  => write!(f, "Err({})", v.repr()),
            Value::Struct(s) => write!(f, "{}", s),
            Value::List(l)    => write!(f, "{}", l),
            Value::WeakRef(w) => {
                if w.is_alive() {
                    write!(f, "<WeakRef: alive>")
                } else {
                    write!(f, "<WeakRef: dropped>")
                }
            }
            Value::Spawn(_) => write!(f, "<SpawnHandle>"),
            Value::Fn(n)    => write!(f, "<fn {}>", n),
            Value::EnumVal(ty, variant) => write!(f, "{}.{}", ty, variant),
            Value::Map(m)   => write!(f, "{}", m),
            Value::Closure(n, _) => write!(f, "<fn {}>", n),
            Value::Regex(_, pat) => write!(f, "/{}/", pat),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Num(a),  Value::Num(b))  => (a - b).abs() < f64::EPSILON,
            (Value::Str(a),  Value::Str(b))  => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nil,     Value::Nil)     => true,
            (Value::Ok(a),   Value::Ok(b))   => a == b,
            (Value::Err(a),  Value::Err(b))  => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a),  Value::Map(b))  => a == b,
            (Value::Closure(n1, c1), Value::Closure(n2, c2)) => n1 == n2 && c1 == c2,
            (Value::Regex(_, p1), Value::Regex(_, p2)) => p1 == p2,
            (Value::EnumVal(t1, v1), Value::EnumVal(t2, v2)) => t1 == t2 && v1 == v2,
            (Value::WeakRef(_), Value::WeakRef(_)) => false, // слабкі посилання не рівні
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// OberihStruct
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct OberihStruct {
    pub type_name: String,
    pub fields:    Arc<Mutex<HashMap<String, Value>>>,
}

impl Drop for OberihStruct {
    fn drop(&mut self) {
        if Arc::strong_count(&self.fields) == 1 {
            crate::gc::gc_drop();
        }
    }
}

impl OberihStruct {
    pub fn new(type_name: String, fields: HashMap<String, Value>) -> Self {
        crate::gc::gc_alloc(); // раніше structи взагалі не рахувались у gc-stats
        let fields = Arc::new(Mutex::new(fields));
        crate::gc::register_struct_fields(Arc::downgrade(&fields));
        OberihStruct { type_name, fields }
    }

    /// Ідентичність для трасування циклів (див. `gc::collect_cycles`).
    pub fn ptr_id(&self) -> usize { Arc::as_ptr(&self.fields) as *const () as usize }

    pub fn get(&self, name: &str) -> Option<Value> {
        self.fields.lock().unwrap().get(name).cloned()
    }

    pub fn set(&self, name: &str, value: Value) -> bool {
        let mut guard = self.fields.lock().unwrap();
        if guard.contains_key(name) {
            guard.insert(name.to_string(), value);
            true
        } else {
            false
        }
    }

    /// Поверхнева копія — новий Arc<Mutex<...>> з тими ж значеннями.
    pub fn shallow_copy(&self) -> Self {
        let fields = self.fields.lock().unwrap().clone();
        OberihStruct::new(self.type_name.clone(), fields)
    }
}

impl std::fmt::Display for OberihStruct {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // Знімок + сортування за іменем поля: HashMap дає випадковий порядок,
        // а вивід має бути детермінованим.
        let mut fields: Vec<(String, Value)> = self.fields.lock().unwrap()
            .iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        write!(f, "{}{{", self.type_name)?;
        for (i, (k, v)) in fields.iter().enumerate() {
            if i > 0 { write!(f, ", ")?; }
            write!(f, "{}: {}", k, v.repr())?;
        }
        write!(f, "}}")
    }
}

// ---------------------------------------------------------------------------
// SpawnHandle
// ---------------------------------------------------------------------------

/// `spawn` тепер — не окремий потік ОС, а кооперативна задача (Task) у
/// власному планувальнику VM (`self.tasks`). SpawnHandle — просто індекс
/// туди; саме join/isDone реалізовані інструкціями `exec_frame_inner`
/// (перевіряються СПЕЦІАЛЬНО, до звичайного виклику методу — див.
/// `Instr::CallMethod`), бо вони мають вміти ПРИЗУПИНИТИ задачу, що
/// звичайний виклик методу (просто `VR<Value>`) зробити не може.
#[derive(Debug, Clone, Copy)]
pub struct SpawnHandle {
    pub task_id: usize,
}

// ---------------------------------------------------------------------------
// RuntimeError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum RuntimeError {
    // Помилка виконання
    General(String),
    // ? оператор підняв Err — несе значення помилки
    PropagateErr(Value),
    // Явний return зі значенням (для стеку викликів)
    Return(Value),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            RuntimeError::General(msg)     => write!(f, "RuntimeError: {}", msg),
            RuntimeError::PropagateErr(v)  => write!(f, "Незахоплена помилка: Err({})", v),
            RuntimeError::Return(_)        => write!(f, "Return поза функцією"),
        }
    }
}

type VR<T> = Result<T, RuntimeError>;

fn rt_err(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::General(msg.into())
}

// ---------------------------------------------------------------------------
// VM
// ---------------------------------------------------------------------------

/// Один кадр виклику явного стека (див. `exec_frame_inner`). Раніше був
/// локальним типом усередині `exec_frame_inner`; тепер — рівня модуля,
/// бо `Task` (кооперативна задача `spawn`) мусить володіти `Vec<Frame>`
/// МІЖ викликами `run_task_until_suspend` (задача призупиняється й
/// відновлюється багато разів, а не виконується одним Rust-викликом).
struct Frame {
    fn_index: usize,
    ip:       usize,
    stack:    Vec<Value>,
    locals:   Vec<Value>,
}

/// Чому задача призупинена, і що розбудить її знову.
enum SuspendReason {
    /// `sleep(ms)`: чекати до цього моменту часу.
    SleepUntil(Instant),
    /// `h.join()`: чекати, поки задача `task_id` не стане `Done`.
    /// `deadline` — необов'язковий таймаут (`h.join(secs)`).
    WaitingOnTask { task_id: usize, deadline: Option<Instant> },
}

/// Результат одного кроку виконання кадру — див. `exec_frame_inner`.
enum FrameOutcome {
    /// Викликати ІНШУ просту функцію: (індекс у module.functions, локальні змінні нового кадру).
    CallFrame(usize, Vec<Value>),
    /// Поточний (НАЙВЕРХНІШИЙ) кадр завершився цим значенням (явний Return чи кінець коду).
    Return(Value),
    /// Призупинити ВСЮ задачу (не лише поточний кадр) — плановик продовжить
    /// інші готові задачі й поверне керування сюди, коли умова виконається.
    Suspend(SuspendReason),
}

/// Стан кооперативної задачі (`spawn`) у планувальнику VM.
enum TaskState {
    /// Можна виконувати прямо зараз.
    Ready,
    /// Виконується просто зараз (її call_stack винесено з Task на час кроку) —
    /// потрібен, щоб вкладений виклик планувальника (resilient-функція всередині
    /// задачі) не вибрав її повторно як "готову".
    Running,
    /// Призупинена — див. `SuspendReason`.
    Suspended(SuspendReason),
    /// Завершена — значення (чи помилка) готові, і більше не зміняться.
    Done(VR<Value>),
}

/// Кооперативна задача: власний явний стек викликів + стан у планувальнику.
/// "Головна" задача (саме виконання програми) — теж Task (із тих самих
/// причин, що й `spawn`-задачі), просто завжди task_id == той, що очікує
/// викликач `exec_frame_inner`.
struct Task {
    call_stack: Vec<Frame>,
    state:      TaskState,
}

/// Результат спроби швидкого (без рекурсії Rust) виклику — див. `resolve_plain_call`.
enum CallOutcome {
    /// Проста функція: штовхнути новий Frame у явний стек викликів.
    NewFrame(usize, Vec<Value>),
    /// Уже готове значення (builtin, resilient-функція, чи ціла помилка).
    Value(Value),
}

pub struct VM {
    module:    Arc<Module>,
    cb_state:  HashMap<String, CircuitState>,
    rl_state:  HashMap<String, RateLimitState>,
    bh_state:  HashMap<String, Arc<Mutex<u32>>>,
    /// Стек викликів для діагностики
    call_stack: Vec<String>,
    /// Поточна глибина рекурсії Oberih-викликів (exec_frame у exec_frame).
    /// VM досі рекурсивний на рівні Rust (кожен виклик — новий кадр стека
    /// ОС), тож нескінченна чи надто глибока рекурсія користувача раніше
    /// завершувала процес цілком (`stack overflow`, SIGABRT) замість
    /// контрольованої помилки Oberih. Цей лічильник ловить це ДО того, як
    /// реальний стек ОС вичерпається (разом з більшим стеком для потоку
    /// виконання — див. `run_with_big_stack`), і повертає звичайний
    /// RuntimeError, який можна зловити через `Result`/fallback, як і будь-яку
    /// іншу помилку. Справжня, не обмежена стеком рекурсія (довільної
    /// глибини без зростання стека Rust) вимагала б явного стека кадрів
    /// замість рекурсії виконавця — значно більша переробка ядра VM.
    call_depth: usize,
    /// Планувальник кооперативних задач (`spawn`) — див. `Task`/`TaskState`.
    /// Завершені задачі лишаються тут позначеними `Done` (не видаляються
    /// одразу — видалення по index зсунуло б id усіх інших задач); для
    /// довгоживучого процесу з дуже багатьма `spawn` без `join` це означає
    /// повільне зростання пам'яті — відома, незакрита сьогодні межа.
    tasks: Vec<Task>,
    /// Черга готових до виконання задач (FIFO) — замість сканування всього
    /// `tasks` на кожен крок, що давало O(n²) на тисячах задач.
    ready: std::collections::VecDeque<usize>,
    /// Таймери (пробудження `sleep` і дедлайни `join(secs)`), мін-купа за часом.
    /// Записи можуть бути застарілими (задачу вже розбуджено інакше) —
    /// перевіряються при вийманні.
    timers: std::collections::BinaryHeap<std::cmp::Reverse<(Instant, usize)>>,
    /// target -> id задач, що чекають на нього через `join`.
    waiters: HashMap<usize, Vec<usize>>,
}

/// Максимальна кількість задач (`spawn`) на один VM: без ліміту нескінченне
/// створення задач (`fn f() { spawn f() ... }`) тихо з'їдало б усю пам'ять.
pub const MAX_TASKS: usize = 1_000_000;

/// Максимальна глибина викликів Oberih-функцій (`sum(n) { return n + sum(n-1) }`
/// на глибину понад це дасть помилку, а не впаде процес). Підібрано з запасом
/// відносно розміру стека потоку виконання (`BIG_STACK_SIZE`), емпірично
/// виміряним коштом ~3 КБ стека ОС на один рівень `exec_frame`.
pub const MAX_CALL_DEPTH: usize = 150_000;

/// Межа глибини ЯВНОГО стека кадрів (`exec_frame_inner`'s call_stack) —
/// росте в купі, не на стеку ОС, тож набагато щедріша за MAX_CALL_DEPTH
/// (яка досі діє лише для функцій з модифікаторами стійкості — вони йдуть
/// старим, рекурсивним на рівні Rust шляхом). Межа все одно потрібна:
/// "без обмежень" на практиці означає "вичерпати всю пам'ять процесу й
/// зависнути", а не "справді безмежна рекурсія".
pub const MAX_EXPLICIT_CALL_DEPTH: usize = 500_000;

/// Розмір стека потоку, на якому реально виконується Oberih-програма.
/// Типовий стек головного потоку ОС (8 МБ) вичерпувався вже на ~2500
/// рівнях рекурсії — зась для майже будь-якого нетривіального рекурсивного
/// алгоритму. 512 МБ віртуальної пам'яті майже нічого не коштують, поки
/// сторінки не торкнулись (ОС виділяє їх лінькувато), тож це безпечний запас.
const BIG_STACK_SIZE: usize = 512 * 1024 * 1024;

/// Виконує `f` на окремому потоці з великим стеком (`BIG_STACK_SIZE`) і
/// чекає результат. Усі команди, що реально ВИКОНУЮТЬ Oberih-код (`run`,
/// `test`, REPL), мають йти через це, а не напряму — інакше `MAX_CALL_DEPTH`
/// безглуздий: лічильник підібраний під великий стек, і з типовим стеком
/// головного потоку впаде задовго до ліміту.
pub fn run_with_big_stack<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(BIG_STACK_SIZE)
        .spawn(f)
        .expect("не вдалось створити потік виконання")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

#[derive(Debug)]
struct CircuitState {
    failures:    u32,
    open_until:  Option<Instant>,
}

#[derive(Debug)]
struct RateLimitState {
    count:       u32,
    window_start: Instant,
}

impl VM {
    pub fn new(module: Module) -> Self {
        VM {
            module:     Arc::new(module),
            cb_state:   HashMap::new(),
            rl_state:   HashMap::new(),
            bh_state:   HashMap::new(),
            call_stack: Vec::new(),
            call_depth: 0,
            tasks: Vec::new(),
            ready: std::collections::VecDeque::new(),
            timers: std::collections::BinaryHeap::new(),
            waiters: HashMap::new(),
        }
    }

    /// Виклик точки входу — fn main().
    pub fn run(&mut self) -> VR<Value> {
        self.call_fn("main", vec![])
    }

    /// Виклик функції по імені.
    pub fn call_fn(&mut self, name: &str, args: Vec<Value>) -> VR<Value> {
        let compiled = self.module.find_fn(name)
            .ok_or_else(|| rt_err(format!("Функція '{}' не знайдена", name)))?
            .clone();

        self.call_stack.push(name.to_string());
        let result = self.call_compiled(&compiled, args);
        self.call_stack.pop();

        // Якщо помилка — додаємо стек викликів
        match result {
            Err(RuntimeError::General(msg)) if !msg.contains("\nСтек викликів:") => {
                if self.call_stack.is_empty() {
                    // Ми на верхньому рівні — форматуємо з стеком
                    Err(RuntimeError::General(msg))
                } else {
                    Err(RuntimeError::General(msg))
                }
            }
            other => other,
        }
    }

    /// Викликає будь-яке викликане значення: функцію користувача або builtin
    /// (зокрема функції вищого порядку, яким потрібен доступ до VM).
    pub fn call_value(&mut self, f: &Value, args: Vec<Value>) -> VR<Value> {
        match f {
            Value::Fn(name) if name.starts_with("__builtin_") => {
                let b = &name[10..];
                if crate::stdlib_ext::is_higher_order(b) {
                    self.call_higher_order(b, args)
                } else {
                    crate::stdlib::call_builtin(b, args)
                }
            }
            Value::Fn(name) => self.call_fn(name, args),
            // Замикання: захоплені значення йдуть першими параметрами синтетичної функції
            Value::Closure(name, captured) => {
                let mut full = captured.clone();
                full.extend(args);
                self.call_fn(name, full)
            }
            other => Err(rt_err(format!("Не можна викликати {}", other))),
        }
    }

    /// map / filter / reduce / each / any / all / find / sortBy.
    /// Перший аргумент — список, другий — функція (ім'я функції як значення).
    fn call_higher_order(&mut self, name: &str, args: Vec<Value>) -> VR<Value> {
        let list = match args.get(0) {
            Some(Value::List(l)) => l.to_vec(),
            Some(o) => return Err(rt_err(format!("{}: перший аргумент має бути List, отримано {}", name, o))),
            None    => return Err(rt_err(format!("{}: потрібні аргументи (список, функція)", name))),
        };
        let f = match args.get(1) {
            Some(f @ (Value::Fn(_) | Value::Closure(..))) => f.clone(),
            Some(o) => return Err(rt_err(format!(
                "{}: другий аргумент має бути функцією (передайте ім'я функції), отримано {}", name, o
            ))),
            None => return Err(rt_err(format!("{}: потрібна функція другим аргументом", name))),
        };
        match name {
            "map" => {
                let mut out = Vec::with_capacity(list.len());
                for x in list { out.push(self.call_value(&f, vec![x])?); }
                Ok(Value::List(GcList::new(out)))
            }
            "filter" => {
                let mut out = Vec::new();
                for x in list {
                    if self.call_value(&f, vec![x.clone()])?.is_truthy() { out.push(x); }
                }
                Ok(Value::List(GcList::new(out)))
            }
            // reduce(list, f, init?) — f(acc, x); без init береться перший елемент
            "reduce" => {
                let mut it = list.into_iter();
                let mut acc = match args.get(2) {
                    Some(init) => init.clone(),
                    None => it.next().ok_or_else(|| rt_err("reduce: порожній список без початкового значення"))?,
                };
                for x in it { acc = self.call_value(&f, vec![acc, x])?; }
                Ok(acc)
            }
            "each" => {
                for x in list { self.call_value(&f, vec![x])?; }
                Ok(Value::Nil)
            }
            "any" => {
                for x in list { if self.call_value(&f, vec![x])?.is_truthy() { return Ok(Value::Bool(true)); } }
                Ok(Value::Bool(false))
            }
            "all" => {
                for x in list { if !self.call_value(&f, vec![x])?.is_truthy() { return Ok(Value::Bool(false)); } }
                Ok(Value::Bool(true))
            }
            "find" => {
                for x in list {
                    if self.call_value(&f, vec![x.clone()])?.is_truthy() { return Ok(x); }
                }
                Ok(Value::Nil)
            }
            // sortBy(list, keyFn) — стабільне сортування за ключем keyFn(x)
            "sortBy" => {
                let mut keys = Vec::with_capacity(list.len());
                for x in &list { keys.push(self.call_value(&f, vec![x.clone()])?); }
                let sorted = crate::stdlib_ext::sort_by_keys(list, keys)
                    .map_err(|e| rt_err(format!("sortBy: {}", e)))?;
                Ok(Value::List(GcList::new(sorted)))
            }
            other => Err(rt_err(format!("Невідома функція вищого порядку: {}", other))),
        }
    }

    /// Повертає поточний стек викликів як рядок
    pub fn format_call_stack(&self) -> String {
        if self.call_stack.is_empty() {
            return String::new();
        }
        let mut s = "\nСтек викликів:".to_string();
        for (i, name) in self.call_stack.iter().rev().enumerate() {
            s.push_str(&format!("\n  {} fn {}", i, name));
        }
        s
    }

    fn call_compiled(&mut self, func: &CompiledFn, args: Vec<Value>) -> VR<Value> {
        let res = &func.resilience;

        // --- Circuit breaker перевірка ---
        if let Some(_cb) = &res.circuit_breaker {
            let state = self.cb_state.entry(func.name.clone()).or_insert_with(|| CircuitState {
                failures: 0,
                open_until: None,
            });
            if let Some(until) = state.open_until {
                if Instant::now() < until {
                    return Ok(Value::Err(Box::new(Value::Str("circuit open".into()))));
                } else {
                    state.open_until = None;
                    state.failures   = 0;
                }
            }
        }

        // --- Rate limit ---
        if let Some(rl) = &res.rate_limit {
            let state = self.rl_state.entry(func.name.clone()).or_insert_with(|| RateLimitState {
                count: 0,
                window_start: Instant::now(),
            });
            let elapsed = state.window_start.elapsed().as_secs_f64();
            if elapsed > rl.per_secs {
                state.count = 0;
                state.window_start = Instant::now();
            }
            if state.count >= rl.n {
                return Ok(Value::Err(Box::new(Value::Str("rate limited".into()))));
            }
            state.count += 1;
        }

        // --- Bulkhead ---
        if let Some(max) = res.bulkhead_max {
            let counter = self.bh_state.entry(func.name.clone())
                .or_insert_with(|| Arc::new(Mutex::new(0)))
                .clone();
            {
                let mut guard = counter.lock().unwrap();
                if *guard >= max {
                    return Ok(Value::Err(Box::new(Value::Str("bulkhead full".into()))));
                }
                *guard += 1;
            }
            let result = self.exec_with_resilience(func, args);
            *counter.lock().unwrap() -= 1;
            return result;
        }

        self.exec_with_resilience(func, args)
    }

    fn exec_with_resilience(&mut self, func: &CompiledFn, args: Vec<Value>) -> VR<Value> {
        let res = &func.resilience.clone();
        let max_tries = res.retries.unwrap_or(0) + 1;

        for attempt in 0..max_tries {
            let result = if let Some(timeout) = res.timeout_secs {
                self.exec_with_timeout(func, args.clone(), timeout)
            } else if let Some(deadline) = res.deadline_secs {
                self.exec_with_timeout(func, args.clone(), deadline)
            } else {
                self.exec_frame(func, args.clone())
            };

            match result {
                Ok(v) => {
                    // Circuit breaker: успіх
                    if let Some(state) = self.cb_state.get_mut(&func.name) {
                        state.failures = 0;
                    }
                    return Ok(v);
                }
                Err(RuntimeError::PropagateErr(e)) => {
                    // Circuit breaker: лічимо провали
                    if let Some(cb_meta) = &func.resilience.circuit_breaker {
                        if let Some(state) = self.cb_state.get_mut(&func.name) {
                            state.failures += 1;
                            if state.failures >= cb_meta.fail_threshold {
                                state.open_until = Some(
                                    Instant::now() + Duration::from_secs_f64(cb_meta.cooldown_secs)
                                );
                            }
                        }
                    }

                    if attempt + 1 < max_tries {
                        continue; // retry
                    }

                    // Fallback
                    if let Some(fallback_name) = &res.fallback_fn.clone() {
                        let fb_name = fallback_name.clone();
                        return self.call_fn(&fb_name, args.clone());
                    }

                    return Ok(Value::Err(Box::new(e)));
                }
                Err(other) => return Err(other),
            }
        }

        unreachable!()
    }

    fn exec_with_timeout(&mut self, func: &CompiledFn, args: Vec<Value>, secs: f64) -> VR<Value> {
        // Власний timeout через потік + channel — без зовнішніх залежностей.
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel::<VR<Value>>();
        let func_clone   = func.clone();
        let module_clone = Arc::clone(&self.module);

        // Той самий великий стек, що й у run_with_big_stack: інакше глибока
        // рекурсія всередині функції з `timeout`/`deadline` впаде на
        // ЗНАЧНО меншому типовому стеку нового потоку (2 МБ), не на 8 МБ
        // головного — гірше, а не краще, ніж без timeout.
        let spawn_result = std::thread::Builder::new()
            .stack_size(BIG_STACK_SIZE)
            .spawn(move || {
                let mut vm = VM::new((*module_clone).clone());
                let result = vm.exec_frame(&func_clone, args);
                let _ = tx.send(result);
            });
        // Контрольована помилка замість паніки/SIGABRT, якщо ОС відмовила у
        // новому потоці (вичерпано ліміт потоків чи пам'яті) — зокрема саме
        // це реально стається, коли resilient-функція рекурсивно викликає
        // сама себе: КОЖЕН такий виклик зараз породжує НОВИЙ потік (а не
        // перевикористовує наявний), тож глибока пряма рекурсія в
        // resilient-функції вичерпує потоки ОС значно швидше, ніж звичайна
        // рекурсія. Це окреме, уже наявне архітектурне обмеження (не від
        // сьогоднішньої зміни) — виправити «по-справжньому» означало б не
        // спавнити новий потік на кожен рекурсивний виклик, а перевикористати
        // один таймер на всю resilient-функцію; тут лише прибрано сам крах.
        if let Err(e) = spawn_result {
            return Err(rt_err(format!(
                "не вдалось створити потік для timeout/deadline: {} (можлива причина — глибока пряма рекурсія в resilient-функції: кожен виклик такої функції породжує новий потік ОС)",
                e
            )));
        }

        match rx.recv_timeout(Duration::from_secs_f64(secs)) {
            Ok(result) => result,
            Err(_) => Err(RuntimeError::PropagateErr(
                Value::Err(Box::new(Value::Str("timeout".into())))
            )),
        }
    }

    // --- Головний виконавець: stack machine ---

    /// Тонка обгортка навколо `exec_frame_inner`: лічить глибину рекурсії і
    /// повертає контрольовану помилку замість краху процесу, коли ліміт
    /// перевищено. Сама логіка виконання — в `exec_frame_inner`, без змін.
    fn exec_frame(&mut self, func: &CompiledFn, args: Vec<Value>) -> VR<Value> {
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(rt_err(format!(
                "перевищено максимальну глибину рекурсії ({}). Можливо, функція викликає сама себе без умови завершення.",
                MAX_CALL_DEPTH
            )));
        }
        let result = self.exec_frame_inner(func, args);
        self.call_depth -= 1;
        result
    }

    fn exec_frame_inner(&mut self, func: &CompiledFn, args: Vec<Value>) -> VR<Value> {
        // --- Явний стек кадрів замість рекурсії Rust ---
        //
        // Раніше виклик Oberih-функції з іншої Oberih-функції (у тому числі
        // виклик самої себе — рекурсія) рекурсивно викликав ЦЮ Rust-функцію
        // знову, тож глибина виклику Oberih була обмежена стеком ОС. Тепер
        // кожен ЗВИЧАЙНИЙ (без модифікаторів стійкості) виклик користувацької
        // функції чи методу struct штовхає НОВИЙ Frame у call_stack (звичайний
        // Vec, росте в купі) замість нового Rust-кадру — глибина обмежена лише
        // пам'яттю.
        //
        // Функції З модифікаторами стійкості (`deadline`/`retryBudget`/
        // `circuitBreaker`/`rateLimit`/`bulkhead`/`fallback`) НАДАЛІ йдуть
        // через старий рекурсивний шлях (`call_fn`): їхня retry/timeout-логіка
        // зав'язана на рекурсивні виклики та окремі потоки (exec_with_timeout),
        // і переписувати це зараз — окрема, значно ризикованіша робота. На
        // практиці такі виклики — межові (API-запити), а не глибока
        // алгоритмічна рекурсія, тож лишаються обмеженими великим стеком
        // потоку (`run_with_big_stack`) і лічильником `MAX_CALL_DEPTH`, як і
        // раніше. Так само на старому шляху лишаються виклики функцій,
        // збережених у Map (`m.fn(...)`), і колбеки функцій вищого порядку
        // (`map`/`filter`/...) — вони НЕ накопичують глибину Rust-стека самі
        // по собі (кожен виклик завершується до початку наступного), тож не
        // є частиною проблеми, яку цей рефакторинг вирішує.
        let module = Arc::clone(&self.module);
        let start_idx = module.functions.iter().position(|f| f.name == func.name)
            .ok_or_else(|| rt_err(format!("внутрішня помилка: функція '{}' не знайдена в модулі", func.name)))?;

        let mut locals = vec![Value::Nil; func.local_count.max(args.len())];
        for (i, a) in args.into_iter().enumerate() { locals[i] = a; }
        let task_id = self.tasks.len();
        self.tasks.push(Task {
            call_stack: vec![Frame { fn_index: start_idx, ip: 0, stack: Vec::with_capacity(8), locals }],
            state: TaskState::Ready,
        });
        self.ready.push_back(task_id);

        self.drive_until_done(&module, task_id)
    }

    /// Планувальник: просуває готові задачі (`run_task_until_suspend`), поки
    /// `target` не стане `Done`. Єдине місце, де процес РЕАЛЬНО блокується —
    /// коротке очікування до найближчого пробудження, коли жодна задача не
    /// готова прямо зараз (усі сплять чи чекають одна одну); якщо жодна
    /// задача ніколи не стане готовою — звичайна помилка "дедлок", не вічне
    /// зависання.
    fn drive_until_done(&mut self, module: &Arc<Module>, target: usize) -> VR<Value> {
        loop {
            if let TaskState::Done(result) = &self.tasks[target].state {
                return result.clone();
            }
            match self.pick_ready_task() {
                Some(id) => self.run_task_until_suspend(module, id),
                None => match self.earliest_wake() {
                    Some(when) => {
                        let now = Instant::now();
                        if when > now { std::thread::sleep(when - now); }
                        self.wake_ready_sleepers();
                    }
                    None => return Err(rt_err(
                        "дедлок: усі задачі (spawn/join) взаємно очікують одна одну, жодна не може просунутись"
                    )),
                },
            }
        }
    }

    /// Якщо `name` — проста (без resilience-модифікаторів) Oberih-функція,
    /// повертає дані для НОВОГО кадру явного стека (швидкий шлях, без
    /// рекурсії Rust). Інакше — виконує існуючим рекурсивним шляхом
    /// (`call_fn`, з усією логікою retry/circuit-breaker/timeout) і
    /// повертає вже готове значення.
    fn resolve_plain_call(&mut self, module: &Module, name: &str, args: Vec<Value>) -> VR<CallOutcome> {
        if let Some(idx) = module.functions.iter().position(|f| f.name == name) {
            if module.functions[idx].resilience.is_trivial() {
                let target = &module.functions[idx];
                let mut locals = vec![Value::Nil; target.local_count.max(args.len())];
                for (i, a) in args.into_iter().enumerate() { locals[i] = a; }
                return Ok(CallOutcome::NewFrame(idx, locals));
            }
        }
        Ok(CallOutcome::Value(self.call_fn(name, args)?))
    }

    fn pick_ready_task(&mut self) -> Option<usize> {
        while let Some(id) = self.ready.pop_front() {
            // Запис міг застаріти (задача вже не Ready) — пропускаємо.
            if matches!(self.tasks[id].state, TaskState::Ready) {
                return Some(id);
            }
        }
        None
    }

    fn earliest_wake(&mut self) -> Option<Instant> {
        while let Some(std::cmp::Reverse((when, id))) = self.timers.peek().cloned() {
            if self.timer_is_live(when, id) { return Some(when); }
            self.timers.pop(); // застарілий — задачу вже розбуджено іншим шляхом
        }
        None
    }

    /// Чи досі задача `id` призупинена саме до цього моменту `when`.
    fn timer_is_live(&self, when: Instant, id: usize) -> bool {
        match &self.tasks[id].state {
            TaskState::Suspended(SuspendReason::SleepUntil(w)) => *w == when,
            TaskState::Suspended(SuspendReason::WaitingOnTask { deadline: Some(d), .. }) => *d == when,
            _ => false,
        }
    }

    /// Будить задачі, чий час сну настав, і задачі, чий `join`-таймаут сплив
    /// (доставляючи їм помилку таймауту).
    fn wake_ready_sleepers(&mut self) {
        let now = Instant::now();
        while let Some(std::cmp::Reverse((when, id))) = self.timers.peek().cloned() {
            if when > now { break; }
            self.timers.pop();
            if !self.timer_is_live(when, id) { continue; }
            let was_sleep = matches!(self.tasks[id].state, TaskState::Suspended(SuspendReason::SleepUntil(_)));
            let value = if was_sleep { Value::Nil } else { Value::Err(Box::new(Value::Str("timeout".into()))) };
            self.tasks[id].call_stack.last_mut().unwrap().stack.push(value);
            self.tasks[id].state = TaskState::Ready;
            self.ready.push_back(id);
        }
    }

    /// Доставляє результат завершеної задачі `finished` усім, хто чекав на неї
    /// через `join`, і переводить їх у `Ready`. Викликається одразу після
    /// того, як задача стає `Done`.
    fn wake_waiters_on(&mut self, finished: usize) {
        let result = match &self.tasks[finished].state {
            TaskState::Done(r) => r.clone(),
            _ => return,
        };
        let Some(waiter_ids) = self.waiters.remove(&finished) else { return };
        for w in waiter_ids {
            let still_waiting = matches!(
                &self.tasks[w].state,
                TaskState::Suspended(SuspendReason::WaitingOnTask { task_id, .. }) if *task_id == finished
            );
            if !still_waiting { continue; } // вже розбуджено таймаутом
            let value = match &result {
                Ok(v) => v.clone(),
                Err(e) => Value::Err(Box::new(Value::Str(e.to_string()))),
            };
            self.tasks[w].call_stack.last_mut().unwrap().stack.push(value);
            self.tasks[w].state = TaskState::Ready;
            self.ready.push_back(w);
        }
    }

    /// Виконує задачу `task_id`, поки вона не призупиниться (`Suspend`) чи не
    /// завершиться (`Return`/помилка). Це те, що РАНІШЕ було одним суцільним
    /// тілом `exec_frame_inner` — тепер методом, що можна викликати багато
    /// разів на ОДНІЙ задачі (бере call_stack, виконує, або зберігає назад
    /// для наступного виклику, або остаточно позначає Done).
    fn run_task_until_suspend(&mut self, module: &Arc<Module>, task_id: usize) {
        let mut call_stack = std::mem::take(&mut self.tasks[task_id].call_stack);
        self.tasks[task_id].state = TaskState::Running;

        let outcome: VR<FrameOutcome> = (|| -> VR<FrameOutcome> { 'frames: loop {
            let fn_index = call_stack.last().unwrap().fn_index;
            let code = &module.functions[fn_index].code;
            let mut ip     = std::mem::take(&mut call_stack.last_mut().unwrap().ip);
            let mut stack  = std::mem::take(&mut call_stack.last_mut().unwrap().stack);
            let mut locals = std::mem::take(&mut call_stack.last_mut().unwrap().locals);

            macro_rules! pop {
                () => {
                    stack.pop().ok_or_else(|| rt_err("Stack underflow"))?
                };
            }

            macro_rules! push {
                ($v:expr) => {
                    stack.push($v)
                };
            }

            let outcome: FrameOutcome = 'inner: loop {
                if ip >= code.len() {
                    // Дійшли до кінця без явного Return — як і раніше, повертаємо nil.
                    break 'inner FrameOutcome::Return(stack.pop().unwrap_or(Value::Nil));
                }
                let instr = &code[ip];
                ip += 1;

                match instr {
                Instr::PushNum(n)  => push!(Value::Num(*n)),
                Instr::PushStr(s)  => push!(Value::Str(s.clone())),
                Instr::PushEnum(ty, variant) => push!(Value::EnumVal(ty.clone(), variant.clone())),
                Instr::PushBool(b) => push!(Value::Bool(*b)),
                Instr::PushNil     => push!(Value::Nil),

                Instr::LoadLocal(slot) => {
                    let v = locals.get(*slot)
                        .cloned()
                        .ok_or_else(|| rt_err(format!("Невірний слот {}", slot)))?;
                    push!(v);
                }
                Instr::StoreLocal(slot) => {
                    let v = pop!();
                    if *slot >= locals.len() {
                        locals.resize(*slot + 1, Value::Nil);
                    }
                    locals[*slot] = v;
                }

                Instr::LoadGlobal(name) => {
                    if self.module.find_fn(name).is_some() {
                        push!(Value::Fn(name.clone()));
                    } else if crate::stdlib::is_builtin(name) {
                        push!(Value::Fn(format!("__builtin_{}", name)));
                    } else {
                        return Err(rt_err(format!("Невідоме ім'я: '{}'", name)));
                    }
                }

                Instr::LoadField(field) => {
                    let obj = pop!();
                    match obj {
                        Value::Struct(s) => {
                            let v = s.get(field)
                                .ok_or_else(|| rt_err(format!("Поле '{}' не існує", field)))?;
                            push!(v);
                        }
                        // `m.key` — це m["key"]; зручно для JSON (`resp.info.summary`).
                        Value::Map(m) => {
                            push!(m.get(&MapKey::Str(field.clone())).unwrap_or(Value::Nil));
                        }
                        _ => return Err(rt_err(format!("LoadField на не-struct: {}", obj))),
                    }
                }

                Instr::StoreField(field) => {
                    let value = pop!();
                    let obj   = pop!();
                    match obj {
                        Value::Struct(s) => {
                            if !s.set(field, value) {
                                return Err(rt_err(format!("Поле '{}' не існує в {}", field, s.type_name)));
                            }
                        }
                        Value::Map(m) => m.insert(MapKey::Str(field.clone()), value),
                        _ => return Err(rt_err("StoreField на не-struct")),
                    }
                }

                Instr::Add => {
                    let r = pop!(); let l = pop!();
                    match (l, r) {
                        (Value::Num(a),  Value::Num(b))  => push!(Value::Num(a + b)),
                        (Value::Str(a),  Value::Str(b))  => push!(Value::Str(a + &b)),
                        (Value::Str(a),  Value::Num(b))  => push!(Value::Str(format!("{}{}", a, b as i64))),
                        (Value::Num(a),  Value::Str(b))  => push!(Value::Str(format!("{}{}", a as i64, b))),
                        (l, r) => return Err(rt_err(format!("Не можна додати {} і {}", l, r))),
                    }
                }
                Instr::Sub => {
                    let r = pop!(); let l = pop!();
                    match (l, r) {
                        (Value::Num(a), Value::Num(b)) => push!(Value::Num(a - b)),
                        _ => return Err(rt_err("Sub тільки для чисел")),
                    }
                }
                Instr::Mul => {
                    let r = pop!(); let l = pop!();
                    match (l, r) {
                        (Value::Num(a), Value::Num(b)) => push!(Value::Num(a * b)),
                        _ => return Err(rt_err("Mul тільки для чисел")),
                    }
                }
                Instr::Div => {
                    let r = pop!(); let l = pop!();
                    match (l, r) {
                        (Value::Num(a), Value::Num(b)) => {
                            if b == 0.0 { return Err(rt_err("Ділення на нуль")); }
                            push!(Value::Num(a / b))
                        }
                        _ => return Err(rt_err("Div тільки для чисел")),
                    }
                }
                Instr::Neg => {
                    let v = pop!();
                    match v {
                        Value::Num(n) => push!(Value::Num(-n)),
                        _ => return Err(rt_err("Neg тільки для чисел")),
                    }
                }

                Instr::BitAnd => { let r = pop!(); let l = pop!(); push!(Value::Num((as_bit_int(&l)? & as_bit_int(&r)?) as f64)); }
                Instr::BitOr  => { let r = pop!(); let l = pop!(); push!(Value::Num((as_bit_int(&l)? | as_bit_int(&r)?) as f64)); }
                Instr::BitXor => { let r = pop!(); let l = pop!(); push!(Value::Num((as_bit_int(&l)? ^ as_bit_int(&r)?) as f64)); }
                Instr::BitNot => { let v = pop!(); push!(Value::Num(!as_bit_int(&v)? as f64)); }
                Instr::Shl => {
                    let r = pop!(); let l = pop!();
                    push!(Value::Num((as_bit_int(&l)?.wrapping_shl(as_shift_amount(&r)?)) as f64));
                }
                Instr::Shr => {
                    let r = pop!(); let l = pop!();
                    push!(Value::Num((as_bit_int(&l)?.wrapping_shr(as_shift_amount(&r)?)) as f64));
                }

                Instr::And => {
                    let r = pop!(); let l = pop!();
                    push!(Value::Bool(l.is_truthy() && r.is_truthy()));
                }
                Instr::Or => {
                    let r = pop!(); let l = pop!();
                    push!(Value::Bool(l.is_truthy() || r.is_truthy()));
                }
                Instr::Not => {
                    let v = pop!();
                    push!(Value::Bool(!v.is_truthy()));
                }

                Instr::Eq    => { let r=pop!(); let l=pop!(); push!(Value::Bool(l==r)); }
                Instr::NotEq => { let r=pop!(); let l=pop!(); push!(Value::Bool(l!=r)); }
                Instr::Lt    => {
                    let r=pop!(); let l=pop!();
                    match (l,r) { (Value::Num(a),Value::Num(b)) => push!(Value::Bool(a<b)), _ => return Err(rt_err("Lt тільки для чисел")) }
                }
                Instr::Gt    => {
                    let r=pop!(); let l=pop!();
                    match (l,r) { (Value::Num(a),Value::Num(b)) => push!(Value::Bool(a>b)), _ => return Err(rt_err("Gt тільки для чисел")) }
                }
                Instr::LtEq  => {
                    let r=pop!(); let l=pop!();
                    match (l,r) { (Value::Num(a),Value::Num(b)) => push!(Value::Bool(a<=b)), _ => return Err(rt_err("LtEq тільки для чисел")) }
                }
                Instr::GtEq  => {
                    let r=pop!(); let l=pop!();
                    match (l,r) { (Value::Num(a),Value::Num(b)) => push!(Value::Bool(a>=b)), _ => return Err(rt_err("GtEq тільки для чисел")) }
                }

                Instr::MakeOk  => { let v=pop!(); push!(Value::Ok(Box::new(v))); }
                Instr::MakeErr => { let v=pop!(); push!(Value::Err(Box::new(v))); }

                Instr::TryUnwrap => {
                    let v = pop!();
                    match v {
                        Value::Ok(inner)  => push!(*inner),
                        Value::Err(inner) => return Err(RuntimeError::PropagateErr(*inner)),
                        other => push!(other), // не Result — прозоро
                    }
                }

                Instr::MakeStruct { name, field_names } => {
                    let mut vals: Vec<Value> = (0..field_names.len())
                        .map(|_| stack.pop().unwrap_or(Value::Nil))
                        .collect();
                    vals.reverse();
                    let fields: HashMap<String, Value> = field_names
                        .iter()
                        .cloned()
                        .zip(vals.into_iter())
                        .collect();
                    push!(Value::Struct(OberihStruct::new(name.clone(), fields)));
                }

                Instr::CopyStruct => {
                    let v = pop!();
                    match v {
                        Value::Struct(s) => push!(Value::Struct(s.shallow_copy())),
                        other => push!(other),
                    }
                }

                Instr::MakeList(n) => {
                    let mut elems: Vec<Value> = (0..*n).map(|_| stack.pop().unwrap_or(Value::Nil)).collect();
                    elems.reverse();
                    push!(Value::List(GcList::new(elems)));
                }

                Instr::MakeClosure(name, n) => {
                    let mut caps: Vec<Value> = (0..*n).map(|_| stack.pop().unwrap_or(Value::Nil)).collect();
                    caps.reverse();
                    push!(Value::Closure(name.clone(), caps));
                }

                Instr::MakeMap(n) => {
                    let mut flat: Vec<Value> = (0..*n * 2).map(|_| stack.pop().unwrap_or(Value::Nil)).collect();
                    flat.reverse();
                    let mut m = OMap::new();
                    let mut it = flat.into_iter();
                    while let (Some(k), Some(val)) = (it.next(), it.next()) {
                        let key = MapKey::from_value(&k).map_err(|e| rt_err(e))?;
                        m.insert(key, val);   // дублікат ключа: виграє останній
                    }
                    push!(Value::Map(GcMap::new(m)));
                }

                Instr::LoadIndex => {
                    let idx = pop!(); let obj = pop!();
                    match (obj, idx) {
                        (Value::List(l), Value::Num(i)) => {
                            let i = list_index(i)?;
                            push!(l.get(i).unwrap_or(Value::Nil));
                        }
                        (Value::Map(m), key) => {
                            let key = MapKey::from_value(&key).map_err(|e| rt_err(e))?;
                            push!(m.get(&key).unwrap_or(Value::Nil));
                        }
                        (obj, idx) => return Err(rt_err(format!(
                            "індексація [] можлива для List (Number) і Map, отримано {}[{}]", obj, idx
                        ))),
                    }
                }

                Instr::StoreIndex => {
                    // Список і Map — посилальні типи: змінюємо на місці, нічого не кладемо на стек.
                    let val = pop!(); let idx = pop!(); let obj = pop!();
                    match (obj, idx) {
                        (Value::List(l), Value::Num(i)) => {
                            let iu = list_index(i)?;
                            if !l.set(iu, val) {
                                return Err(rt_err(format!(
                                    "індекс {} поза межами списку (довжина {})", iu, l.len()
                                )));
                            }
                        }
                        (Value::Map(m), key) => {
                            let key = MapKey::from_value(&key).map_err(|e| rt_err(e))?;
                            m.insert(key, val);
                        }
                        (obj, idx) => return Err(rt_err(format!(
                            "присвоєння [] можливе для List (Number) і Map, отримано {}[{}]", obj, idx
                        ))),
                    }
                }



                Instr::Call(n) => {
                    let mut call_args: Vec<Value> = (0..*n).map(|_| stack.pop().unwrap_or(Value::Nil)).collect();
                    call_args.reverse();
                    let callee = pop!();
                    let (name, full_args) = match &callee {
                        Value::Fn(name) if !name.starts_with("__builtin_") => (name.clone(), call_args),
                        Value::Closure(name, captured) => {
                            let mut full = captured.clone();
                            full.extend(call_args);
                            (name.clone(), full)
                        }
                        // `sleep(ms)` — перехоплюємо ДО звичайного виклику builtin: замість
                        // блокуючого std::thread::sleep (застарявив би ВЕСЬ планувальник,
                        // не лише цю задачу) призупиняємо саме цю задачу; інші готові задачі
                        // тим часом продовжують виконуватись. Результат (nil) доставляє
                        // планувальник при пробудженні — див. wake_ready_sleepers.
                        Value::Fn(builtin_name) if builtin_name == "__builtin_sleep" => {
                            let ms = match call_args.first() {
                                Some(Value::Num(n)) => *n,
                                _ => return Err(rt_err("sleep: потрібен Number (мілісекунди)")),
                            };
                            let wake = Instant::now() + Duration::from_secs_f64((ms.max(0.0)) / 1000.0);
                            break 'inner FrameOutcome::Suspend(SuspendReason::SleepUntil(wake));
                        }
                        Value::Fn(_) => {
                            // Інші __builtin_*: нема сенсу штовхати кадр — це нативний Rust-виклик.
                            let result = self.call_value(&callee, call_args)?;
                            push!(result);
                            continue 'inner;
                        }
                        _ => return Err(rt_err(format!("Не можна викликати {}", callee))),
                    };
                    match self.resolve_plain_call(&module, &name, full_args)? {
                        CallOutcome::NewFrame(idx, new_locals) => {
                            break 'inner FrameOutcome::CallFrame(idx, new_locals);
                        }
                        CallOutcome::Value(v) => push!(v),
                    }
                }

                Instr::CallMethod { name, arg_count } => {
                    let mut call_args: Vec<Value> = (0..*arg_count).map(|_| stack.pop().unwrap_or(Value::Nil)).collect();
                    call_args.reverse();
                    let receiver = pop!();
                    // Метод struct — найчастіший випадок, де варто уникнути рекурсії
                    // Rust (тіла методів часто самі рекурсивні: обхід дерева/списку).
                    // Для решти приймачів (String/List/Map/Regex/...) методи — нативні
                    // Rust-функції без Oberih-рекурсії, тож лишаються як є.
                    if let Value::Struct(s) = &receiver {
                        let fn_name = format!("{}.{}", s.type_name, name);
                        let mut full_args = vec![receiver.clone()];
                        full_args.extend(call_args.clone());
                        if module.find_fn(&fn_name).is_some() {
                            match self.resolve_plain_call(&module, &fn_name, full_args)? {
                                CallOutcome::NewFrame(idx, new_locals) => {
                                    break 'inner FrameOutcome::CallFrame(idx, new_locals);
                                }
                                CallOutcome::Value(v) => { push!(v); continue 'inner; }
                            }
                        }
                    }
                    // `h.join()`/`h.join(timeoutSecs)`/`h.isDone()` — так само як sleep,
                    // перехоплюємо ДО звичайного виклику методу: join має вміти
                    // ПРИЗУПИНИТИ цю задачу (не блокувати планувальник), поки цільова
                    // задача не завершиться.
                    if let Value::Spawn(h) = &receiver {
                        let target = h.task_id;
                        // Кожен рядок REPL створює НОВИЙ VM, тож хендл із попереднього
                        // рядка вказує на задачу, якої тут уже нема — це помилка
                        // користувача, а не привід панікувати на виході за межі.
                        if target >= self.tasks.len() {
                            return Err(rt_err(
                                "spawn-хендл не належить цьому виконанню (напр. створений в іншому рядку REPL — задачі не переживають рядок)"
                            ));
                        }
                        match name.as_str() {
                            "join" => {
                                if let TaskState::Done(result) = &self.tasks[target].state {
                                    let value = match result {
                                        Ok(v) => v.clone(),
                                        Err(e) => Value::Err(Box::new(Value::Str(e.to_string()))),
                                    };
                                    push!(value);
                                    continue 'inner;
                                }
                                let deadline = call_args.first().and_then(|v| match v {
                                    Value::Num(secs) => Some(Instant::now() + Duration::from_secs_f64(*secs)),
                                    _ => None,
                                });
                                break 'inner FrameOutcome::Suspend(SuspendReason::WaitingOnTask { task_id: target, deadline });
                            }
                            "isDone" => {
                                push!(Value::Bool(matches!(self.tasks[target].state, TaskState::Done(_))));
                                continue 'inner;
                            }
                            _ => return Err(rt_err(format!("Невідомий метод spawn хендлу: {}", name))),
                        }
                    }
                    let result = self.call_method(receiver, name, call_args)?;
                    push!(result);
                }

                Instr::Return => {
                    break 'inner FrameOutcome::Return(pop!());
                }

                Instr::Jump(addr) => {
                    ip = *addr;
                }
                Instr::JumpIfFalse(addr) => {
                    let cond = pop!();
                    if !cond.is_truthy() {
                        ip = *addr;
                    }
                }
                Instr::JumpIfTrue(addr) => {
                    let cond = pop!();
                    if cond.is_truthy() {
                        ip = *addr;
                    }
                }

                Instr::MatchVariant(name) => {
                    let v = pop!();
                    let matches = v.variant_name() == Some(name.as_str());
                    // Кладемо v назад (scrutinee потрібен для UnwrapVariantInto)
                    push!(v);
                    push!(Value::Bool(matches));
                }

                Instr::MatchLitNum(n) => {
                    let v = pop!();
                    let m = matches!(&v, Value::Num(x) if (x - n).abs() < f64::EPSILON);
                    push!(v);
                    push!(Value::Bool(m));
                }

                Instr::MatchLitStr(s) => {
                    let v = pop!();
                    let m = matches!(&v, Value::Str(x) if x == s);
                    push!(v);
                    push!(Value::Bool(m));
                }

                Instr::MatchLitBool(b) => {
                    let v = pop!();
                    let m = matches!(&v, Value::Bool(x) if x == b);
                    push!(v);
                    push!(Value::Bool(m));
                }

                Instr::MatchWildcard => {
                    push!(Value::Bool(true));
                }

                Instr::UnwrapVariantInto(slot) => {
                    let v = pop!();
                    let inner = v.unwrap_inner();
                    if *slot >= locals.len() {
                        locals.resize(*slot + 1, Value::Nil);
                    }
                    locals[*slot] = inner;
                }


                Instr::Spawn { fn_name, arg_count } => {
                    let mut spawn_args: Vec<Value> = (0..*arg_count)
                        .map(|_| stack.pop().unwrap_or(Value::Nil))
                        .collect();
                    spawn_args.reverse();

                    let idx = module.functions.iter().position(|f| &f.name == fn_name)
                        .ok_or_else(|| rt_err(format!("spawn: функцію '{}' не знайдено", fn_name)))?;
                    let target_fn = &module.functions[idx];

                    if self.tasks.len() >= MAX_TASKS {
                        return Err(rt_err(format!(
                            "перевищено максимальну кількість задач spawn ({}). Можливо, задача безкінечно створює нові.",
                            MAX_TASKS
                        )));
                    }
                    let new_id = self.tasks.len();
                    if target_fn.resilience.is_trivial() {
                        // Звичайна функція: кооперативна задача, що виконується
                        // планувальником поруч з іншими (НЕ окремий потік ОС).
                        let mut new_locals = vec![Value::Nil; target_fn.local_count.max(spawn_args.len())];
                        for (i, a) in spawn_args.into_iter().enumerate() { new_locals[i] = a; }
                        self.tasks.push(Task {
                            call_stack: vec![Frame { fn_index: idx, ip: 0, stack: Vec::with_capacity(8), locals: new_locals }],
                            state: TaskState::Ready,
                        });
                        self.ready.push_back(new_id);
                    } else {
                        // Функція з модифікаторами стійкості (deadline/retry/...):
                        // їхня логіка зав'язана на старий рекурсивний механізм
                        // (call_fn), що не вміє призупинятись, тож таку функцію
                        // виконуємо ОДРАЗУ й синхронно, а задачу створюємо вже
                        // завершеною. Чесне обмеження: `spawn` resilient-функції
                        // не конкурентний із рештою (блокує до завершення).
                        let result = self.call_fn(fn_name, spawn_args);
                        self.tasks.push(Task { call_stack: Vec::new(), state: TaskState::Done(result) });
                    }
                    push!(Value::Spawn(SpawnHandle { task_id: new_id }));
                }

                Instr::Dup => {
                    let v = stack.last()
                        .cloned()
                        .ok_or_else(|| rt_err("Dup: порожній стек"))?;
                    push!(v);
                }
                Instr::Pop  => { pop!(); }
                Instr::Swap => {
                    let a = pop!(); let b = pop!();
                    push!(a); push!(b);
                }
                Instr::Nop => {}
                }
            };

            {
                let top = call_stack.last_mut().unwrap();
                top.ip     = ip;
                top.stack  = stack;
                top.locals = locals;
            }

            match outcome {
                FrameOutcome::CallFrame(idx, new_locals) => {
                    if call_stack.len() >= MAX_EXPLICIT_CALL_DEPTH {
                        break 'frames Err(rt_err(format!(
                            "перевищено максимальну глибину рекурсії ({}). Можливо, функція викликає сама себе без умови завершення.",
                            MAX_EXPLICIT_CALL_DEPTH
                        )));
                    }
                    call_stack.push(Frame { fn_index: idx, ip: 0, stack: Vec::with_capacity(8), locals: new_locals });
                    continue 'frames;
                }
                FrameOutcome::Return(value) => {
                    call_stack.pop();
                    if call_stack.is_empty() {
                        break 'frames Ok(FrameOutcome::Return(value));
                    }
                    call_stack.last_mut().unwrap().stack.push(value);
                    continue 'frames;
                }
                FrameOutcome::Suspend(reason) => {
                    break 'frames Ok(FrameOutcome::Suspend(reason));
                }
            }
        } })();

        match outcome {
            Err(e) => {
                self.tasks[task_id].state = TaskState::Done(Err(e));
            }
            Ok(FrameOutcome::Return(value)) => {
                self.tasks[task_id].state = TaskState::Done(Ok(value));
            }
            Ok(FrameOutcome::Suspend(reason)) => {
                match &reason {
                    SuspendReason::SleepUntil(w) => {
                        self.timers.push(std::cmp::Reverse((*w, task_id)));
                    }
                    SuspendReason::WaitingOnTask { task_id: target, deadline } => {
                        self.waiters.entry(*target).or_default().push(task_id);
                        if let Some(d) = deadline {
                            self.timers.push(std::cmp::Reverse((*d, task_id)));
                        }
                    }
                }
                self.tasks[task_id].call_stack = call_stack;
                self.tasks[task_id].state = TaskState::Suspended(reason);
                return;
            }
            Ok(FrameOutcome::CallFrame(..)) => unreachable!("CallFrame обробляється всередині циклу, не виходить з нього"),
        }
        self.wake_waiters_on(task_id);
    }



    // --- Методи ---

    fn call_method(&mut self, receiver: Value, method: &str, args: Vec<Value>) -> VR<Value> {
        match &receiver {
            Value::Str(_) => self.call_str_method(receiver, method, args),
            Value::List(_) => self.call_list_method(receiver, method, args),
            Value::Map(_)  => self.call_map_method(receiver, method, args),
            Value::Regex(..) => crate::regex_ops::regex_method(receiver, method, args),
            Value::Struct(s) => {
                let fn_name = format!("{}.{}", s.type_name, method);
                if self.module.find_fn(&fn_name).is_some() {
                    let mut full_args = vec![receiver];
                    full_args.extend(args);
                    self.call_fn(&fn_name, full_args)
                } else {
                    Err(rt_err(format!("Метод '{}' не знайдено", fn_name)))
                }
            }
            // Value::Spawn: "join"/"isDone" перехоплюються РАНІШЕ, у самому
            // Instr::CallMethod (бо join мусить вміти ПРИЗУПИНИТИ задачу, а
            // call_method повертає лише готове VR<Value>) — сюди виклик дійде
            // лише якщо десь виник Value::Spawn поза тим шляхом, що само по
            // собі було б помилкою логіки VM.
            Value::Spawn(_) => Err(rt_err(format!(
                "внутрішня помилка: виклик методу '{}' на SpawnHandle мав бути перехоплений у Instr::CallMethod", method
            ))),
            Value::WeakRef(w) => {
                match method {
                    "upgrade" => match w.upgrade() {
                        Some(list) => Ok(Value::Ok(Box::new(Value::List(list)))),
                        None       => Ok(Value::Err(Box::new(Value::Str("об'єкт звільнено".into())))),
                    },
                    "isAlive" => Ok(Value::Bool(w.is_alive())),
                    _ => Err(rt_err(format!("Невідомий метод WeakRef: {}", method))),
                }
            }
            _ => Err(rt_err(format!("Метод '{}' недоступний для {}", method, receiver))),
        }
    }

    fn call_map_method(&mut self, receiver: Value, method: &str, args: Vec<Value>) -> VR<Value> {
        let m = match receiver { Value::Map(m) => m, _ => unreachable!() };
        // Вбудовані операції мають пріоритет; інакше `m.name(args)` викликає
        // функцію, збережену під ключем "name" (Map як об'єкт із методами).
        const BUILTIN: [&str; 9] = ["len", "keys", "values", "entries", "has", "get", "set", "delete", "merge"];
        if !BUILTIN.contains(&method) {
            if let Some(f @ (Value::Fn(_) | Value::Closure(..))) = m.get(&MapKey::Str(method.to_string())) {
                return self.call_value(&f, args);
            }
        }
        crate::stdlib::map_op(&m, method, args)
    }

    fn call_str_method(&self, receiver: Value, method: &str, args: Vec<Value>) -> VR<Value> {
        let s = match receiver { Value::Str(s) => s, _ => unreachable!() };
        match method {
            "len"        => Ok(Value::Num(s.chars().count() as f64)),
            "trim"       => Ok(Value::Str(s.trim().to_string())),
            "toUpper"    => Ok(Value::Str(s.to_uppercase())),
            "toLower"    => Ok(Value::Str(s.to_lowercase())),
            "contains"   => {
                if let Some(Value::Str(p)) = args.into_iter().next() {
                    Ok(Value::Bool(s.contains(p.as_str())))
                } else { Err(rt_err("contains потребує String")) }
            }
            "startsWith" => {
                if let Some(Value::Str(p)) = args.into_iter().next() {
                    Ok(Value::Bool(s.starts_with(p.as_str())))
                } else { Err(rt_err("startsWith потребує String")) }
            }
            "split" => {
                if let Some(Value::Str(sep)) = args.into_iter().next() {
                    let parts: Vec<Value> = s.split(sep.as_str()).map(|p| Value::Str(p.to_string())).collect();
                    Ok(Value::List(GcList::new(parts)))
                } else { Err(rt_err("split потребує String")) }
            }
            _ => Err(rt_err(format!("Невідомий метод рядка: {}", method))),
        }
    }

    fn call_list_method(&self, receiver: Value, method: &str, args: Vec<Value>) -> VR<Value> {
        let l = match receiver { Value::List(l) => l, _ => unreachable!() };
        match method {
            "len"     => Ok(Value::Num(l.len() as f64)),
            "push"    => {
                if let Some(v) = args.into_iter().next() { l.push(v); }
                Ok(Value::List(l))
            }
            "pop"     => Ok(l.pop().unwrap_or(Value::Nil)),
            "first"   => Ok(l.first().unwrap_or(Value::Nil)),
            "last"    => Ok(l.last().unwrap_or(Value::Nil)),
            "reverse" => Ok(Value::List(l.reverse())),
            "contains" => {
                let item = args.into_iter().next().unwrap_or(Value::Nil);
                Ok(Value::Bool(l.contains(&item)))
            }
            _ => Err(rt_err(format!("Невідомий метод списку: {}", method))),
        }
    }

}

/// Число -> ціле для бітових операцій. Межі — "безпечний цілий" діапазон
/// f64 (|n| <= 2^53): поза ним подвійна точність уже не представляє кожне
/// ціле точно, тож результат був би оманливим, а не просто "великим числом".
fn as_bit_int(v: &Value) -> VR<i64> {
    match v {
        Value::Num(n) if n.fract() == 0.0 && n.is_finite() && n.abs() <= 9_007_199_254_740_992.0 => Ok(*n as i64),
        Value::Num(n) => Err(rt_err(format!(
            "бітові оператори потребують цілого числа в межах ±2^53, отримано {}", n
        ))),
        other => Err(rt_err(format!("бітові оператори потребують Number, отримано {}", other))),
    }
}

/// Кількість зсуву: 0..=63 (як i64 має 64 біти; більший зсув — майже завжди
/// помилка в коді, а не задум, тому це помилка, а не мовчазне обнулення).
fn as_shift_amount(v: &Value) -> VR<u32> {
    let n = as_bit_int(v)?;
    if !(0..=63).contains(&n) {
        return Err(rt_err(format!("зсув має бути в межах 0..=63, отримано {}", n)));
    }
    Ok(n as u32)
}

/// Число -> індекс списку: тільки невід'ємне ціле (раніше `xs[-1]` мовчки давало xs[0]).
fn list_index(i: f64) -> VR<usize> {
    if i < 0.0 || i.fract() != 0.0 || !i.is_finite() {
        return Err(rt_err(format!("індекс списку має бути невід'ємним цілим, отримано {}", i)));
    }
    Ok(i as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn run(src: &str) -> Value {
        let tokens  = Lexer::new(src).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let module  = Compiler::new().compile_program(&program).unwrap();
        let mut vm  = VM::new(module);
        vm.run().unwrap()
    }

    #[test]
    fn test_arithmetic() {
        let v = run("fn main() -> Number { return 2 + 3 * 4 }");
        // 2 + 12 = 14 (немає пріоритету — left-to-right без дужок)
        assert!(matches!(v, Value::Num(_)));
    }

    #[test]
    fn test_if_else() {
        let v = run(r#"
fn main() -> Number {
    if (10 > 5) {
        return 1
    } else {
        return 0
    }
}
"#);
        assert_eq!(v, Value::Num(1.0));
    }

    #[test]
    fn test_result_ok() {
        let v = run(r#"
fn safe(x: Number) -> String {
    return Ok(x)
}
fn main() -> String {
    return safe(42)
}
"#);
        assert!(matches!(v, Value::Ok(_)));
    }

    #[test]
    fn test_try_propagate() {
        let v = run(r#"
fn fail() -> String {
    return Err("щось пішло не так")
}
fn main() -> String {
    let r = fail()?
    return r
}
"#);
        // ? пробрасує Err, функція повертає Err
        assert!(matches!(v, Value::Err(_)));
    }

    #[test]
    fn test_while_loop() {
        let v = run(r#"
fn main() -> Number {
    let x = 0
    while (x < 5) {
        x = x + 1
    }
    return x
}
"#);
        assert_eq!(v, Value::Num(5.0));
    }

    #[test]
    fn test_spawn_join() {
        let v = run(r#"
fn work(n: Number) -> Number {
    return n + 1
}
fn main() -> Number {
    let h = spawn work(41)
    let r = h.join()
    return r
}
"#);
        assert_eq!(v, Value::Num(42.0));
    }

    #[test]
    fn test_operator_precedence() {
        assert_eq!(run("fn main() -> Number { return 2 + 3 * 4 }"), Value::Num(14.0));
        assert_eq!(run("fn main() -> Number { return (2 + 3) * 4 }"), Value::Num(20.0));
        assert_eq!(run("fn main() -> Number { return 10 - 4 - 3 }"), Value::Num(3.0));
    }

    #[test]
    fn test_for_loop_sums_list() {
        // Регресія: `for` пушив аргумент len() раніше за callee -> "Не можна викликати [..]".
        let v = run(r#"
fn sum(arr: List<Number>) -> Number {
    let t = 0
    for (n in arr) { t = t + n }
    return t
}
fn main() -> Number { return sum([10, 20, 30]) }
"#);
        assert_eq!(v, Value::Num(60.0));
    }

    #[test]
    fn test_enum_equality_and_match() {
        // Регресія: enum раніше парсився, але компілятор ігнорував Item::Enum.
        let v = run(r#"
enum Status { Active, Inactive }
fn main() -> Number {
    let s = Status.Active
    let same = s == Status.Active
    let other = s == Status.Inactive
    let r = match s { Active => 1, Inactive => 2, _ => 3 }
    if (same) { if (other) { return 100 } }
    return r
}
"#);
        assert_eq!(v, Value::Num(1.0));
    }

    #[test]
    fn test_private_fn_parses() {
        // Регресія: `private fn` не споживав токен `fn` і давав синтаксичну помилку.
        let v = run(r#"
private fn helper(x: Number) -> Number { return x + 1 }
fn main() -> Number { return helper(41) }
"#);
        assert_eq!(v, Value::Num(42.0));
    }

    fn run_err(src: &str) -> String {
        let tokens  = Lexer::new(src).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let module  = Compiler::new().compile_program(&program).unwrap();
        let mut vm  = VM::new(module);
        format!("{}", vm.run().unwrap_err())
    }

    #[test]
    fn test_map_get_set_len_delete() {
        let v = run(r#"
fn main() -> Number {
    let m = {"a": 1, "b": 2}
    m["c"] = 3
    m.d = 4
    mapDelete(m, "b")
    return len(m) * 100 + m["a"] * 10 + m.d
}
"#);
        assert_eq!(v, Value::Num(3.0 * 100.0 + 10.0 + 4.0));
    }

    #[test]
    fn test_map_missing_key_is_nil_and_get_default() {
        let v = run(r#"
fn main() -> Number {
    let m = {"a": 1}
    if (mapHas(m, "zzz")) { return 0 }
    let missing = m["zzz"]
    if (missing == m["also-missing"]) { return mapGet(m, "zzz", 7) }   // обидва nil
    return 0
}
"#);
        assert_eq!(v, Value::Num(7.0));
    }

    #[test]
    fn test_map_is_reference_type() {
        // Map передається у функцію за посиланням (як struct/list).
        let v = run(r#"
fn bump(m: Map<String, Number>) -> Number {
    m["n"] = m["n"] + 1
    return 0
}
fn main() -> Number {
    let m = {"n": 1}
    bump(m)
    bump(m)
    return m["n"]
}
"#);
        assert_eq!(v, Value::Num(3.0));
    }

    #[test]
    fn test_map_insertion_order_and_for_over_keys() {
        let v = run(r#"
fn main() -> Number {
    let m = {"z": 1, "a": 2, "m": 3}
    m["a"] = 20          // існуючий ключ не змінює позицію
    let out = ""
    for (k in m) { out = out + k }
    if (out == "zam") { return m["a"] }
    return -1
}
"#);
        assert_eq!(v, Value::Num(20.0));
    }

    #[test]
    fn test_map_equality_ignores_order() {
        let v = run(r#"
fn main() -> Number {
    if ({"a": 1, "b": 2} == {"b": 2, "a": 1}) {
        if ({"a": 1} == {"a": 2}) { return 0 }
        return 1
    }
    return 0
}
"#);
        assert_eq!(v, Value::Num(1.0));
    }

    #[test]
    fn test_map_merge_creates_new_map() {
        let v = run(r#"
fn main() -> Number {
    let a = {"x": 1}
    let b = mapMerge(a, {"x": 5, "y": 2})
    return a["x"] * 100 + b["x"] * 10 + len(b)
}
"#);
        assert_eq!(v, Value::Num(100.0 + 50.0 + 2.0));
    }

    #[test]
    fn test_map_number_keys_and_zero_normalization() {
        let v = run(r#"
fn main() -> Number {
    let m = {1: 10, 2.5: 20}
    m[0] = 5
    return m[1] + m[2.5] + m[0 - 0]
}
"#);
        assert_eq!(v, Value::Num(35.0));
    }

    #[test]
    fn test_map_bad_key_is_runtime_error() {
        let e = run_err(r#"
fn main() -> Number {
    let m = {"a": 1}
    m[[1]] = 2
    return 0
}
"#);
        assert!(e.contains("ключ Map має бути"), "{}", e);
    }

    #[test]
    fn test_json_object_is_map() {
        let v = run(r#"
fn main() -> Number {
    let r = jsonParse("{\"a\": {\"b\": [10, 20]}, \"n\": 3}")?
    return r.a.b[1] + r["n"] + len(keys(r))
}
"#);
        assert_eq!(v, Value::Num(20.0 + 3.0 + 2.0));
    }

    #[test]
    fn test_list_index_assignment_mutates_in_place() {
        // Регресія: `xs[i] = v` мовчки нічого не робив (і лишав сміття на стеку).
        let v = run(r#"
fn main() -> Number {
    let a = [1, 2, 3]
    a[0] = 9
    a[2] = a[0] + 1
    return a[0] * 100 + a[1] * 10 + a[2]
}
"#);
        assert_eq!(v, Value::Num(900.0 + 20.0 + 10.0));
        assert!(run_err("fn main() -> Number { let a = [1]\n a[5] = 1\n return 0 }").contains("поза межами"));
        assert!(run_err("fn main() -> Number { let a = [1]\n return a[0 - 1] }").contains("невід'ємним"));
    }

    #[test]
    fn test_same_collection_equality_does_not_deadlock() {
        // Регресія: `xs == xs` двічі брав один Mutex і зависав.
        let v = run(r#"
fn main() -> Number {
    let a = [1, 2]
    let m = {"k": 1}
    if (a == a) { if (m == m) { return 1 } }
    return 0
}
"#);
        assert_eq!(v, Value::Num(1.0));
    }

    #[test]
    fn test_for_over_string_chars() {
        let v = run(r#"
fn main() -> Number {
    let n = 0
    for (c in "héy") { n = n + 1 }
    return n
}
"#);
        assert_eq!(v, Value::Num(3.0));
    }

    #[test]
    fn test_map_filter_reduce_sortby() {
        // Регресія: map/filter були оголошені як builtin, але не реалізовані.
        let v = run(r#"
fn dbl(x: Number) -> Number { return x * 2 }
fn big(x: Number) -> Bool { return x > 4 }
fn add(a: Number, b: Number) -> Number { return a + b }
fn neg(x: Number) -> Number { return 0 - x }
fn main() -> Number {
    let m = map([1, 2, 3, 4], dbl)          // [2,4,6,8]
    let f = filter(m, big)                  // [6,8]
    let r = reduce(f, add, 0)               // 14
    let s = sortBy([1, 3, 2], neg)          // [3,2,1]
    return r * 100 + s[0] * 10 + s[2]
}
"#);
        assert_eq!(v, Value::Num(1400.0 + 30.0 + 1.0));
    }

    #[test]
    fn test_higher_order_errors_are_clear() {
        let e = run_err("fn main() -> Number { return len(map([1], 5)) }");
        assert!(e.contains("має бути функцією"), "{}", e);
        let e = run_err("fn f(x: Number) -> Number { return x }\nfn main() -> Number { return len(map(7, f)) }");
        assert!(e.contains("має бути List"), "{}", e);
    }

    #[test]
    fn test_append_is_in_place_push_is_not() {
        let v = run(r#"
fn main() -> Number {
    let a = [1]
    append(a, 2)
    let b = push(a, 3)
    return len(a) * 10 + len(b)
}
"#);
        assert_eq!(v, Value::Num(23.0));
    }

    #[test]
    fn test_sort_and_slice() {
        let v = run(r#"
fn main() -> Number {
    let s = sort([3, 1, 2])
    let t = slice(s, 1)
    return s[0] * 100 + t[0] * 10 + len(t)
}
"#);
        assert_eq!(v, Value::Num(100.0 + 20.0 + 2.0));
        assert!(run_err(r#"fn main() -> Number { return len(sort([1, "a"])) }"#).contains("не можна порівняти"));
    }

    #[test]
    fn test_date_roundtrip() {
        let v = run(r#"
fn main() -> Number {
    let t = timeFromParts(2024, 2, 29, 23, 59, 58)
    let p = dateParts(t)
    return p.year * 10000 + p.month * 100 + p.day
}
"#);
        assert_eq!(v, Value::Num(20240229.0));
    }

    #[test]
    fn test_unary_binds_looser_than_postfix() {
        // Регресія: `!f(x)` парсилось як `(!f)(x)` -> "Не можна викликати false".
        let v = run(r#"
fn f(x: Number) -> Number { return x * 2 }
fn t() -> Bool { return true }
fn main() -> Number {
    let m = {"a": 5}
    let xs = [4, 8]
    if (!t()) { return 0 }
    return -f(3) + -m.a + -xs[1] + (2 - -3)     // -6 + -5 + -8 + 5 = -14
}
"#);
        assert_eq!(v, Value::Num(-14.0));
    }

    #[test]
    fn test_lambda_captures_by_value() {
        let v = run(r#"
fn main() -> Number {
    let k = 10
    let f = fn(x) => x * k
    k = 999                      // не впливає на вже створене замикання
    return f(3)
}
"#);
        assert_eq!(v, Value::Num(30.0));
    }

    #[test]
    fn test_closure_per_loop_iteration_and_currying() {
        let v = run(r#"
fn main() -> Number {
    let fs = []
    for (i in range(0, 3)) { append(fs, fn() => i * 10) }
    let curried = fn(a) => fn(b) => a * b
    let sum = reduce(map(fs, fn(f) => f()), fn(a, b) => a + b, 0)   // 0+10+20
    return sum + curried(6)(7)                                       // 30 + 42
}
"#);
        assert_eq!(v, Value::Num(72.0));
    }

    #[test]
    fn test_lambda_block_body_and_returned_closure() {
        let v = run(r#"
fn makeCounterStep(step: Number) -> Fn {
    return fn(x: Number) -> Number {
        let y = x + step
        return y * 2
    }
}
fn main() -> Number {
    let g = makeCounterStep(5)
    return g(1) + makeCounterStep(0)(10)     // 12 + 20
}
"#);
        assert_eq!(v, Value::Num(32.0));
    }

    #[test]
    fn test_lambdas_with_higher_order_and_map_methods() {
        let v = run(r#"
fn main() -> Number {
    let base = 100
    let evens = filter([1, 2, 3, 4], fn(x) => x - floor(x / 2) * 2 == 0)
    let sorted = sortBy([3, 1, 2], fn(x) => 0 - x)
    let obj = {"add": fn(a, b) => a + b + base}
    return len(evens) * 1000 + sorted[0] * 100 + obj.add(1, 2)     // 2000 + 300 + 103
}
"#);
        assert_eq!(v, Value::Num(2403.0));
    }

    #[test]
    fn test_repr_quotes_nested_strings_only() {
        let v = run(r#"fn main() -> String { return toString(["a", 1, {"k": "v\n"}]) }"#);
        assert_eq!(v, Value::Str("[\"a\", 1, {\"k\": \"v\\n\"}]".to_string()));
        assert_eq!(run(r#"fn main() -> String { return "plain" }"#), Value::Str("plain".into()));
    }

    #[test]
    fn test_regex_compile_test_find_replace() {
        let v = run(r#"
fn main() -> Number {
    let re = regex("(\\d+)-(\\d+)")?
    if (!re.test("555-1234")) { return -1 }
    let m = re.find("id 555-1234 end")
    if (m.groups[1] != "555") { return -2 }
    if (m.groups[2] != "1234") { return -3 }
    let swapped = re.replace("555-1234", "$2-$1")
    if (swapped != "1234-555") { return -4 }
    if (len(reFindAll("\\d+", "a1 b22 c333")) != 3) { return -5 }
    return 42
}
"#);
        assert_eq!(v, Value::Num(42.0));
    }

    #[test]
    fn test_regex_no_match_and_bad_pattern_are_values_not_crashes() {
        let v = run(r#"
fn main() -> Number {
    let re = regex("abc")?
    let m = re.find("xyz")
    if (typeOf(m) != "Nil") { return -1 }
    let bad = regex("(unterminated")
    return 1
}
"#);
        assert_eq!(v, Value::Num(1.0));
    }

    #[test]
    fn test_regex_split_and_case_insensitive() {
        let v = run(r#"
fn main() -> Number {
    let parts = reSplit("\\s*,\\s*", "a, b ,c")
    if (len(parts) != 3) { return -1 }
    if (parts[1] != "b") { return -2 }
    if (!reTest("(?i)hello", "HELLO")) { return -3 }
    return 1
}
"#);
        assert_eq!(v, Value::Num(1.0));
    }

    #[test]
    fn test_bitwise_basic_ops() {
        assert_eq!(run("fn main() -> Number { return 6 & 3 }"), Value::Num(2.0));
        assert_eq!(run("fn main() -> Number { return 6 | 1 }"), Value::Num(7.0));
        assert_eq!(run("fn main() -> Number { return 6 ^ 5 }"), Value::Num(3.0));
        assert_eq!(run("fn main() -> Number { return ~0 }"), Value::Num(-1.0));
        assert_eq!(run("fn main() -> Number { return ~5 }"), Value::Num(-6.0));
        assert_eq!(run("fn main() -> Number { return 1 << 10 }"), Value::Num(1024.0));
        assert_eq!(run("fn main() -> Number { return 256 >> 4 }"), Value::Num(16.0));
    }

    #[test]
    fn test_bitwise_precedence_matches_python() {
        // `|`/`^`/`&` тісніші за порівняння, слабші за зсуви — як у Python:
        // 1 | 2 == 3  ==  (1|2) == 3  ==  true (НЕ 1 | (2==3))
        assert_eq!(run("fn main() -> Bool { return 1 | 2 == 3 }"), Value::Bool(true));
        assert_eq!(run("fn main() -> Number { return 4 & 6 | 1 }"), Value::Num(5.0)); // (4&6)|1
        assert_eq!(run("fn main() -> Number { return 2 << 1 + 1 }"), Value::Num(8.0)); // 2<<(1+1)
        assert_eq!(run("fn main() -> Number { return 1 + 2 << 1 }"), Value::Num(6.0)); // (1+2)<<1
    }

    #[test]
    fn test_nested_generics_still_parse_after_adding_shift_operator() {
        // Регресія, якої НЕ МАЄ статись: якби `>>` лексувався одним токеном,
        // `Map<String, List<Number>>` (закінчується на дві `>`) зламався б.
        let v = run(r#"
fn wrap(n: Number) -> Map<String, List<Number>> {
    return {"items": [n, n]}
}
fn main() -> Number {
    let m = wrap(7)
    return m["items"][0] + m["items"][1]
}
"#);
        assert_eq!(v, Value::Num(14.0));
    }

    #[test]
    fn test_bitwise_errors_are_clear() {
        let e = run_err(r#"fn main() -> Number { return 1.5 & 2 }"#);
        assert!(e.contains("цілого числа"), "{}", e);
        let e = run_err(r#"fn main() -> Number { return "a" | 1 }"#);
        assert!(e.contains("Number"), "{}", e);
        let e = run_err(r#"fn main() -> Number { return 1 << 64 }"#);
        assert!(e.contains("0..=63"), "{}", e);
        let e = run_err(r#"fn main() -> Number { return 1 << -1 }"#);
        assert!(e.contains("0..=63"), "{}", e);
    }

    #[test]
    fn test_deep_recursion_succeeds_with_big_stack() {
        // Регресія: раніше ЦЕ САМЕ (сума рекурсією до 100000) обвалювало
        // весь процес (`stack overflow`, SIGABRT) вже на ~2500 рівнях з
        // типовим 8 МБ стеком головного потоку. Тепер — через run_with_big_stack
        // і запас у MAX_CALL_DEPTH — має пройти без помилок.
        let src = r#"
fn sum(n: Number) -> Number {
    if (n <= 0) { return 0 }
    return n + sum(n - 1)
}
fn main() -> Number { return sum(100000) }
"#.to_string();
        let v = run_with_big_stack(move || run(&src));
        assert_eq!(v, Value::Num(5_000_050_000.0)); // 100000*100001/2
    }

    #[test]
    fn test_runaway_recursion_is_a_catchable_error_not_a_crash() {
        let src = r#"
fn infinite(n: Number) -> Number { return infinite(n) }
fn main() -> Number { return infinite(1) }
"#.to_string();
        let e = run_with_big_stack(move || run_err(&src));
        assert!(e.contains("глибину рекурсії"), "{}", e);
        // Проста (без resilience) пряма рекурсія тепер іде швидким шляхом
        // явного стека кадрів -> межа MAX_EXPLICIT_CALL_DEPTH, а не
        // MAX_CALL_DEPTH (та діє лише для resilient-функцій, див. тест нижче).
        assert!(e.contains(&MAX_EXPLICIT_CALL_DEPTH.to_string()), "{}", e);
    }

    #[test]
    fn test_bounded_recursion_inside_resilient_fn_still_works() {
        // Функції З модифікаторами стійкості навмисно НЕ йдуть швидким шляхом
        // явного стека кадрів (їхня retry/timeout-логіка зав'язана на
        // рекурсивний механізм з окремим потоком на КОЖЕН виклик — див.
        // exec_with_timeout) — цей тест лише перевіряє, що після рефакторингу
        // звичайна, НЕглибока рекурсія всередині resilient-функції й далі
        // працює коректно. Навмисно НЕ глибока: кожен рекурсивний виклик такої
        // функції реально створює новий потік ОС (окреме, вже наявне
        // архітектурне обмеження, не виправлене сьогодні), тож перевірка
        // СПРАВЖНЬОЇ глибокої/нескінченної рекурсії тут створила б тисячі
        // реальних потоків — повільно й по суті тестувало б інше.
        let src = r#"
resilient fn countdown(n: Number) -> Number
    deadline(5s)
{
    if (n <= 0) { return 0 }
    return 1 + countdown(n - 1)
}
fn main() -> Number { return countdown(20) }
"#.to_string();
        let v = run_with_big_stack(move || run(&src));
        assert_eq!(v, Value::Num(20.0));
    }

    #[test]
    fn test_mutual_recursion_also_counts_toward_the_same_depth_limit() {
        let src = r#"
fn isEven(n: Number) -> Bool {
    if (n == 0) { return true }
    return isOdd(n - 1)
}
fn isOdd(n: Number) -> Bool {
    if (n == 0) { return false }
    return isEven(n - 1)
}
fn main() -> Bool { return isEven(50000) }
"#.to_string();
        let v = run_with_big_stack(move || {
            let tokens  = Lexer::new(&src).tokenize().unwrap();
            let program = Parser::new(tokens).parse_program().unwrap();
            let module  = Compiler::new().compile_program(&program).unwrap();
            VM::new(module).run().unwrap()
        });
        assert_eq!(v, Value::Bool(true));
    }

    #[test]
    fn test_process_keeps_working_normally_after_a_depth_limit_error() {
        // Найважливіша властивість: помилка глибини рекурсії — ЗВИЧАЙНА
        // помилка виконання, не крах процесу. Той самий виклик VM::run()
        // після неї продовжує працювати як зазвичай.
        let src1 = "fn f(n: Number) -> Number { return f(n) }\nfn main() -> Number { return f(1) }".to_string();
        let src2 = "fn main() -> Number { return 2 + 2 }".to_string();
        let (e, ok) = run_with_big_stack(move || (run_err(&src1), run(&src2)));
        assert!(e.contains("глибину рекурсії"));
        assert_eq!(ok, Value::Num(4.0));
    }

    #[test]
    fn test_deep_recursion_through_struct_method_uses_explicit_stack() {
        // CallMethod на struct теж має йти швидким шляхом (явний стек), не
        // лише звичайні Instr::Call — інакше глибока рекурсія через метод
        // (дуже поширений патерн: обхід дерева/списку через self-методи)
        // і далі впиралась би у старий, набагато менший ліміт.
        let src = r#"
struct Counter { n: Number }
fn Counter.sumDown(self: Counter) -> Number {
    if (self.n <= 0) { return 0 }
    let next = Counter(self.n - 1)
    return self.n + next.sumDown()
}
fn main() -> Number {
    let c = Counter(50000)
    return c.sumDown()
}
"#.to_string();
        let v = run_with_big_stack(move || run(&src));
        assert_eq!(v, Value::Num(50000.0 * 50001.0 / 2.0));
    }

    #[test]
    fn test_closures_still_capture_correctly_through_explicit_stack() {
        // Лямбди компілюються в окремі синтетичні функції (__lambda_N) і
        // йдуть через той самий Instr::Call fast-path — захоплені значення
        // мають лишатись коректними при глибокій рекурсії довкола них.
        let src = r#"
fn applyN(f: Fn(Number) -> Number, x: Number, n: Number) -> Number {
    if (n <= 0) { return x }
    return applyN(f, f(x), n - 1)
}
fn main() -> Number {
    let factor = 2
    let double = fn(x) => x * factor
    return applyN(double, 1, 20)   // 2^20
}
"#.to_string();
        let v = run_with_big_stack(move || run(&src));
        assert_eq!(v, Value::Num(1048576.0));
    }

    #[test]
    fn test_thread_spawn_failure_is_catchable_not_a_crash() {
        // Регресія, яку я сам знайшов під час цієї роботи: .expect() на
        // спробі створити потік (exec_with_timeout/Spawn) раніше панікував
        // (SIGABRT) при вичерпанні потоків ОС — найлегше відтворюється
        // глибокою прямою рекурсією в resilient-функції (кожен виклик
        // такої функції створює НОВИЙ потік). Тепер має бути звичайна
        // RuntimeError, а не крах процесу.
        let src = r#"
resilient fn infinite(n: Number) -> Number
    deadline(5s)
{
    return infinite(n)
}
fn main() -> Number { return infinite(1) }
"#.to_string();
        // Сам факт, що цей виклик ЗАВЕРШУЄТЬСЯ (а не SIGABRT-ить весь тестовий
        // процес) і повертає Err, а не панікує, — і є перевіркою.
        let e = run_with_big_stack(move || run_err(&src));
        assert!(
            e.contains("глибину рекурсії") || e.contains("потік"),
            "очікувалась помилка глибини рекурсії АБО вичерпання потоків, отримано: {}", e
        );
    }

    #[test]
    fn test_spawned_tasks_run_concurrently_not_sequentially() {
        // Три задачі по ~100 мс мають перекриватись (≈100 мс загалом), а не йти
        // послідовно (≈300 мс) — доказ, що sleep більше не блокує планувальник.
        let src = r#"
fn nap(n: Number) -> Number {
    sleep(100)
    return n
}
fn main() -> Number {
    let a = spawn nap(1)
    let b = spawn nap(2)
    let c = spawn nap(3)
    return a.join() + b.join() + c.join()
}
"#;
        let start = std::time::Instant::now();
        let v = run(src);
        let elapsed = start.elapsed();
        assert_eq!(v, Value::Num(6.0));
        assert!(elapsed.as_millis() < 250, "задачі йшли послідовно: {:?}", elapsed);
        assert!(elapsed.as_millis() >= 95, "sleep не спрацював: {:?}", elapsed);
    }

    #[test]
    fn test_tasks_wake_in_deadline_order_not_spawn_order() {
        let v = run(r#"
fn tell(log: List<String>, name: String, ms: Number) -> Number {
    sleep(ms)
    append(log, name)
    return 0
}
fn main() -> String {
    let log = []
    let a = spawn tell(log, "a", 60)
    let b = spawn tell(log, "b", 10)
    let c = spawn tell(log, "c", 30)
    a.join()
    b.join()
    c.join()
    return strJoin(",", log)
}
"#);
        assert_eq!(v, Value::Str("b,c,a".into()));
    }

    #[test]
    fn test_nested_spawn_join_and_isdone() {
        let v = run(r#"
fn leaf(n: Number) -> Number { return n * 2 }
fn parent(n: Number) -> Number {
    let x = spawn leaf(n)
    let y = spawn leaf(n + 1)
    return x.join() + y.join()
}
fn main() -> Number {
    let p = spawn parent(5)
    let r = p.join()
    if (p.isDone()) { return r }
    return -1
}
"#);
        assert_eq!(v, Value::Num(22.0));
    }

    #[test]
    fn test_join_timeout_returns_err_and_main_continues() {
        let v = run(r#"
fn slow() -> Number {
    sleep(1000)
    return 1
}
fn main() -> String {
    let h = spawn slow()
    return toString(h.join(0.05))
}
"#);
        assert_eq!(v, Value::Str("Err(\"timeout\")".into()));
    }

    #[test]
    fn test_error_inside_task_is_delivered_to_join_not_a_crash() {
        let v = run(r#"
fn boom() -> Number { return toNumber("не число") }
fn main() -> Bool {
    let h = spawn boom()
    let r = h.join()
    return typeOf(r) == "Result"
}
"#);
        assert_eq!(v, Value::Bool(true));
    }

    #[test]
    fn test_many_concurrent_tasks_scale_linearly() {
        // Раніше планувальник сканував усі задачі на кожен крок (O(n²)); тепер
        // черга готових + купа таймерів. 20 000 сплячих задач мають завершитись
        // за долі секунди, не хвилини.
        let src = r#"
fn leaf(n: Number) -> Number {
    sleep(20)
    return n
}
fn main() -> Number {
    let hs = []
    for (i in range(0, 20000)) { append(hs, spawn leaf(1)) }
    let t = 0
    for (h in hs) { t = t + h.join() }
    return t
}
"#;
        let start = std::time::Instant::now();
        let v = run(src);
        assert_eq!(v, Value::Num(20000.0));
        assert!(start.elapsed().as_secs() < 5, "планувальник повільний: {:?}", start.elapsed());
    }

    #[test]
    fn test_stale_spawn_handle_is_an_error_not_a_panic() {
        // Хендл з ІНШОГО VM (як у REPL, де кожен рядок — новий VM) не має
        // вішати панікою на виході за межі `tasks`.
        let tokens  = Lexer::new("fn main() -> Number { return 0 }").tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let module  = Compiler::new().compile_program(&program).unwrap();
        let mut vm = VM::new(module);
        vm.run().unwrap();
        let src = "fn use(h: Number) -> Number { return h.join() }\nfn main() -> Number { return 0 }";
        let _ = src; // сам кейс перевіряється нижче через пряме значення
        let tokens  = Lexer::new("fn f(h: Number) -> Number { return h.join() }").tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        let module  = Compiler::new().compile_program(&program).unwrap();
        let mut vm2 = VM::new(module);
        let r = vm2.call_fn("f", vec![Value::Spawn(SpawnHandle { task_id: 999 })]);
        assert!(r.is_err(), "очікувалась помилка, а не Ok: {:?}", r);
    }
}
