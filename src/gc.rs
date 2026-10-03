/// Oberih GC — reference counting garbage collector.
/// Arc<Mutex<>> — thread-safe, працює з spawn.
/// WeakRef — слабкі посилання для циклічних структур.

use std::sync::{Arc, Mutex, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::collections::HashSet;

static GC_ALLOCS: AtomicUsize = AtomicUsize::new(0);
static GC_DROPS:  AtomicUsize = AtomicUsize::new(0);

pub fn gc_alloc() { GC_ALLOCS.fetch_add(1, Ordering::Relaxed); }
pub fn gc_drop()  { GC_DROPS.fetch_add(1, Ordering::Relaxed);  }

#[derive(Debug, Clone)]
pub struct GcStats {
    pub total_allocs: usize,
    pub total_drops:  usize,
    pub live_objects: usize,
}

pub fn gc_stats() -> GcStats {
    let allocs = GC_ALLOCS.load(Ordering::Relaxed);
    let drops  = GC_DROPS.load(Ordering::Relaxed);
    GcStats {
        total_allocs: allocs,
        total_drops:  drops,
        live_objects: allocs.saturating_sub(drops),
    }
}

#[derive(Debug, Clone)]
pub struct GcList(pub Arc<Mutex<Vec<crate::vm::Value>>>);

impl GcList {
    pub fn new(elems: Vec<crate::vm::Value>) -> Self {
        gc_alloc();
        let arc = Arc::new(Mutex::new(elems));
        register(GcHandle::List(Arc::downgrade(&arc)));
        GcList(arc)
    }

    /// Ідентичність для трасування циклів: та сама адреса — той самий об'єкт,
    /// незалежно від того, скільки `Value` на нього посилаються.
    pub fn ptr_id(&self) -> usize { Arc::as_ptr(&self.0) as *const () as usize }

    pub fn empty() -> Self { Self::new(vec![]) }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn get(&self, idx: usize) -> Option<crate::vm::Value> {
        self.0.lock().unwrap().get(idx).cloned()
    }

    pub fn push(&self, v: crate::vm::Value) {
        self.0.lock().unwrap().push(v);
    }

    pub fn pop(&self) -> Option<crate::vm::Value> {
        self.0.lock().unwrap().pop()
    }

    pub fn first(&self) -> Option<crate::vm::Value> {
        self.0.lock().unwrap().first().cloned()
    }

    pub fn last(&self) -> Option<crate::vm::Value> {
        self.0.lock().unwrap().last().cloned()
    }

    pub fn reverse(&self) -> Self {
        let mut v = self.0.lock().unwrap().clone();
        v.reverse();
        Self::new(v)
    }

    pub fn contains(&self, val: &crate::vm::Value) -> bool {
        self.0.lock().unwrap().contains(val)
    }

    pub fn to_vec(&self) -> Vec<crate::vm::Value> {
        self.0.lock().unwrap().clone()
    }

    /// Записує значення на місці (список — посилальний тип). false — індекс поза межами.
    pub fn set(&self, idx: usize, v: crate::vm::Value) -> bool {
        let mut g = self.0.lock().unwrap();
        if idx < g.len() { g[idx] = v; true } else { false }
    }

    pub fn ref_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }

    pub fn downgrade(&self) -> WeakList {
        WeakList(Arc::downgrade(&self.0))
    }
}

impl Drop for GcList {
    fn drop(&mut self) {
        if Arc::strong_count(&self.0) == 1 {
            gc_drop();
        }
    }
}

impl PartialEq for GcList {
    fn eq(&self, other: &Self) -> bool {
        // Той самий список: без цього `xs == xs` намагався б двічі взяти
        // один і той самий Mutex і назавжди зависав (deadlock).
        if Arc::ptr_eq(&self.0, &other.0) { return true; }
        let a = self.to_vec();            // lock береться і відпускається одразу
        let b = other.0.lock().unwrap();
        a == *b
    }
}

