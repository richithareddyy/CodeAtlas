use crate::Authorizer;

pub struct AdminService;

impl AdminService {
    pub fn authorize(&self, user: &str) -> bool {
        user == "root"
    }
}

impl Authorizer for AdminService {
    fn authorize(&self, user: &str) -> bool {
        AdminService::authorize(self, user)
    }
}
