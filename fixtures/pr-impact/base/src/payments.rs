use crate::gateway::Gateway;

pub struct PaymentService {
    gateway: Box<dyn Gateway>,
}

impl PaymentService {
    pub fn new(gateway: Box<dyn Gateway>) -> Self {
        Self { gateway }
    }

    pub fn authorize(&self, cents: u64) -> Result<String, String> {
        validate_amount(cents)?;
        self.gateway.charge(cents)
    }
}

pub fn preauthorize(cents: u64) -> bool {
    validate_amount(cents).is_ok()
}

fn validate_amount(cents: u64) -> Result<(), String> {
    if cents == 0 {
        Err("amount must be positive".to_string())
    } else {
        Ok(())
    }
}
