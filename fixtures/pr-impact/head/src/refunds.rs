use crate::gateway::Gateway;

/// Refunds a payment through the given gateway.
pub fn refund_order(gateway: &dyn Gateway, payment_id: &str) -> bool {
    // The gateway decides whether the payment can be refunded.
    gateway
        .refund(payment_id)
}