impl std::fmt::Display for GcList {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // Знімок, щоб не тримати lock під час друку вкладених значень.
        let items = self.to_vec();
        write!(f, "[")?;
        for (i, v) in items.iter().enumerate() {
            if i > 0 { write!(f, ", ")?; }
            write!(f, "{}", v.repr())?;
        }
        write!(f, "]")
    }
}

#[derive(Debug, Clone)]
pub struct WeakList(pub Weak<Mutex<Vec<crate::vm::Value>>>);

impl WeakList {
    pub fn upgrade(&self) -> Option<GcList> {
        self.0.upgrade().map(GcList)
    }

    pub fn is_alive(&self) -> bool {
        self.0.strong_count() > 0
    }
}

// ---------------------------------------------------------------------------
// Map
// ---------------------------------------------------------------------------

use std::collections::HashMap;

/// Ключ Map: тільки значення, що мають чітку рівність і хеш.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MapKey {
    Str(String),
    /// f64 у вигляді бітів (-0.0 нормалізується до 0.0, NaN заборонений)
    Num(u64),
    Bool(bool),
}

impl MapKey {
    pub fn from_value(v: &crate::vm::Value) -> Result<MapKey, String> {
        use crate::vm::Value;
        match v {
            Value::Str(s)  => Ok(MapKey::Str(s.clone())),
            Value::Bool(b) => Ok(MapKey::Bool(*b)),
            Value::Num(n)  => {
                if n.is_nan() { return Err("NaN не може бути ключем Map".into()); }
                let n = if *n == 0.0 { 0.0 } else { *n };
                Ok(MapKey::Num(n.to_bits()))
            }
            other => Err(format!(
                "ключ Map має бути String, Number або Bool, отримано {}", other
            )),
        }
    }

    pub fn to_value(&self) -> crate::vm::Value {
        use crate::vm::Value;
        match self {
            MapKey::Str(s)  => Value::Str(s.clone()),
            MapKey::Num(b)  => Value::Num(f64::from_bits(*b)),
            MapKey::Bool(b) => Value::Bool(*b),
        }
    }
}

/// Map зі збереженням порядку вставки: Vec для порядку + HashMap для O(1) пошуку.
/// Видалення лишає «позначку» (None) у Vec, а коли позначок стає забагато —
/// Vec ущільнюється; тож `remove` амортизовано O(1), а порядок не порушується.
#[derive(Debug, Clone, Default)]
pub struct OMap {
    entries: Vec<Option<(MapKey, crate::vm::Value)>>,
    index:   HashMap<MapKey, usize>,
}

impl OMap {
    pub fn new() -> Self { OMap::default() }
    pub fn len(&self) -> usize { self.index.len() }
    pub fn contains(&self, k: &MapKey) -> bool { self.index.contains_key(k) }

    pub fn get(&self, k: &MapKey) -> Option<&crate::vm::Value> {
        let i = *self.index.get(k)?;
        self.entries[i].as_ref().map(|(_, v)| v)
    }

    /// Вставка: існуючий ключ зберігає свою позицію, новий іде в кінець.
    pub fn insert(&mut self, k: MapKey, v: crate::vm::Value) {
        if let Some(&i) = self.index.get(&k) {
            if let Some(slot) = self.entries[i].as_mut() { slot.1 = v; }
        } else {
            self.index.insert(k.clone(), self.entries.len());
            self.entries.push(Some((k, v)));
        }
    }

    /// Амортизовано O(1).
    pub fn remove(&mut self, k: &MapKey) -> Option<crate::vm::Value> {
        let i = self.index.remove(k)?;
        let removed = self.entries[i].take().map(|(_, v)| v);
        let dead = self.entries.len() - self.index.len();
        if dead > 16 && dead > self.entries.len() / 2 {
            self.compact();
        }
        removed
    }

    fn compact(&mut self) {
        let live: Vec<_> = self.entries.drain(..).flatten().collect();
        self.index.clear();
        for (i, (k, _)) in live.iter().enumerate() {
            self.index.insert(k.clone(), i);
        }
        self.entries = live.into_iter().map(Some).collect();
    }

    pub fn snapshot(&self) -> Vec<(MapKey, crate::vm::Value)> {
        self.entries.iter().flatten().cloned().collect()
    }

