use simple_repo::checkout::{checkout, Order};

#[test]
fn checkout_succeeds_for_small_orders() {
    let order = Order {
        id: 1,
        amount_cents: 500,
    };
    assert!(checkout(&order).is_ok());
}
