/// Oberih GC — reference counting garbage collector.
/// Arc<Mutex<>> — thread-safe, працює з spawn.
/// WeakRef — слабкі посилання для циклічних структур.

use std::sync::{Arc, Mutex, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};

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
        GcList(Arc::new(Mutex::new(elems)))
    }

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
        *self.0.lock().unwrap() == *other.0.lock().unwrap()
    }
}

impl std::fmt::Display for GcList {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let guard = self.0.lock().unwrap();
        write!(f, "[")?;
        for (i, v) in guard.iter().enumerate() {
            if i > 0 { write!(f, ", ")?; }
            write!(f, "{}", v)?;
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
