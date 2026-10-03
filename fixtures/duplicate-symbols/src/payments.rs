pub struct PaymentService;

impl PaymentService {
    pub fn authorize(&self, amount_cents: u64) -> bool {
        amount_cents <= platform_fee() * 1_000
    }
}

#[cfg(unix)]
pub fn platform_fee() -> u64 {
    1
}

#[cfg(not(unix))]
pub fn platform_fee() -> u64 {
    2
}

pub fn charge(service: &PaymentService, token: &str) -> bool {
    crate::auth::authorize(token) && service.authorize(100)
}
