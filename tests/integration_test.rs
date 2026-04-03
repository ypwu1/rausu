//! Integration tests for the Rausu gateway.

use axum::{routing::get, Router};
use serde_json::Value;
use tokio::net::TcpListener;

/// Start a test server on a random port, returning the base URL.
async fn start_test_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = server_app();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}", addr)
}

fn server_app() -> Router {
    use axum::routing::post;
    use axum::Json;
    use serde_json::json;

    let models_handler = get(|| async {
        Json(json!({
            "object": "list",
            "data": []
        }))
    });

    let echo_handler = || post(|body: String| async move { Json(json!({"echo": body})) });

    Router::new()
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        // Canonical /v1/ routes
        .route("/v1/models", models_handler.clone())
        .route("/v1/chat/completions", echo_handler())
        .route("/v1/responses", echo_handler())
        .route("/v1/responses/compact", echo_handler())
        .route("/v1/messages", echo_handler())
        // Compatibility routes without /v1/ prefix
        .route("/models", models_handler)
        .route("/chat/completions", echo_handler())
        .route("/responses", echo_handler())
        .route("/responses/compact", echo_handler())
        .route("/messages", echo_handler())
}

#[tokio::test]
async fn test_health_endpoint() {
    let base_url = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn test_models_endpoint_empty() {
    let base_url = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/v1/models", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert!(body["data"].is_array());
}

// --- Non-prefixed compatibility route tests ---

#[tokio::test]
async fn test_models_without_v1_prefix() {
    let base_url = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/models", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert!(body["data"].is_array());
}

#[tokio::test]
async fn test_responses_without_v1_prefix() {
    let base_url = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/responses", base_url))
        .body("test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_responses_compact_without_v1_prefix() {
    let base_url = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/responses/compact", base_url))
        .body("test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_chat_completions_without_v1_prefix() {
    let base_url = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/chat/completions", base_url))
        .body("test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_messages_without_v1_prefix() {
    let base_url = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/messages", base_url))
        .body("test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
