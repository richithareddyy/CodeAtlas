use crate::gateway::Gateway;

pub fn refund_order(gateway: &dyn Gateway, payment_id: &str) -> bool {
    gateway.refund(payment_id)
}
