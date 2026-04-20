//! API Key 验证中间件

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone)]
pub struct AuthState {
    api_key_hashes: Vec<String>,
}

impl AuthState {
    pub fn new(api_keys: &[String]) -> Self {
        let hashes = api_keys.iter().map(|k| hash_key(k)).collect();
        Self {
            api_key_hashes: hashes,
        }
    }

    fn verify(&self, key: &str) -> bool {
        let h = hash_key(key);
        self.api_key_hashes.iter().any(|stored| stored == &h)
    }
}

fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub async fn auth_layer(
    request: Request,
    next: Next,
) -> Response {
    let auth_state = request
        .extensions()
        .get::<Arc<AuthState>>()
        .cloned();

    let Some(state) = auth_state else {
        return next.run(request).await;
    };

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let Some(header) = auth_header else {
        return (StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response();
    };

    let key = header.strip_prefix("Bearer ").unwrap_or(header);

    if !state.verify(key) {
        return (StatusCode::UNAUTHORIZED, "Invalid API key").into_response();
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_key() {
        let state = AuthState::new(&["mk_test_key_123".to_string()]);
        assert!(state.verify("mk_test_key_123"));
        assert!(!state.verify("wrong_key"));
    }

    #[test]
    fn test_multiple_keys() {
        let state = AuthState::new(&[
            "key_alice".to_string(),
            "key_bob".to_string(),
        ]);
        assert!(state.verify("key_alice"));
        assert!(state.verify("key_bob"));
        assert!(!state.verify("key_eve"));
    }
}
