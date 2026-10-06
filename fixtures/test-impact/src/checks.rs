/// Expands to a call of `util::validate`. Calls written inside a macro
/// definition are not visible to static analysis.
#[macro_export]
macro_rules! check {
    ($value:expr) => {
        $crate::util::validate($value)
    };
}

pub fn all_valid(values: &[u64]) -> bool {
    values.iter().all(|v| check!(*v))
}
