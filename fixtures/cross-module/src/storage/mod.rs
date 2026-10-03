mod memory;

pub use memory::MemoryStore;

use crate::model::Invoice;

pub trait Store {
    fn save(&mut self, invoice: Invoice) -> Result<(), String>;
    fn load(&self, id: u64) -> Option<Invoice>;

    fn exists(&self, id: u64) -> bool {
        self.load(id).is_some()
    }
}