    /// Розриває цикл (див. `collect_cycles`): звільняє все, що Map тримав.
    pub fn clear(&mut self) { self.entries.clear(); self.index.clear(); }
}

/// Map як значення VM. Посилальний тип (як struct і list): присвоєння і передача
/// у функцію не копіюють дані, зміни бачать усі власники. Arc<Mutex<>> — щоб
/// працювало з `spawn`.
#[derive(Debug, Clone)]
pub struct GcMap(pub Arc<Mutex<OMap>>);

impl GcMap {
    pub fn new(m: OMap) -> Self {
        gc_alloc();
        let arc = Arc::new(Mutex::new(m));
        register(GcHandle::Map(Arc::downgrade(&arc)));
        GcMap(arc)
    }

    pub fn ptr_id(&self) -> usize { Arc::as_ptr(&self.0) as *const () as usize }
    pub fn empty() -> Self { Self::new(OMap::new()) }
    pub fn len(&self) -> usize { self.0.lock().unwrap().len() }
    pub fn get(&self, k: &MapKey) -> Option<crate::vm::Value> {
        self.0.lock().unwrap().get(k).cloned()
    }
    pub fn contains(&self, k: &MapKey) -> bool { self.0.lock().unwrap().contains(k) }
    pub fn insert(&self, k: MapKey, v: crate::vm::Value) { self.0.lock().unwrap().insert(k, v); }
    pub fn remove(&self, k: &MapKey) -> Option<crate::vm::Value> { self.0.lock().unwrap().remove(k) }
    pub fn snapshot(&self) -> Vec<(MapKey, crate::vm::Value)> { self.0.lock().unwrap().snapshot() }
    pub fn keys(&self) -> Vec<crate::vm::Value> {
        self.snapshot().into_iter().map(|(k, _)| k.to_value()).collect()
    }
    pub fn values(&self) -> Vec<crate::vm::Value> {
        self.snapshot().into_iter().map(|(_, v)| v).collect()
    }
}

impl Drop for GcMap {
    fn drop(&mut self) {
        if Arc::strong_count(&self.0) == 1 {
            gc_drop();
        }
    }
}

impl PartialEq for GcMap {
    /// Рівність не залежить від порядку вставки.
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.0, &other.0) { return true; }
        let a = self.snapshot();            // lock відпускається одразу
        let b = other.0.lock().unwrap();
        a.len() == b.len() && a.iter().all(|(k, v)| b.get(k) == Some(v))
    }
}

