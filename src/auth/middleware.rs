//! API key authentication middleware.

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};

use crate::schema::error::ErrorResponse;

/// Shared set of valid API keys for O(1) lookup.
#[derive(Clone)]
pub struct AuthState {
    /// When `None`, authentication is disabled (all requests pass through).
    valid_keys: Option<Arc<HashSet<String>>>,
}

impl AuthState {
    /// Create a disabled (no-op) auth state.
    pub fn disabled() -> Self {
        Self { valid_keys: None }
    }

    /// Create a static-key auth state from an iterator of key strings.
    pub fn from_keys(keys: impl IntoIterator<Item = String>) -> Self {
        Self {
            valid_keys: Some(Arc::new(keys.into_iter().collect())),
        }
    }
}

/// Axum middleware that enforces Bearer-token authentication.
///
/// Exempt paths (`/health`, `/`) are always allowed through.
pub async fn auth_middleware(
    axum::extract::State(auth): axum::extract::State<AuthState>,
    req: Request,
    next: Next,
) -> Response {
    // Auth disabled → pass through immediately
    let valid_keys = match &auth.valid_keys {
        Some(keys) => keys,
        None => return next.run(req).await,
    };

    // Exempt paths never require auth
    let path = req.uri().path();
    if path == "/health" || path == "/" {
        return next.run(req).await;
    }

    // Extract Bearer token
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) if valid_keys.contains(t) => next.run(req).await,
        _ => {
            let body = ErrorResponse::new("Invalid API key", "auth_error");
            (StatusCode::UNAUTHORIZED, Json(body)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        middleware,
        routing::{get, post},
        Router,
    };
    use http::Request as HttpRequest;
    use tower::ServiceExt;

    fn test_router(auth_state: AuthState) -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/v1/chat/completions", post(|| async { "chat" }))
            .route("/v1/models", get(|| async { "models" }))
            .route_layer(middleware::from_fn_with_state(
                auth_state.clone(),
                auth_middleware,
            ))
            .with_state(auth_state)
    }

    #[tokio::test]
    async fn test_disabled_allows_all() {
        let app = test_router(AuthState::disabled());

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_static_allows_valid_key() {
        let auth = AuthState::from_keys(vec!["rausu-sk-abc123".to_string()]);
        let app = test_router(auth);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("authorization", "Bearer rausu-sk-abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_static_rejects_invalid_key() {
        let auth = AuthState::from_keys(vec!["rausu-sk-abc123".to_string()]);
        let app = test_router(auth);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("authorization", "Bearer wrong-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_static_rejects_missing_header() {
        let auth = AuthState::from_keys(vec!["rausu-sk-abc123".to_string()]);
        let app = test_router(auth);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/v1/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_health_exempt_in_static_mode() {
        let auth = AuthState::from_keys(vec!["rausu-sk-abc123".to_string()]);
        let app = test_router(auth);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_static_rejects_non_bearer_auth() {
        let auth = AuthState::from_keys(vec!["rausu-sk-abc123".to_string()]);
        let app = test_router(auth);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("authorization", "Basic rausu-sk-abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
