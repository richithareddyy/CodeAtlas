use super::PaymentError;

pub(crate) fn stripe_call(amount_cents: u64) -> Result<(), PaymentError> {
    if amount_cents == 0 {
        Err(PaymentError::Declined)
    } else {
        Ok(())
    }
}
