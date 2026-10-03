pub fn clamp(value: u64, low: u64, high: u64) -> u64 {
    value.max(low).min(high)
}
