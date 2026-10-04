pub fn to_cents(amount: u64, currency: &str) -> Result<u64, String> {
    match currency {
        "USD" | "EUR" => Ok(amount),
        "JPY" => Ok(amount * 100),
        other => Err(format!("unsupported currency {other}")),
    }
}
