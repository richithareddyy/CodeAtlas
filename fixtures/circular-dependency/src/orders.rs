use crate::checkout;

pub fn record(order_id: u64) -> bool {
    checkout::confirm(order_id)
}
