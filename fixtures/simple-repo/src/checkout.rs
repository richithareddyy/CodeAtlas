use crate::inventory;
use crate::payments::{self, PaymentError};

pub struct Order {
    pub id: u64,
    pub amount_cents: u64,
}

pub fn checkout(order: &Order) -> Result<(), PaymentError> {
    inventory::reserve(order.id);
    payments::process_payment(order.amount_cents)?;
    Ok(())
}
