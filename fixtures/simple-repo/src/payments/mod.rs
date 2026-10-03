mod gateway;

#[derive(Debug, PartialEq)]
pub enum PaymentError {
    Declined,
}

pub struct PaymentService {
    limit_cents: u64,
}

impl PaymentService {
    pub fn new(limit_cents: u64) -> Self {
        Self { limit_cents }
    }

    pub fn authorize(&self, amount_cents: u64) -> Result<(), PaymentError> {
        if amount_cents > self.limit_cents {
            return Err(PaymentError::Declined);
        }
        gateway::stripe_call(amount_cents)
    }
}

pub fn process_payment(amount_cents: u64) -> Result<(), PaymentError> {
    let service = PaymentService::new(10_000);
    service.authorize(amount_cents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_amounts_over_limit() {
        assert_eq!(
            PaymentService::new(5).authorize(10),
            Err(PaymentError::Declined)
        );
    }
}
