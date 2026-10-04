use crate::currency::to_cents;
use crate::gateway::Gateway;

pub struct PaymentService {
    gateway: Box<dyn Gateway>,
}

impl PaymentService {
    pub fn new(gateway: Box<dyn Gateway>) -> Self {
        Self { gateway }
    }

    pub fn authorize(&self, amount: u64, currency: &str) -> Result<String, String> {
        let cents = to_cents(amount, currency)?;
        validate_amount(cents)?;
        self.gateway.charge(cents)
    }
}

pub fn preauthorize(cents: u64) -> bool {
    validate_amount(cents).is_ok()
}

fn validate_amount(cents: u64) -> Result<(), String> {
    if cents == 0 || cents > 1_000_000 {
        Err("amount out of range".to_string())
    } else {
        Ok(())
    }
}
