use crate::payments;
use crate::util::clamp;

pub fn checkout(order_id: u64) -> bool {
    payments::charge(order_id)
}

pub fn confirm(order_id: u64) -> bool {
    clamp(order_id, 1, 100) == order_id
}
