/// `KEY=VALUE` to `(KEY, VALUE)`.
pub fn pair(line: &str) -> Option<(String, u64)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim().to_string(), number(value)?))
}

pub fn number(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}
