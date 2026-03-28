//! Health check endpoint.

use axum::Json;
use serde_json::{json, Value};

/// GET /health — returns `{"status": "ok"}`.
pub async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
