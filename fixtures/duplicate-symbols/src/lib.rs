pub mod admin;
pub mod auth;
pub mod payments;

pub trait Authorizer {
    fn authorize(&self, user: &str) -> bool;
}
