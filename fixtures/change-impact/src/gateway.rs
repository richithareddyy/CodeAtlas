pub trait Gateway {
    fn charge(&self, cents: u64) -> Result<String, String>;
    fn refund(&self, payment_id: &str) -> bool;
}

pub struct StripeGateway;

impl Gateway for StripeGateway {
    fn charge(&self, cents: u64) -> Result<String, String> {
        Ok(format!("stripe-{cents}"))
    }

    fn refund(&self, payment_id: &str) -> bool {
        payment_id.starts_with("stripe-")
    }
}

pub struct FakeGateway;

impl Gateway for FakeGateway {
    fn charge(&self, cents: u64) -> Result<String, String> {
        Ok(format!("fake-{cents}"))
    }

    fn refund(&self, _payment_id: &str) -> bool {
        true
    }
}
