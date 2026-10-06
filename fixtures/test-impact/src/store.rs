use std::collections::BTreeMap;

pub trait Store {
    fn get(&self, key: &str) -> Option<u64>;
}

#[derive(Default)]
pub struct MemoryStore {
    values: BTreeMap<String, u64>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: &str, value: u64) {
        self.values.insert(key.to_string(), value);
    }
}

impl Store for MemoryStore {
    fn get(&self, key: &str) -> Option<u64> {
        self.values.get(key).copied()
    }
}

/// Values read from `KEY=VALUE` lines.
pub struct FileStore {
    lines: Vec<(String, u64)>,
}

impl FileStore {
    pub fn parse(text: &str) -> Self {
        // `pair` is passed as a value, not called.
        let lines = text.lines().filter_map(crate::parse::pair).collect();
        Self { lines }
    }
}

impl Store for FileStore {
    fn get(&self, key: &str) -> Option<u64> {
        self.lines.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

/// Generic over the store: the call through `S` cannot be resolved statically.
pub fn lookup_or_zero<S: Store>(store: &S, key: &str) -> u64 {
    store.get(key).unwrap_or(0)
}
