use cross_module::storage::{MemoryStore, Store};
use cross_module::Billing;

#[test]
fn issues_and_stores_invoices() {
    let mut billing = Billing::new();
    let bill = billing.issue(7, 1_500).unwrap();
    assert_eq!(bill.total_cents, 1_500);
}

#[test]
fn memory_store_round_trip() {
    let mut store = MemoryStore::default();
    store.save(cross_module::model::Invoice::new(1, 10)).unwrap();
    assert!(store.exists(1));
}
