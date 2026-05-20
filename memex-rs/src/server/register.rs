//! 自注册 - 用 master_key 换取 api_key

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::IngestState;

#[derive(Clone)]
pub struct MasterKey(pub String);

#[derive(Deserialize)]
pub struct RegisterRequest {
    master_key: String,
    name: String,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    api_key: String,
    name: String,
}

async fn handle_register(
    State(state): State<Arc<IngestState>>,
    master_key: axum::Extension<MasterKey>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    if req.master_key != master_key.0 .0 {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "invalid master_key"})),
        );
    }

    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "name must be 1-64 characters"})),
        );
    }

    match state.register_user(&name) {
        Some(api_key) => (
            StatusCode::OK,
            Json(serde_json::json!(RegisterResponse { api_key, name })),
        ),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "registration failed"})),
        ),
    }
}

pub fn create_register_router(state: Arc<IngestState>, master_key: String) -> Router {
    Router::new()
        .route("/api/auth/register", post(handle_register))
        .layer(axum::Extension(MasterKey(master_key)))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Arc<IngestState>) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("server.db");
        let state = Arc::new(IngestState::new(db_path.to_str().unwrap()).unwrap());
        (dir, state)
    }

    #[test]
    fn test_register_new_user() {
        let (_dir, state) = setup();
        let key = state.register_user("alice").unwrap();
        assert!(key.starts_with("mk_"));
    }

    #[test]
    fn test_register_idempotent() {
        let (_dir, state) = setup();
        let key1 = state.register_user("bob").unwrap();
        let key2 = state.register_user("bob").unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_lookup_registered_user() {
        let (_dir, state) = setup();
        let key = state.register_user("carol").unwrap();
        let name = state.lookup_registered_user_by_key(&key);
        assert_eq!(name.as_deref(), Some("carol"));
    }

    #[test]
    fn test_lookup_unknown_key() {
        let (_dir, state) = setup();
        assert_eq!(state.lookup_registered_user_by_key("mk_nonexistent"), None);
    }

    #[test]
    fn test_different_users_get_different_keys() {
        let (_dir, state) = setup();
        let key1 = state.register_user("alice").unwrap();
        let key2 = state.register_user("bob").unwrap();
        assert_ne!(key1, key2);
    }
}
