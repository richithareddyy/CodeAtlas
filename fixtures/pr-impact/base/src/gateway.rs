pub trait Gateway {
    fn charge(&self, cents: u64) -> Result<String, String>;
    fn refund(&self, payment_id: &str) -> bool;
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
