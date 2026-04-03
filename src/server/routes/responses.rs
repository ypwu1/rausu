//! POST /v1/responses endpoint — Responses API passthrough proxy.
//!
//! This is the first-class path for Codex CLI via ChatGPT subscription.
//! Requests are forwarded as-is to the upstream Responses API with auth
//! and identity headers injected by the provider.

use axum::{
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::Value;
use tracing::{error, info, warn};

use crate::schema::error::ErrorResponse;
use crate::server::AppState;

/// POST /v1/responses — proxy requests to a Responses-API-capable provider.
///
/// Accepts native OpenAI Responses API format and forwards it transparently
/// to the configured provider (e.g. `chatgpt-subscription`), injecting the
/// appropriate authentication headers. The response (streaming or not) is
/// byte-proxied back to the client without modification.
pub async fn responses(State(state): State<AppState>, Json(mut body): Json<Value>) -> Response {
    handle_responses(state, &mut body).await
}

/// POST /v1/responses/compact — same passthrough as /v1/responses.
///
/// The ChatGPT backend does not expose a separate "compact" endpoint, so
/// this route forwards to the same upstream. Clients requesting compact
/// responses will receive standard (non-compact) output.
pub async fn responses_compact(
    State(state): State<AppState>,
    Json(mut body): Json<Value>,
) -> Response {
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

    // Preserve content-type from upstream if present; fall back to a sensible default.
    // For error responses the upstream sends JSON regardless of the stream flag.
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
            "Responses proxy succeeded"
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

    (
        upstream_status,
        [(axum::http::header::CONTENT_TYPE, content_type)],
        Body::from_stream(upstream.bytes_stream()),
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
        registry: Vec<(String, String, String)>,
    ) -> Router {
        let mut map = std::collections::HashMap::new();
        for (name, pname, pmodel) in registry {
            map.insert(name, (pname, pmodel));
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
        let app = make_app(
            vec![Box::new(StubProvider {
                provider_name: "anthropic",
            })],
            vec![(
                "claude-3".to_string(),
                "anthropic".to_string(),
                "claude-3-haiku".to_string(),
            )],
        );
        let resp = post_json(
            app,
            "/v1/responses",
            r#"{"model": "claude-3", "input": "Hello"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn test_chatgpt_subscription_provider_returns_200() {
        let app = make_app(
            vec![Box::new(ResponsesCapableStubProvider {
                provider_name: "chatgpt-subscription",
            })],
            vec![(
                "gpt-5.4".to_string(),
                "chatgpt-subscription".to_string(),
                "gpt-5.4".to_string(),
            )],
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
            vec![(
                "gpt-4o".to_string(),
                "openai".to_string(),
                "gpt-4o".to_string(),
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
        // Upstream returns 429; the route must proxy it through, not replace with 500.
        let app = make_app(
            vec![Box::new(UpstreamErrorStubProvider {
                provider_name: "openai",
                upstream_status: 429,
            })],
            vec![(
                "gpt-4o".to_string(),
                "openai".to_string(),
                "gpt-4o".to_string(),
            )],
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
        // When upstream sets a specific content-type, it should be forwarded.
        let app = make_app(
            vec![Box::new(ResponsesCapableStubProvider {
                provider_name: "openai",
            })],
            vec![(
                "gpt-4o".to_string(),
                "openai".to_string(),
                "gpt-4o".to_string(),
            )],
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
