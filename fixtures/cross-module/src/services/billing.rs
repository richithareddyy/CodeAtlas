use crate::model::Invoice as Bill;
use crate::storage::{MemoryStore, Store};

pub struct Billing {
    store: MemoryStore,
}

impl Billing {
    pub fn new() -> Self {
        Self {
            store: MemoryStore::default(),
        }
    }

    pub fn issue(&mut self, id: u64, total_cents: u64) -> Result<Bill, String> {
        let bill = Bill::new(id, total_cents);
        self.store.save(bill.clone())?;
        Self::audit(&bill);
        Ok(bill)
    }

    fn audit(bill: &Bill) {
        super::reporting::record(bill.id);
    }
}

impl Default for Billing {
    fn default() -> Self {
        Self::new()
    }
}

// An inherent impl written outside the type's own module.
impl crate::model::Invoice {
    pub fn discounted(&self, percent: u64) -> u64 {
        self.total_cents * (100 - percent) / 100
    }
}
