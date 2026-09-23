/// Oberih VM — stack-based виконавець bytecode.
/// Нуль залежностей: тільки std::thread і std::sync.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::compiler::bytecode::{Instr, Module, CompiledFn};
use crate::gc::GcList;

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
            Value::Ok(v)   => write!(f, "Ok({})", v),
            Value::Err(v)  => write!(f, "Err({})", v),
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

impl OberihStruct {
    pub fn new(type_name: String, fields: HashMap<String, Value>) -> Self {
        OberihStruct {
            type_name,
            fields: Arc::new(Mutex::new(fields)),
        }
    }

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
        let guard = self.fields.lock().unwrap();
        write!(f, "{}", self.type_name)?;
        write!(f, "{{")?;
        let mut first = true;
        for (k, v) in guard.iter() {
            if !first { write!(f, ", ")?; }
            write!(f, "{}: {}", k, v)?;
            first = false;
        }
        write!(f, "}}")
    }
}

// ---------------------------------------------------------------------------
// SpawnHandle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SpawnHandle {
    pub result: Arc<Mutex<Option<Value>>>,
    pub done:   Arc<std::sync::atomic::AtomicBool>,
}

impl SpawnHandle {
    pub fn join(&self, timeout: Option<Duration>) -> Value {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            if self.done.load(std::sync::atomic::Ordering::Acquire) {
                let guard = self.result.lock().unwrap();
                return guard.clone().unwrap_or(Value::Nil);
            }
            if let Some(d) = deadline {
                if Instant::now() > d {
                    return Value::Err(Box::new(Value::Str("timeout".into())));
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

// ---------------------------------------------------------------------------
// RuntimeError
// ---------------------------------------------------------------------------

#[derive(Debug)]
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

pub struct VM {
    module: Arc<Module>,
    // Circuit breaker стан — per function
    cb_state: HashMap<String, CircuitState>,
    // Rate limit стан
    rl_state: HashMap<String, RateLimitState>,
    // Bulkhead стан
    bh_state: HashMap<String, Arc<Mutex<u32>>>,
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
            module:   Arc::new(module),
            cb_state: HashMap::new(),
            rl_state: HashMap::new(),
            bh_state: HashMap::new(),
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

        self.call_compiled(&compiled, args)
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
            if elapsed > res.rate_limit.as_ref().unwrap().per_secs {
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

        std::thread::spawn(move || {
            let mut vm = VM::new((*module_clone).clone());
            let result = vm.exec_frame(&func_clone, args);
            let _ = tx.send(result);
        });

        match rx.recv_timeout(Duration::from_secs_f64(secs)) {
            Ok(result) => result,
            Err(_) => Err(RuntimeError::PropagateErr(
                Value::Err(Box::new(Value::Str("timeout".into())))
            )),
        }
    }

    // --- Головний виконавець: stack machine ---

    fn exec_frame(&mut self, func: &CompiledFn, args: Vec<Value>) -> VR<Value> {
        let mut stack: Vec<Value> = Vec::with_capacity(64);
        let mut locals: Vec<Value> = vec![Value::Nil; func.local_count.max(args.len())];

        // Завантажуємо аргументи в перші слоти
        for (i, a) in args.into_iter().enumerate() {
            locals[i] = a;
        }

        let code = &func.code;
        let mut ip = 0usize;

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

        while ip < code.len() {
            let instr = &code[ip];
            ip += 1;

            match instr {
                Instr::PushNum(n)  => push!(Value::Num(*n)),
                Instr::PushStr(s)  => push!(Value::Str(s.clone())),
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

                Instr::LoadIndex => {
                    let idx = pop!(); let obj = pop!();
                    match (obj, idx) {
                        (Value::List(l), Value::Num(i)) => {
                            let i = i as usize;
                            push!(l.get(i).unwrap_or(Value::Nil));
                        }
                        _ => return Err(rt_err("LoadIndex: очікується List і Number")),
                    }
                }

                Instr::StoreIndex => {
                    let val = pop!(); let idx = pop!(); let obj = pop!();
                    match (obj, idx) {
                        (Value::List(l), Value::Num(i)) => {
                            let i = i as usize;
                            let mut v = l.to_vec();
                            if i < v.len() { v[i] = val; }
                            push!(Value::List(GcList::new(v)));
                        }
                        _ => return Err(rt_err("StoreIndex: очікується List і Number")),
                    }
                }

                Instr::Call(n) => {
                    let mut args: Vec<Value> = (0..*n).map(|_| stack.pop().unwrap_or(Value::Nil)).collect();
                    args.reverse();
                    let callee = pop!();
                    let result = match callee {
                        Value::Fn(ref name) if name.starts_with("__builtin_") => {
                            crate::stdlib::call_builtin(&name[10..], args)?
                        }
                        Value::Fn(name) => self.call_fn(&name, args)?,
                        _ => return Err(rt_err(format!("Не можна викликати {}", callee))),
                    };
                    push!(result);
                }

                Instr::CallMethod { name, arg_count } => {
                    let mut args: Vec<Value> = (0..*arg_count).map(|_| stack.pop().unwrap_or(Value::Nil)).collect();
                    args.reverse();
                    let receiver = pop!();
                    let result = self.call_method(receiver, name, args)?;
                    push!(result);
                }

                Instr::Return => {
                    return Ok(pop!());
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
                    let mut args: Vec<Value> = (0..*arg_count)
                        .map(|_| stack.pop().unwrap_or(Value::Nil))
                        .collect();
                    args.reverse();

                    let result_arc: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
                    let done_arc = Arc::new(std::sync::atomic::AtomicBool::new(false));

                    let result_clone = Arc::clone(&result_arc);
                    let done_clone   = Arc::clone(&done_arc);
                    let module_clone = Arc::clone(&self.module);
                    let fn_name_clone = fn_name.clone();

                    std::thread::spawn(move || {
                        let mut vm = VM::new((*module_clone).clone());
                        let val = vm.call_fn(&fn_name_clone, args).unwrap_or(Value::Nil);
                        *result_clone.lock().unwrap() = Some(val);
                        done_clone.store(true, std::sync::atomic::Ordering::Release);
                    });

                    push!(Value::Spawn(SpawnHandle { result: result_arc, done: done_arc }));
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
        }

        // Якщо дійшли до кінця без Return — повертаємо nil
        Ok(stack.pop().unwrap_or(Value::Nil))
    }

    // --- Методи ---

    fn call_method(&mut self, receiver: Value, method: &str, args: Vec<Value>) -> VR<Value> {
        match &receiver {
            Value::Str(_) => self.call_str_method(receiver, method, args),
            Value::List(_) => self.call_list_method(receiver, method, args),
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
            Value::Spawn(h) => {
                match method {
                    "join" => {
                        let timeout = args.into_iter().next().and_then(|v| {
                            if let Value::Num(n) = v { Some(Duration::from_secs_f64(n)) } else { None }
                        });
                        Ok(h.join(timeout))
                    }
                    "isDone" => Ok(Value::Bool(h.done.load(std::sync::atomic::Ordering::Acquire))),
                    _ => Err(rt_err(format!("Невідомий метод spawn хендлу: {}", method))),
                }
            }
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
}
