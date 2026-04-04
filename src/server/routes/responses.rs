//! POST /v1/responses endpoint — Responses API passthrough proxy.
//!
//! This is the first-class path for Codex CLI via ChatGPT subscription.
//! Requests are forwarded as-is to the upstream Responses API with auth
//! and identity headers injected by the provider.
//!
//! Uses raw body extraction instead of Axum's `Json<Value>` extractor to
//! handle edge cases where Codex CLI falls back from WebSocket to HTTPS
//! and the body may be empty or compressed.

use std::io::Read as _;

use axum::{
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use flate2::read::{DeflateDecoder, GzDecoder};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::providers::is_retryable_status;
use crate::schema::error::ErrorResponse;
use crate::server::AppState;

/// Maximum request body size (200 MB).
const MAX_BODY_SIZE: usize = 200 * 1024 * 1024;

/// POST /v1/responses — proxy requests to a Responses-API-capable provider.
///
/// Accepts native OpenAI Responses API format and forwards it transparently
/// to the configured provider (e.g. `chatgpt-subscription`), injecting the
/// appropriate authentication headers. The response (streaming or not) is
/// byte-proxied back to the client without modification.
pub async fn responses(State(state): State<AppState>, request: axum::extract::Request) -> Response {
    match extract_json_body(request).await {
        Ok(mut body) => handle_responses(state, &mut body).await,
        Err(resp) => resp,
    }
}

/// POST /v1/responses/compact — same passthrough as /v1/responses.
///
/// The ChatGPT backend does not expose a separate "compact" endpoint, so
/// this route forwards to the same upstream. Clients requesting compact
/// responses will receive standard (non-compact) output.
pub async fn responses_compact(
    State(state): State<AppState>,
    request: axum::extract::Request,
) -> Response {
    match extract_json_body(request).await {
        Ok(mut body) => handle_responses(state, &mut body).await,
        Err(resp) => resp,
    }
}

/// Extract and parse JSON body from the raw request, with decompression support.
async fn extract_json_body(request: axum::extract::Request) -> Result<Value, Response> {
    let (parts, req_body) = request.into_parts();
    let headers = &parts.headers;

    let body_bytes = match axum::body::to_bytes(req_body, MAX_BODY_SIZE).await {
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
        content_type = ?headers.get("content-type"),
        content_encoding = ?headers.get("content-encoding"),
        "Received /v1/responses request"
    );

    if body_bytes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::invalid_request(
                "Empty request body. If using Codex CLI, ensure OPENAI_BASE_URL is set correctly.",
            )),
        )
            .into_response());
    }

    // Try parsing the raw bytes as JSON first.
    if let Ok(value) = serde_json::from_slice::<Value>(&body_bytes) {
        return Ok(value);
    }

    // If JSON parse failed and content-encoding is present, try decompressing.
    if let Some(encoding) = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
    {
        let decompressed = try_decompress(encoding, &body_bytes);

        if let Some(data) = decompressed {
            if let Ok(value) = serde_json::from_slice::<Value>(&data) {
                return Ok(value);
            }
        }
    }

    // Auto-detect zstd by magic bytes (28 b5 2f fd) when no content-encoding header.
    if body_bytes.len() >= 4 && body_bytes[..4] == [0x28, 0xb5, 0x2f, 0xfd] {
        if let Some(data) = try_decompress("zstd", &body_bytes) {
            if let Ok(value) = serde_json::from_slice::<Value>(&data) {
                return Ok(value);
            }
        }
    }

    // All parsing attempts failed — return a diagnostic error.
    let preview_len = body_bytes.len().min(100);
    let hex_preview: String = body_bytes[..preview_len]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    Err((
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse::invalid_request(format!(
            "Failed to parse request body as JSON (len={}, first bytes: {hex_preview})",
            body_bytes.len()
        ))),
    )
        .into_response())
}

/// Try to decompress body bytes according to the given encoding.
fn try_decompress(encoding: &str, data: &[u8]) -> Option<Vec<u8>> {
    match encoding {
        "gzip" => {
            let mut decoder = GzDecoder::new(data);
            let mut buf = Vec::new();
            decoder.read_to_end(&mut buf).ok().map(|_| buf)
        }
        "deflate" => {
            let mut decoder = DeflateDecoder::new(data);
            let mut buf = Vec::new();
            decoder.read_to_end(&mut buf).ok().map(|_| buf)
        }
        "zstd" => zstd::decode_all(data).ok(),
        _ => None,
    }
}

