pub fn subtotal(items: &[u64]) -> u64 {
    items.iter().sum()
}

pub fn tax(amount: u64) -> u64 {
    amount * 8 / 100
}

pub fn total(items: &[u64]) -> u64 {
    let base = subtotal(items);
    base + tax(base)
}

/// Large orders get a discount; the tests only exercise small ones.
pub fn route(amount: u64) -> u64 {
    if amount > 1_000 {
        bulk_discount(amount)
    } else {
        amount
    }
}

fn bulk_discount(amount: u64) -> u64 {
    amount - amount / 20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtotal_adds_items() {
        assert_eq!(subtotal(&[1, 2, 3]), 6);
    }

    #[test]
    fn tax_is_eight_percent() {
        assert_eq!(tax(100), 8);
    }

    #[test]
    fn small_orders_are_not_discounted() {
        assert_eq!(route(50), 50);
    }
}
