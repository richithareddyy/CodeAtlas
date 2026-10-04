pub fn format_cents(cents: u64) -> String {
    format!("${}.{:02}", cents / 100, cents % 100)
}

pub fn daily_total(amounts: &[u64]) -> u64 {
    amounts.iter().sum()
}