/// Shared handler for both /v1/responses and /v1/responses/compact.
///
/// When multiple providers are configured for a model, they are tried in
/// priority order. Retryable errors (429, 5xx, transport failures) trigger
/// failover to the next provider.
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
    let provider_list = match state.model_registry.get(&model_name) {
        Some(list) => list.clone(),
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

    let total_providers = provider_list.len();
    let mut providers_tried: Vec<String> = Vec::new();

    for (attempt, (provider_name, provider_model)) in provider_list.iter().enumerate() {
        // Resolve the provider instance.
        let provider = match state.providers.iter().find(|p| p.name() == *provider_name) {
            Some(p) => p,
            None => {
                error!(provider = %provider_name, "Provider configured in registry but not instantiated");
                continue;
            }
        };

        info!(model = %model_name, provider = %provider_name, attempt = attempt + 1, "Trying provider");
        providers_tried.push(provider_name.clone());

        // Replace the virtual model name with the upstream model name before forwarding.
        body["model"] = Value::String(provider_model.clone());

        let upstream = match provider.proxy_responses(body.clone(), is_stream).await {
            Ok(r) => r,
            Err(e) => {
                let status_code = e.status_code();
                if e.is_retryable() && attempt + 1 < total_providers {
                    warn!(model = %model_name, provider = %provider_name, status = status_code, "Provider failed, falling back");
                    continue;
                }
                error!(error = %e, provider = %provider_name, "Responses proxy error");
                let status =
                    StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                return (status, Json(ErrorResponse::internal(e.to_string()))).into_response();
            }
        };

        // Check upstream HTTP status before streaming — retryable statuses trigger failover.
        let upstream_status = upstream.status();
        if !upstream_status.is_success()
            && is_retryable_status(upstream_status.as_u16())
            && attempt + 1 < total_providers
        {
            warn!(model = %model_name, provider = %provider_name, status = upstream_status.as_u16(), "Provider failed, falling back");
            continue;
        }

        // Preserve content-type from upstream if present; fall back to a sensible default.
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
                status = upstream_status.as_u16(),
                stream = is_stream,
                "Request served by provider"
            );
        } else {
            warn!(
                model = %model_name,
                provider = %provider_name,
                status = upstream_status.as_u16(),
                stream = is_stream,
                "Upstream returned non-2xx for responses proxy"
            );
        }

        return (
            upstream_status,
            [(axum::http::header::CONTENT_TYPE, content_type)],
            Body::from_stream(upstream.bytes_stream()),
        )
            .into_response();
    }

    // All providers exhausted.
    error!(model = %model_name, providers_tried = ?providers_tried, "All providers failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse::internal(format!(
            "All providers failed for model '{}'. Tried: {}",
            model_name,
            providers_tried.join(", ")
        ))),
    )
        .into_response()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::{body::Body, routing::post, Router};
    use futures::Stream;

    use crate::providers::{Provider, ProviderError};
    use crate::schema::chat::{
        ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ModelInfo,
    };
    use crate::server::AppState;

    /// A stub provider with no capabilities — `proxy_responses` uses the default (Unsupported).
    struct StubProvider {
        provider_name: &'static str,
    }

    #[async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        fn models(&self) -> Vec<ModelInfo> {
            vec![]
        }

        async fn chat_completions(
            &self,
            _req: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, ProviderError> {
            Err(ProviderError::Unsupported("stub".to_string()))
        }

        async fn chat_completions_stream(
            &self,
            _req: ChatCompletionRequest,
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
            ProviderError,
        > {
            Err(ProviderError::Unsupported("stub".to_string()))
        }
        // proxy_responses uses the default (Unsupported) implementation.
    }

    /// A stub provider that claims Responses API capability by returning a synthetic 200.
    struct ResponsesCapableStubProvider {
        provider_name: &'static str,
    }

    #[async_trait]
    impl Provider for ResponsesCapableStubProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        fn models(&self) -> Vec<ModelInfo> {
            vec![]
        }

        async fn chat_completions(
            &self,
            _req: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, ProviderError> {
            Err(ProviderError::Unsupported("stub".to_string()))
        }

        async fn chat_completions_stream(
            &self,
            _req: ChatCompletionRequest,
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
            ProviderError,
        > {
            Err(ProviderError::Unsupported("stub".to_string()))
        }

        async fn proxy_responses(
            &self,
            _body: serde_json::Value,
            _is_stream: bool,
        ) -> Result<reqwest::Response, ProviderError> {
            let http_resp = axum::http::Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(bytes::Bytes::from(r#"{"id":"resp_test"}"#))
                .unwrap();
            Ok(reqwest::Response::from(http_resp))
        }
    }

    fn make_app(
        providers: Vec<Box<dyn Provider>>,
        registry: Vec<(String, Vec<(String, String)>)>,
    ) -> Router {
        let mut map = std::collections::HashMap::new();
        for (name, entries) in registry {
            map.insert(name, entries);
        }
        let state = AppState {
            providers: Arc::new(providers),
            model_registry: Arc::new(map),
        };
        Router::new()
            .route("/v1/responses", post(responses))
            .route("/v1/responses/compact", post(responses_compact))
            .with_state(state)
    }

    fn single(name: &str, pname: &str, pmodel: &str) -> (String, Vec<(String, String)>) {
        (
            name.to_string(),
            vec![(pname.to_string(), pmodel.to_string())],
        )
    }

    async fn post_json(app: Router, uri: &str, body: &str) -> axum::http::Response<Body> {
        use tower::ServiceExt;
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        app.oneshot(request).await.unwrap()
    }

    #[tokio::test]
    async fn test_missing_model_field_returns_400() {
        let app = make_app(vec![], vec![]);
        let resp = post_json(app, "/v1/responses", r#"{"input": "Hello"}"#).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_unknown_model_returns_404() {
        let app = make_app(vec![], vec![]);
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "no-such-model", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_unsupported_provider_returns_405() {
        // StubProvider does not override proxy_responses, so it returns Unsupported → 405.
        // With single provider, Unsupported is the final answer.
        let app = make_app(
            vec![Box::new(StubProvider {
                provider_name: "anthropic",
            })],
            vec![single("claude-3", "anthropic", "claude-3-haiku")],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "claude-3", "input": "Hello"}"#,
        )
        .await;
        // Single provider: Unsupported is retryable but no fallback available → 405.
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn test_chatgpt_subscription_provider_returns_200() {
        let app = make_app(
            vec![Box::new(ResponsesCapableStubProvider {
                provider_name: "chatgpt-subscription",
            })],
            vec![single("gpt-5.4", "chatgpt-subscription", "gpt-5.4")],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "gpt-5.4", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_openai_provider_returns_200() {
        let app = make_app(
            vec![Box::new(ResponsesCapableStubProvider {
                provider_name: "openai",
            })],
            vec![single("gpt-4o", "openai", "gpt-4o")],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "gpt-4o", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A stub provider that returns a synthetic upstream error (e.g. 429 rate limit).
    struct UpstreamErrorStubProvider {
        provider_name: &'static str,
        upstream_status: u16,
    }

    #[async_trait]
    impl Provider for UpstreamErrorStubProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        fn models(&self) -> Vec<ModelInfo> {
            vec![]
        }

        async fn chat_completions(
            &self,
            _req: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, ProviderError> {
            Err(ProviderError::Unsupported("stub".to_string()))
        }

        async fn chat_completions_stream(
            &self,
            _req: ChatCompletionRequest,
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
            ProviderError,
        > {
            Err(ProviderError::Unsupported("stub".to_string()))
        }

        async fn proxy_responses(
            &self,
            _body: serde_json::Value,
            _is_stream: bool,
        ) -> Result<reqwest::Response, ProviderError> {
            let http_resp = axum::http::Response::builder()
                .status(self.upstream_status)
                .header("content-type", "application/json")
                .body(bytes::Bytes::from(r#"{"error":"upstream error"}"#))
                .unwrap();
            Ok(reqwest::Response::from(http_resp))
        }
    }

    #[tokio::test]
    async fn test_upstream_error_status_is_proxied_through() {
        // Single provider: upstream returns 429; the route must proxy it through.
        let app = make_app(
            vec![Box::new(UpstreamErrorStubProvider {
                provider_name: "openai",
                upstream_status: 429,
            })],
            vec![single("gpt-4o", "openai", "gpt-4o")],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "gpt-4o", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_upstream_content_type_is_preserved() {
        let app = make_app(
            vec![Box::new(ResponsesCapableStubProvider {
                provider_name: "openai",
            })],
            vec![single("gpt-4o", "openai", "gpt-4o")],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "gpt-4o", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn test_failover_429_to_second_provider() {
        let app = make_app(
            vec![
                Box::new(UpstreamErrorStubProvider {
                    provider_name: "openai",
                    upstream_status: 429,
                }),
                Box::new(ResponsesCapableStubProvider {
                    provider_name: "chatgpt-subscription",
                }),
            ],
            vec![(
                "gpt-4o".to_string(),
                vec![
                    ("openai".to_string(), "gpt-4o".to_string()),
                    ("chatgpt-subscription".to_string(), "gpt-4o".to_string()),
                ],
            )],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "gpt-4o", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_no_failover_on_400() {
        let app = make_app(
            vec![
                Box::new(UpstreamErrorStubProvider {
                    provider_name: "openai",
                    upstream_status: 400,
                }),
                Box::new(ResponsesCapableStubProvider {
                    provider_name: "chatgpt-subscription",
                }),
            ],
            vec![(
                "gpt-4o".to_string(),
                vec![
                    ("openai".to_string(), "gpt-4o".to_string()),
                    ("chatgpt-subscription".to_string(), "gpt-4o".to_string()),
                ],
            )],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "gpt-4o", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_unsupported_skips_to_next_provider() {
        // First provider doesn't support responses API, second does.
        let app = make_app(
            vec![
                Box::new(StubProvider {
                    provider_name: "anthropic",
                }),
                Box::new(ResponsesCapableStubProvider {
                    provider_name: "openai",
                }),
            ],
            vec![(
                "my-model".to_string(),
                vec![
                    ("anthropic".to_string(), "my-model".to_string()),
                    ("openai".to_string(), "my-model".to_string()),
                ],
            )],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "my-model", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_compact_missing_model_returns_400() {
        let app = make_app(vec![], vec![]);
        let resp = post_json(app, "/v1/responses/compact", r#"{"input": "Hello"}"#).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_compact_unknown_model_returns_404() {
        let app = make_app(vec![], vec![]);
        let resp = post_json(
            app,
            "/v1/responses/compact",
            r#"{"model": "no-such-model", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
