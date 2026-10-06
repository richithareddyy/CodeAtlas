use crate::pricing;
use crate::store::Store;

pub fn summary(items: &[u64]) -> String {
    format!("total {}", pricing::total(items))
}

pub fn balance(store: &dyn Store, key: &str) -> u64 {
    store.get(key).unwrap_or(0)
}

pub fn scaled(items: &[u64]) -> Vec<u64> {
    items.iter().map(|x| crate::util::clamp(*x)).collect()
}
