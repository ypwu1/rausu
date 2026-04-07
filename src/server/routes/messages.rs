//! POST /v1/messages endpoint — transparent Anthropic Messages API proxy.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::Value;
use tracing::{error, info, warn};

use crate::providers::{is_retryable_status, Capability};
use crate::schema::error::ErrorResponse;
use crate::server::AppState;

/// POST /v1/messages — proxy requests to an Anthropic-compatible provider.
///
/// Accepts native Anthropic Messages API format and forwards it transparently
/// to the configured `anthropic` or `claude-subscription` provider, injecting
/// the appropriate authentication headers. The response (streaming or not) is
/// byte-proxied back to the client without modification.
///
/// When multiple providers are configured for a model, they are tried in
/// priority order. Retryable errors (429, 5xx, transport failures) trigger
/// failover to the next provider.
pub async fn messages(
    State(state): State<AppState>,
    req_headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    // Forward the client's anthropic-beta header so features like context_management
    // (added in Claude Code 2.1.87+) are accepted by the upstream API.
    let client_betas = req_headers
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

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
            warn!(model = %model_name, "No provider found for model in /v1/messages");
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
    let mut capability_skipped: usize = 0;

    for (attempt, (provider_name, provider_model)) in provider_list.iter().enumerate() {
        // Resolve the provider instance.
        let provider = match state.providers.iter().find(|p| p.name() == *provider_name) {
            Some(p) => p,
            None => {
                error!(provider = %provider_name, "Provider configured in registry but not instantiated");
                continue;
            }
        };

        // Capability pre-check: skip providers that don't support the Messages API
        if !provider.has_capability(Capability::MessagesApi) {
            warn!(
                model = %model_name,
                provider = %provider_name,
                "Provider does not support Messages API, skipping"
            );
            capability_skipped += 1;
            continue;
        }

        info!(model = %model_name, provider = %provider_name, attempt = attempt + 1, "Trying provider");
        providers_tried.push(provider_name.clone());

        // Replace the virtual model name with the upstream model name before forwarding.
        body["model"] = Value::String(provider_model.clone());

        let upstream = match provider
            .proxy_messages(body.clone(), is_stream, client_betas.clone())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let status_code = e.status_code();
                if e.is_retryable() && attempt + 1 < total_providers {
                    warn!(model = %model_name, provider = %provider_name, status = status_code, "Provider failed, falling back");
                    continue;
                }
                error!(error = %e, provider = %provider_name, "Messages proxy error");
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
                "Upstream returned non-2xx for messages proxy"
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
    if capability_skipped > 0 && providers_tried.is_empty() {
        warn!(model = %model_name, "No provider supports the Messages API");
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse::unsupported_capability(format!(
                "No provider for model '{}' supports the required capability: messages_api. \
                 Use /v1/chat/completions instead.",
                model_name
            ))),
        )
            .into_response();
    }

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

    /// A stub provider that only knows its name and exposes no real functionality.
    struct StubProvider {
        provider_name: &'static str,
    }

    /// A stub that returns a synthetic upstream response with a given status code.
    struct MessagesCapableStubProvider {
        provider_name: &'static str,
        upstream_status: u16,
    }

    #[async_trait]
    impl Provider for MessagesCapableStubProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        fn capabilities(&self) -> &'static [Capability] {
            use Capability::*;
            &[ChatCompletions, Streaming, MessagesApi]
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

        async fn proxy_messages(
            &self,
            _body: serde_json::Value,
            _is_stream: bool,
            _client_betas: Option<String>,
        ) -> Result<reqwest::Response, ProviderError> {
            let http_resp = axum::http::Response::builder()
                .status(self.upstream_status)
                .header("content-type", "application/json")
                .body(bytes::Bytes::from(r#"{"type":"message","content":[]}"#))
                .unwrap();
            Ok(reqwest::Response::from(http_resp))
        }
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
        // proxy_messages uses the default (Unsupported) implementation.
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
            .route("/v1/messages", post(messages))
            .with_state(state)
    }

    /// Convenience: build a registry entry with a single provider.
    fn single(name: &str, pname: &str, pmodel: &str) -> (String, Vec<(String, String)>) {
        (
            name.to_string(),
            vec![(pname.to_string(), pmodel.to_string())],
        )
    }

    async fn post_json(app: Router, body: &str) -> axum::http::Response<Body> {
        use tower::ServiceExt;
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        app.oneshot(request).await.unwrap()
    }

    #[tokio::test]
    async fn test_missing_model_field_returns_400() {
        let app = make_app(vec![], vec![]);
        let resp = post_json(app, r#"{"messages": [], "max_tokens": 100}"#).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_unknown_model_returns_404() {
        let app = make_app(vec![], vec![]);
        let resp = post_json(
            app,
            r#"{"model": "no-such-model", "messages": [], "max_tokens": 100}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_non_messages_provider_returns_422() {
        // Provider without MessagesApi capability → 422 unsupported_capability.
        let app = make_app(
            vec![Box::new(StubProvider {
                provider_name: "openai",
            })],
            vec![single("gpt-4o", "openai", "gpt-4o")],
        );
        let resp = post_json(
            app,
            r#"{"model": "gpt-4o", "messages": [], "max_tokens": 100}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn test_upstream_error_status_is_proxied_through() {
        // Single provider: upstream returns 429; the route must proxy it through.
        let app = make_app(
            vec![Box::new(MessagesCapableStubProvider {
                provider_name: "anthropic",
                upstream_status: 429,
            })],
            vec![single("claude-3", "anthropic", "claude-3-haiku-20240307")],
        );
        let resp = post_json(
            app,
            r#"{"model": "claude-3", "messages": [], "max_tokens": 100}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_upstream_success_is_proxied_through() {
        let app = make_app(
            vec![Box::new(MessagesCapableStubProvider {
                provider_name: "anthropic",
                upstream_status: 200,
            })],
            vec![single("claude-3", "anthropic", "claude-3-haiku-20240307")],
        );
        let resp = post_json(
            app,
            r#"{"model": "claude-3", "messages": [], "max_tokens": 100}"#,
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
        // First provider returns 429, second succeeds.
        let app = make_app(
            vec![
                Box::new(MessagesCapableStubProvider {
                    provider_name: "anthropic",
                    upstream_status: 429,
                }),
                Box::new(MessagesCapableStubProvider {
                    provider_name: "claude-subscription",
                    upstream_status: 200,
                }),
            ],
            vec![(
                "claude-3".to_string(),
                vec![
                    ("anthropic".to_string(), "claude-3-haiku".to_string()),
                    (
                        "claude-subscription".to_string(),
                        "claude-3-haiku".to_string(),
                    ),
                ],
            )],
        );
        let resp = post_json(
            app,
            r#"{"model": "claude-3", "messages": [], "max_tokens": 100}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_failover_500_to_second_provider() {
        let app = make_app(
            vec![
                Box::new(MessagesCapableStubProvider {
                    provider_name: "anthropic",
                    upstream_status: 500,
                }),
                Box::new(MessagesCapableStubProvider {
                    provider_name: "claude-subscription",
                    upstream_status: 200,
                }),
            ],
            vec![(
                "claude-3".to_string(),
                vec![
                    ("anthropic".to_string(), "claude-3-haiku".to_string()),
                    (
                        "claude-subscription".to_string(),
                        "claude-3-haiku".to_string(),
                    ),
                ],
            )],
        );
        let resp = post_json(
            app,
            r#"{"model": "claude-3", "messages": [], "max_tokens": 100}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_no_failover_on_400() {
        // 400 is not retryable — should be returned immediately even if more providers exist.
        let app = make_app(
            vec![
                Box::new(MessagesCapableStubProvider {
                    provider_name: "anthropic",
                    upstream_status: 400,
                }),
                Box::new(MessagesCapableStubProvider {
                    provider_name: "claude-subscription",
                    upstream_status: 200,
                }),
            ],
            vec![(
                "claude-3".to_string(),
                vec![
                    ("anthropic".to_string(), "claude-3-haiku".to_string()),
                    (
                        "claude-subscription".to_string(),
                        "claude-3-haiku".to_string(),
                    ),
                ],
            )],
        );
        let resp = post_json(
            app,
            r#"{"model": "claude-3", "messages": [], "max_tokens": 100}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_all_providers_fail_returns_503() {
        let app = make_app(
            vec![
                Box::new(MessagesCapableStubProvider {
                    provider_name: "anthropic",
                    upstream_status: 503,
                }),
                Box::new(MessagesCapableStubProvider {
                    provider_name: "claude-subscription",
                    upstream_status: 502,
                }),
            ],
            vec![(
                "claude-3".to_string(),
                vec![
                    ("anthropic".to_string(), "claude-3-haiku".to_string()),
                    (
                        "claude-subscription".to_string(),
                        "claude-3-haiku".to_string(),
                    ),
                ],
            )],
        );
        let resp = post_json(
            app,
            r#"{"model": "claude-3", "messages": [], "max_tokens": 100}"#,
        )
        .await;
        // Last provider returned 502, which is retryable but no more providers, so we get it.
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
