use ledger::checks::all_valid;
use ledger::report::{balance, scaled, summary};
use ledger::store::{lookup_or_zero, FileStore, MemoryStore};

#[test]
fn summary_includes_tax() {
    assert_eq!(summary(&[100]), "total 108");
}

#[test]
fn balance_reads_the_memory_store() {
    let mut store = MemoryStore::new();
    store.insert("cash", 5);
    assert_eq!(balance(&store, "cash"), 5);
}

#[test]
fn file_store_parses_lines() {
    let store = FileStore::parse("cash = 7\nbank=3");
    assert_eq!(balance(&store, "bank"), 3);
}

#[test]
fn generic_lookup_defaults_to_zero() {
    let store = MemoryStore::new();
    assert_eq!(lookup_or_zero(&store, "missing"), 0);
}

#[test]
fn scaled_clamps_values() {
    assert_eq!(scaled(&[5, 20_000]), vec![5, 10_000]);
}

#[test]
fn positive_values_are_valid() {
    assert!(all_valid(&[1, 2]));
}