impl std::fmt::Display for GcMap {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // Знімок, щоб не тримати lock під час друку вкладених значень.
        let snap = self.snapshot();
        write!(f, "{{")?;
        for (i, (k, v)) in snap.iter().enumerate() {
            if i > 0 { write!(f, ", ")?; }
            write!(f, "{}: {}", k.to_value().repr(), v.repr())?;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;
    use crate::vm::Value;

    fn k(s: &str) -> MapKey { MapKey::Str(s.into()) }

    #[test]
    fn delete_keeps_order_and_lookup_across_compaction() {
        let mut m = OMap::new();
        for i in 0..200 { m.insert(k(&format!("k{}", i)), Value::Num(i as f64)); }
        // видаляємо всі парні — кілька разів спрацює ущільнення
        for i in (0..200).step_by(2) { assert!(m.remove(&k(&format!("k{}", i))).is_some()); }
        assert_eq!(m.len(), 100);
        let keys: Vec<String> = m.snapshot().into_iter()
            .map(|(k, _)| match k { MapKey::Str(s) => s, _ => unreachable!() }).collect();
        let expected: Vec<String> = (0..200).filter(|i| i % 2 == 1).map(|i| format!("k{}", i)).collect();
        assert_eq!(keys, expected, "порядок порушено");
        for i in (1..200).step_by(2) {
            assert_eq!(m.get(&k(&format!("k{}", i))), Some(&Value::Num(i as f64)));
        }
        assert!(m.get(&k("k0")).is_none());
        // повторна вставка видаленого ключа йде в кінець
        m.insert(k("k0"), Value::Num(0.0));
        assert_eq!(m.snapshot().last().unwrap().0, k("k0"));
    }

    #[test]
    fn delete_is_fast_for_large_maps() {
        let mut m = OMap::new();
        for i in 0..100_000 { m.insert(MapKey::Num(i as u64), Value::Nil); }
        let t = std::time::Instant::now();
        for i in 0..100_000 { m.remove(&MapKey::Num(i as u64)); }
        assert_eq!(m.len(), 0);
        // раніше (O(n) на видалення) це були б секунди/хвилини
        assert!(t.elapsed().as_secs_f64() < 2.0, "видалення надто повільне: {:?}", t.elapsed());
    }
}

// ---------------------------------------------------------------------------
// Виявлення циклів
// ---------------------------------------------------------------------------
//
// Reference counting (усе вище) не звільняє цикли: `a.next = b; b.prev = a`
// тримають одне одного живими назавжди, навіть коли ззовні на них уже ніхто
// не посилається. Це не проблема пам'яті в сенсі unsafe — Rust-виклик просто
// "втрачає" ці байти до завершення процесу — але для довгоживучого процесу
// (найперше — REPL-сесія) це реальна витік пам'яті.
//
// Рішення: реєстр СЛАБКИХ посилань на кожен виділений List/Map/Struct
// (`REGISTRY`), і `collect_cycles(roots)` — класичний mark-and-sweep:
// 1) позначаємо все, до чого можна дістатись з `roots`;
// 2) усе зареєстроване, але НЕ позначене (а отже — недосяжне ззовні,
//    і, раз досі живе при ref-counting, тримається лише циклом) —
//    примусово спорожняємо, розриваючи цикл; нормальний Arc-drop після
//    цього звільняє пам'ять як завжди.
//
// Через специфіку `exec_frame` (рекурсивний Rust-виклик на кожен кадр
// Oberih-функції, без явного стека кадрів) неможливо безпечно перелічити
// ВСІ живі локальні змінні під час виконання довільної програми — це
// вимагало б переписати VM на явний стек кадрів. Тому автоматичний запуск
// є лише в REPL між рядками (там `roots` — це просто `session_vars`,
// повний і точний список), а `gcCollectCycles(...)` як вбудована функція —
// явний, з коренями, які передає сам виклик.

enum GcHandle {
    List(Weak<Mutex<Vec<crate::vm::Value>>>),
    Map(Weak<Mutex<OMap>>),
    Struct(Weak<Mutex<std::collections::HashMap<String, crate::vm::Value>>>),
}

impl GcHandle {
    fn id(&self) -> usize {
        match self {
            GcHandle::List(w)   => w.as_ptr() as *const () as usize,
            GcHandle::Map(w)    => w.as_ptr() as *const () as usize,
            GcHandle::Struct(w) => w.as_ptr() as *const () as usize,
        }
    }
    fn is_alive(&self) -> bool {
        match self {
            GcHandle::List(w)   => w.strong_count() > 0,
            GcHandle::Map(w)    => w.strong_count() > 0,
            GcHandle::Struct(w) => w.strong_count() > 0,
        }
    }
    /// Спорожняє об'єкт (розриває цикл). `true`, якщо щось справді впало.
    ///
    /// КРИТИЧНО: `Weak::upgrade()` повертає "сирий" `Arc<Mutex<...>>`, а НЕ
    /// `GcList`/`GcMap`/`OberihStruct` — тобто БЕЗ їхнього кастомного `Drop`,
    /// який власне і викликає `gc_drop()`. Якщо просто дати цьому сирому
    /// `Arc` вийти зі скоупу як є, останнє посилання на об'єкт звільниться
    /// TИХО, без обліку — і `gc-stats`/`gcStats()` назавжди "забудуть" один
    /// об'єкт на кожен розірваний цикл (реальної витік пам'яті при цьому
    /// немає, і `WeakRef`/is_alive після цього так само коректні — рахунок
    /// просто розходиться зі станом). Тому обгортаємо назад у типізований
    /// wrapper НАПРЯМУ (не через `::new()`, щоб не викликати повторний
    /// `gc_alloc()`/реєстрацію) — так його `Drop` відпрацює як завжди.
    fn clear(&self) -> bool {
        match self {
            GcHandle::List(w) => match w.upgrade() {
                Some(arc) => {
                    let temp = GcList(arc);
                    let had = { let mut g = temp.0.lock().unwrap(); let had = !g.is_empty(); g.clear(); had };
                    had
                }
                None => false,
            },
            GcHandle::Map(w) => match w.upgrade() {
                Some(arc) => {
                    let temp = GcMap(arc);
                    let had = { let mut g = temp.0.lock().unwrap(); let had = g.len() > 0; g.clear(); had };
                    had
                }
                None => false,
            },
            GcHandle::Struct(w) => match w.upgrade() {
                Some(arc) => {
                    let temp = crate::vm::OberihStruct { type_name: String::new(), fields: arc };
                    let had = { let mut g = temp.fields.lock().unwrap(); let had = !g.is_empty(); g.clear(); had };
                    had
                }
                None => false,
            },
        }
    }
}

static REGISTRY: Mutex<Vec<GcHandle>> = Mutex::new(Vec::new());

fn register(h: GcHandle) { REGISTRY.lock().unwrap().push(h); }

/// Викликає `OberihStruct::new` — реєструє поля struct для трасування циклів.
/// Публічна обгортка, бо сам `GcHandle` приватний для цього модуля.
pub fn register_struct_fields(w: Weak<Mutex<std::collections::HashMap<String, crate::vm::Value>>>) {
    register(GcHandle::Struct(w));
}

/// Позначає `v` і все, до чого можна дістатись з нього, як досяжне.
/// `visited` — ідентичність уже позначених контейнерів (щоб не зациклитись
/// на самому цикл, який ми ж і шукаємо — сама структура даних тут цілком
/// звичайна цикл-стійка обходка графа).
fn trace_value(v: &crate::vm::Value, visited: &mut HashSet<usize>, worklist: &mut Vec<crate::vm::Value>) {
    use crate::vm::Value;
    match v {
        Value::List(l) => {
            if visited.insert(l.ptr_id()) { worklist.extend(l.to_vec()); }
        }
        Value::Map(m) => {
            if visited.insert(m.ptr_id()) { worklist.extend(m.values()); }
        }
        Value::Struct(s) => {
            if visited.insert(s.ptr_id()) {
                worklist.extend(s.fields.lock().unwrap().values().cloned());
            }
        }
        Value::Closure(_, captured) => worklist.extend(captured.iter().cloned()),
        Value::Ok(b) | Value::Err(b) => worklist.push((**b).clone()),
        // WeakRef навмисно НЕ трасуємо — слабке посилання не повинне утримувати
        // об'єкт живим і не рахується "шляхом" для mark-фази.
        Value::Spawn(h) => {
            if let Some(v) = h.result.lock().unwrap().clone() { worklist.push(v); }
        }
        _ => {} // Num/Str/Bool/Nil/EnumVal/Fn — без вкладених Value
    }
}

/// Запускає збирання циклів: усе, зареєстроване в REGISTRY, але недосяжне
/// з `roots`, примусово спорожняється. Повертає кількість очищених об'єктів.
///
/// БЕЗПЕКА: викликайте лише тоді, коли `roots` справді містить ВСІ значення,
/// які програма ще використовує (усі активні змінні/поля), і коли жоден
/// інший потік (`spawn`) паралельно не читає/не пише ці самі List/Map/Struct —
/// інакше можна розірвати структуру, яка комусь ще потрібна. REPL це гарантує
/// сам (запускає тільки коли немає незавершених `spawn`); при виклику
/// `gcCollectCycles(...)` з коду програми відповідальність на розробнику.
pub fn collect_cycles(roots: &[crate::vm::Value]) -> usize {
    { REGISTRY.lock().unwrap().retain(|h| h.is_alive()); }

    let mut visited: HashSet<usize> = HashSet::new();
    let mut worklist: Vec<crate::vm::Value> = roots.to_vec();
    while let Some(v) = worklist.pop() {
        trace_value(&v, &mut visited, &mut worklist);
    }

    let reg = REGISTRY.lock().unwrap();
    reg.iter()
        .filter(|h| h.is_alive() && !visited.contains(&h.id()))
        .filter(|h| h.clear())
        .count()
}

#[cfg(test)]
mod cycle_tests {
    use super::*;
    use crate::vm::{OberihStruct, Value};
    use std::collections::HashMap;

