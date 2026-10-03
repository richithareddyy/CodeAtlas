use crate::orders;

pub fn charge(order_id: u64) -> bool {
    orders::record(order_id)
}
