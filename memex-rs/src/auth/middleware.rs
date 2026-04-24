//! API Key 验证中间件

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

pub type UserLookupFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct AuthenticatedUser(pub String);

#[derive(Clone)]
pub struct AuthState {
    keys: HashMap<String, String>, // key_hash → username
    dynamic_lookup: Option<UserLookupFn>,
}

impl AuthState {
    pub fn from_users(users: &[(String, String)]) -> Self {
        let keys = users
            .iter()
            .map(|(name, key)| (hash_key(key), name.clone()))
            .collect();
        Self { keys, dynamic_lookup: None }
    }

    pub fn with_dynamic_lookup(
        users: &[(String, String)],
        lookup: UserLookupFn,
    ) -> Self {
        let keys = users
            .iter()
            .map(|(name, key)| (hash_key(key), name.clone()))
            .collect();
        Self { keys, dynamic_lookup: Some(lookup) }
    }

    pub fn verify(&self, key: &str) -> Option<String> {
        let h = hash_key(key);
        if let Some(name) = self.keys.get(&h) {
            return Some(name.clone());
        }
        if let Some(ref lookup) = self.dynamic_lookup {
            return lookup(key);
        }
        None
    }
}

fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub async fn auth_layer(
    mut request: Request,
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

    match state.verify(key) {
        Some(username) => {
            request.extensions_mut().insert(AuthenticatedUser(username));
            next.run(request).await
        }
        None => (StatusCode::UNAUTHORIZED, "Invalid API key").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_key() {
        let state = AuthState::from_users(&[
            ("alice".to_string(), "mk_test_key_123".to_string()),
        ]);
        assert_eq!(state.verify("mk_test_key_123").as_deref(), Some("alice"));
        assert_eq!(state.verify("wrong_key"), None);
    }

    #[test]
    fn test_multiple_users() {
        let state = AuthState::from_users(&[
            ("alice".to_string(), "key_alice".to_string()),
            ("bob".to_string(), "key_bob".to_string()),
        ]);
        assert_eq!(state.verify("key_alice").as_deref(), Some("alice"));
        assert_eq!(state.verify("key_bob").as_deref(), Some("bob"));
        assert_eq!(state.verify("key_eve"), None);
    }

    #[test]
    fn test_dynamic_lookup() {
        let lookup: UserLookupFn = Arc::new(|key: &str| {
            if key == "dynamic_key_123" {
                Some("dynamic_user".to_string())
            } else {
                None
            }
        });
        let state = AuthState::with_dynamic_lookup(
            &[("alice".to_string(), "key_alice".to_string())],
            lookup,
        );
        assert_eq!(state.verify("key_alice").as_deref(), Some("alice"));
        assert_eq!(state.verify("dynamic_key_123").as_deref(), Some("dynamic_user"));
        assert_eq!(state.verify("unknown"), None);
    }
}