    fn make_struct(name: &str) -> OberihStruct {
        OberihStruct::new(name.to_string(), HashMap::new())
    }

    #[test]
    fn breaks_a_two_node_struct_cycle() {
        let a = make_struct("Node");
        let b = make_struct("Node");
        a.fields.lock().unwrap().insert("next".into(), Value::Struct(b.clone()));
        b.fields.lock().unwrap().insert("prev".into(), Value::Struct(a.clone()));
        // Тепер a і b тримають одне одного; локальні змінні a/b (Rust) ще живі,
        // але жодного Oberih-кореня на них немає.
        let swept = collect_cycles(&[]);
        assert!(swept >= 2, "мало розірвати обидва вузли циклу, розірвано {}", swept);
        assert!(a.get("next").is_none(), "поле мало спорожніти після розриву циклу");
        assert!(b.get("prev").is_none());
    }

    #[test]
    fn does_not_touch_objects_reachable_from_roots() {
        let a = make_struct("Node");
        let b = make_struct("Node");
        a.fields.lock().unwrap().insert("next".into(), Value::Struct(b.clone()));
        b.fields.lock().unwrap().insert("prev".into(), Value::Struct(a.clone()));
        // a переданий як корінь -> весь цикл (a і b) лишається живим.
        let swept = collect_cycles(&[Value::Struct(a.clone())]);
        assert_eq!(swept, 0);
        assert!(a.get("next").is_some());
        assert!(b.get("prev").is_some());
    }

