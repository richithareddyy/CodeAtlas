use change_impact::checkout::checkout;
use change_impact::gateway::FakeGateway;
use change_impact::payments::PaymentService;
use change_impact::refunds::refund_order;
use change_impact::reports::daily_total;

#[test]
fn test_authorize_valid() {
    let result = PaymentService::new(Box::new(FakeGateway)).authorize(100);
    assert_eq!(result, Ok("fake-100".to_string()));
}

#[test]
fn test_checkout() {
    let service = PaymentService::new(Box::new(FakeGateway));
    assert_eq!(checkout(&service, 100), Ok("fake-90".to_string()));
}

#[test]
fn test_refund() {
    assert!(refund_order(&FakeGateway, "fake-1"));
}

#[test]
fn test_daily_total() {
    assert_eq!(daily_total(&[1, 2, 3]), 6);
}
