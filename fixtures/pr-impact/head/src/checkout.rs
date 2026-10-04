use crate::payments::PaymentService;

pub fn checkout(service: &PaymentService, cart_total: u64) -> Result<String, String> {
    let total = apply_discount(cart_total);
    service.authorize(total, "USD")
}

fn apply_discount(total: u64) -> u64 {
    total - total / 10
}
