//! POST /v1/responses endpoint — Responses API passthrough proxy.
//!
//! This is the first-class path for Codex CLI via ChatGPT subscription.
//! Requests are forwarded as-is to the upstream Responses API with auth
//! and identity headers injected by the provider.
//!
//! Uses raw body extraction (not Axum's `Json<Value>`) to handle edge
//! cases such as Codex CLI WebSocket→HTTPS fallback sending unusual bodies.

use axum::{
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::schema::error::ErrorResponse;
use crate::server::AppState;

/// Maximum request body size (200 MB).
const MAX_BODY_SIZE: usize = 200 * 1024 * 1024;

/// Extract raw body bytes from a request, log diagnostics, and parse as JSON.
async fn extract_json_body(request: axum::extract::Request) -> Result<Value, Response> {
    let (parts, body) = request.into_parts();
    let headers = &parts.headers;

    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<missing>");
    let content_encoding = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok());

    let body_bytes = match axum::body::to_bytes(body, MAX_BODY_SIZE).await {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "Failed to read request body");
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::invalid_request(format!(
                    "Failed to read request body: {e}"
                ))),
            )
                .into_response());
        }
    };

    debug!(
        body_len = body_bytes.len(),
        content_type = content_type,
        content_encoding = content_encoding.unwrap_or("<none>"),
        method = %parts.method,
        uri = %parts.uri,
        "Received /v1/responses request"
    );

    if body_bytes.is_empty() {
        warn!("Empty request body on /v1/responses");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::invalid_request(
                "Empty request body. If using Codex CLI, ensure OPENAI_BASE_URL is set correctly (e.g. http://localhost:4000/v1)."
            )),
        )
            .into_response());
    }

    // Try parsing as JSON directly.
    match serde_json::from_slice::<Value>(&body_bytes) {
        Ok(v) => Ok(v),
        Err(json_err) => {
            // Log diagnostic info for debugging.
            let preview: String = body_bytes
                .iter()
                .take(100)
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ");
            warn!(
                body_len = body_bytes.len(),
                body_preview_hex = %preview,
                error = %json_err,
                "Failed to parse request body as JSON"
            );
            Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::invalid_request(format!(
                    "Failed to parse the request body as JSON: {json_err} (body length: {} bytes, first bytes hex: {preview})",
                    body_bytes.len()
                ))),
            )
                .into_response())
        }
    }
}

/// POST /v1/responses — proxy requests to a Responses-API-capable provider.
pub async fn responses(
    State(state): State<AppState>,
    request: axum::extract::Request,
) -> Response {
    let mut body = match extract_json_body(request).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    handle_responses(state, &mut body).await
}

/// POST /v1/responses/compact — same passthrough as /v1/responses.
pub async fn responses_compact(
    State(state): State<AppState>,
    request: axum::extract::Request,
) -> Response {
    let mut body = match extract_json_body(request).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    handle_responses(state, &mut body).await
}

/// Shared handler for both /v1/responses and /v1/responses/compact.
async fn handle_responses(state: AppState, body: &mut Value) -> Response {
    let model_name = match body.get("model").and_then(Value::as_str) {
        Some(m) => m.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::invalid_request(
                    "Missing required field: 'model'",
                )),
            )
                .into_response();
        }
    };

    let is_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);

    // Look up the model in the registry.
    let (provider_name, provider_model) = match state.model_registry.get(&model_name) {
        Some((pname, pmodel)) => (pname.clone(), pmodel.clone()),
        None => {
            warn!(model = %model_name, "No provider found for model in /v1/responses");
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse::invalid_request(format!(
                    "Model '{}' not found. Check your configuration.",
                    model_name
                ))),
            )
                .into_response();
        }
    };

    // Resolve the provider instance.
    let provider = match state.providers.iter().find(|p| p.name() == provider_name) {
        Some(p) => p,
        None => {
            error!(provider = %provider_name, "Provider configured in registry but not instantiated");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::internal("Provider not configured")),
            )
                .into_response();
        }
    };

    // Replace the virtual model name with the upstream model name before forwarding.
    body["model"] = Value::String(provider_model);

    let upstream = match provider.proxy_responses(body.clone(), is_stream).await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, provider = %provider_name, "Responses proxy error");
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return (status, Json(ErrorResponse::internal(e.to_string()))).into_response();
        }
    };

    // Byte-proxy the upstream response — no parsing or rewriting.
    let upstream_status = upstream.status();

    let content_type = upstream
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(if is_stream {
            "text/event-stream"
        } else {
            "application/json"
        })
        .to_string();

    if upstream_status.is_success() {
        info!(
            model = %model_name,
            provider = %provider_name,
            stream = is_stream,
            "Responses proxy success"
        );
    } else {
        warn!(
            model = %model_name,
            provider = %provider_name,
            status = upstream_status.as_u16(),
            "Upstream returned non-2xx for responses proxy"
        );
    }

    let stream = upstream.bytes_stream();
    Response::builder()
        .status(upstream_status)
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build response",
            )
                .into_response()
        })
}


// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::post, Router};
    use http::Request;
    use tower::ServiceExt;

    /// Minimal stub provider that always returns Unsupported for proxy_responses.
    struct StubProvider;

    #[async_trait::async_trait]
    impl crate::providers::Provider for StubProvider {
        fn name(&self) -> &str {
            "stub"
        }

        async fn chat_completions(
            &self,
            _req: crate::schema::chat::ChatCompletionRequest,
        ) -> Result<crate::schema::chat::ChatCompletionResponse, crate::providers::ProviderError>
        {
            unimplemented!()
        }

        async fn chat_completions_stream(
            &self,
            _req: crate::schema::chat::ChatCompletionRequest,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                crate::schema::chat::ChatCompletionChunk,
                                crate::providers::ProviderError,
                            >,
                        > + Send,
                >,
            >,
            crate::providers::ProviderError,
        > {
            unimplemented!()
        }

        fn models(&self) -> Vec<crate::schema::chat::ModelInfo> {
            vec![]
        }
    }

    fn build_app(provider_name: &str, model_name: &str) -> Router {
        let providers: Vec<Box<dyn crate::providers::Provider>> = vec![Box::new(StubProvider)];
        let mut registry = std::collections::HashMap::new();
        registry.insert(
            model_name.to_string(),
            (provider_name.to_string(), model_name.to_string()),
        );
        let state = AppState {
            providers: std::sync::Arc::new(providers),
            model_registry: std::sync::Arc::new(registry),
        };
        Router::new()
            .route("/v1/responses", post(responses))
            .route("/v1/responses/compact", post(responses_compact))
            .with_state(state)
    }

    async fn post_json(app: Router, path: &str, json: &str) -> (StatusCode, String) {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).to_string())
    }

    #[tokio::test]
    async fn missing_model_returns_400() {
        let app = build_app("stub", "test-model");
        let (status, _body) = post_json(app, "/v1/responses", r#"{"input": "Hello"}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_model_returns_404() {
        let app = build_app("stub", "known-model");
        let (status, _body) = post_json(
            app,
            "/v1/responses",
            r#"{"model": "unknown-model", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unsupported_provider_returns_405() {
        let app = build_app("stub", "test-model");
        let (status, body) = post_json(
            app,
            "/v1/responses",
            r#"{"model": "test-model", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert!(body.contains("does not support the Responses API"));
    }

    #[tokio::test]
    async fn empty_body_returns_400() {
        let app = build_app("stub", "test-model");
        let req = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("Empty request body"));
    }

    /// A provider that returns a canned Responses API reply.
    struct EchoProvider;

    #[async_trait::async_trait]
    impl crate::providers::Provider for EchoProvider {
        fn name(&self) -> &str {
            "echo"
        }

        async fn chat_completions(
            &self,
            _req: crate::schema::chat::ChatCompletionRequest,
        ) -> Result<crate::schema::chat::ChatCompletionResponse, crate::providers::ProviderError>
        {
            unimplemented!()
        }

        async fn chat_completions_stream(
            &self,
            _req: crate::schema::chat::ChatCompletionRequest,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                crate::schema::chat::ChatCompletionChunk,
                                crate::providers::ProviderError,
                            >,
                        > + Send,
                >,
            >,
            crate::providers::ProviderError,
        > {
            unimplemented!()
        }

        async fn proxy_responses(
            &self,
            _body: serde_json::Value,
            _is_stream: bool,
        ) -> Result<reqwest::Response, crate::providers::ProviderError> {
            let http_resp = http::Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(bytes::Bytes::from(r#"{"id":"resp_1","output":[{"type":"message","content":[{"type":"output_text","text":"hi"}]}]}"#))
                .unwrap();
            Ok(reqwest::Response::from(http_resp))
        }

        fn models(&self) -> Vec<crate::schema::chat::ModelInfo> {
            vec![]
        }
    }

    #[tokio::test]
    async fn successful_proxy_returns_upstream_body() {
        let providers: Vec<Box<dyn crate::providers::Provider>> = vec![Box::new(EchoProvider)];
        let mut registry = std::collections::HashMap::new();
        registry.insert(
            "test-model".to_string(),
            ("echo".to_string(), "test-model".to_string()),
        );
        let state = AppState {
            providers: std::sync::Arc::new(providers),
            model_registry: std::sync::Arc::new(registry),
        };
        let app = Router::new()
            .route("/v1/responses", post(responses))
            .with_state(state);

        let (status, body) = post_json(
            app,
            "/v1/responses",
            r#"{"model": "test-model", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("resp_1"));
    }

    #[tokio::test]
    async fn compact_route_works() {
        let app = build_app("stub", "test-model");
        let (status, body) = post_json(
            app,
            "/v1/responses/compact",
            r#"{"model": "test-model", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert!(body.contains("does not support the Responses API"));
    }
}
