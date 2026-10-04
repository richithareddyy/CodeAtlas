use crate::legacy::legacy_fee;
use crate::reports::format_cents;

pub fn invoice_total(amount: u64) -> u64 {
    amount + legacy_fee(amount)
}

pub fn print_invoice(amount: u64) -> String {
    format_cents(invoice_total(amount))
}
