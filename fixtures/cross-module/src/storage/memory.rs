use std::collections::HashMap;

use super::Store;
use crate::model::Invoice;

#[derive(Default)]
pub struct MemoryStore {
    invoices: HashMap<u64, Invoice>,
}

impl Store for MemoryStore {
    fn save(&mut self, invoice: Invoice) -> Result<(), String> {
        self.invoices.insert(invoice.id, invoice);
        Ok(())
    }

    fn load(&self, id: u64) -> Option<Invoice> {
        self.invoices.get(&id).cloned()
    }
}
