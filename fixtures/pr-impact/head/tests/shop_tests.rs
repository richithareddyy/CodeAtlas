use pr_shop::checkout::checkout;
use pr_shop::gateway::FakeGateway;
use pr_shop::invoice::print_invoice;
use pr_shop::payments::{preauthorize, PaymentService};
use pr_shop::refunds::refund_order;
use pr_shop::reporting::daily_total;

#[test]
fn test_authorize_valid() {
    let service = PaymentService::new(Box::new(FakeGateway));
    assert_eq!(service.authorize(100, "USD"), Ok("fake-100".to_string()));
}

#[test]
fn test_checkout() {
    let service = PaymentService::new(Box::new(FakeGateway));
    assert_eq!(checkout(&service, 100), Ok("fake-90".to_string()));
}

#[test]
fn test_preauthorize() {
    assert!(preauthorize(5));
}

#[test]
fn test_print_invoice() {
    assert_eq!(print_invoice(100), "$1.02");
}

#[test]
fn test_refund() {
    assert!(refund_order(&FakeGateway, "fake-1"));
}

#[test]
fn test_daily_total() {
    assert_eq!(daily_total(&[1, 2, 3]), 6);
}
