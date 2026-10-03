use crate::model::Invoice;
use crate::storage::*;

pub fn record(id: u64) {
    let _ = id;
}

pub fn summarize(store: &dyn Store, ids: &[u64]) -> usize {
    ids.iter().filter(|id| store.exists(**id)).count()
}

pub fn total(store: &impl Store, id: u64) -> u64 {
    store
        .load(id)
        .map(|invoice: Invoice| invoice.total_cents)
        .unwrap_or(0)
}

pub fn first_saved<S: Store>(store: &mut S) -> bool {
    store.save(Invoice::new(1, 1)).is_ok()
}

pub fn discounted_total(invoice: &Invoice) -> u64 {
    invoice.discounted(10)
}