    #[test]
    fn list_and_map_cycles_are_broken_too() {
        let list = GcList::new(vec![Value::Nil]);
        let map  = GcMap::new(OMap::new());
        map.insert(MapKey::Str("l".into()), Value::List(list.clone()));
        list.set(0, Value::Map(map.clone()));
        let swept = collect_cycles(&[]);
        assert!(swept >= 2);
        assert_eq!(list.len(), 0);
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn non_cyclic_garbage_is_not_double_counted_as_a_problem() {
        // Звичайне (нециклічне) сміття вже звільнене ref-counting до виклику
        // collect_cycles — воно не мало б навіть потрапити в підрахунок,
        // бо Drop уже прибрав його з реєстру (is_alive() == false).
        { let _tmp = GcList::new(vec![Value::Num(1.0)]); }
        let swept = collect_cycles(&[]);
        assert_eq!(swept, 0, "нециклічне сміття не повинно рахуватись тут — воно вже звільнене");
    }
}

#[cfg(test)]
mod cycle_scale_tests {
    use super::*;
    use crate::vm::Value;

    #[test]
    fn many_independent_map_pairs_are_all_freed() {
        let before = gc_stats();
        for _ in 0..100 {
            let a = GcMap::new(OMap::new());
            let b = GcMap::new(OMap::new());
            a.insert(MapKey::Str("next".into()), Value::Map(b.clone()));
            b.insert(MapKey::Str("prev".into()), Value::Map(a.clone()));
            // a, b виходять з області видимості тут — цикл, недосяжний ззовні.
        }
        let leaked = gc_stats();
        assert_eq!(leaked.live_objects - before.live_objects, 200, "усі 200 мали лишитись живими (цикл)");

        let swept = collect_cycles(&[]);
        let cleaned = gc_stats();
        eprintln!("swept={} live_before={} live_after={}", swept, leaked.live_objects, cleaned.live_objects);
        assert_eq!(cleaned.live_objects, before.live_objects, "усі 200 мали звільнитись, лишилось {}", cleaned.live_objects - before.live_objects);
    }
}




