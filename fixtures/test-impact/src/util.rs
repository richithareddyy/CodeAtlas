pub fn clamp(value: u64) -> u64 {
    value.min(10_000)
}

pub fn validate(value: u64) -> bool {
    value > 0
}

pub fn unused_helper(value: u64) -> u64 {
    value * 2
}
