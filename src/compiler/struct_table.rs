/// Таблиця struct-типів — зберігає імена полів в порядку оголошення.
/// Компілятор будує її при першому проході, VM використовує при MakeStruct.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct StructTable {
    /// name -> [field_name, ...]
    types: HashMap<String, Vec<String>>,
}

impl StructTable {
    pub fn new() -> Self {
        StructTable { types: HashMap::new() }
    }

    pub fn register(&mut self, name: &str, fields: Vec<String>) {
        self.types.insert(name.to_string(), fields);
    }

    pub fn fields(&self, name: &str) -> Option<&Vec<String>> {
        self.types.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }
}
