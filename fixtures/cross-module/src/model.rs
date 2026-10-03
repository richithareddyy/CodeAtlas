#[derive(Debug, Clone, Default)]
pub struct Invoice {
    pub id: u64,
    pub total_cents: u64,
}

impl Invoice {
    pub fn new(id: u64, total_cents: u64) -> Self {
        Invoice { id, total_cents }
    }
}
