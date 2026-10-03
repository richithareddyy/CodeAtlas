pub struct OAuthService {
    pub issuer: String,
}

impl OAuthService {
    pub fn authorize(&self, token: &str) -> bool {
        authorize(token)
    }
}

pub fn authorize(token: &str) -> bool {
    !token.is_empty()
}
