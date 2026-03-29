//! POST /v1/messages endpoint — transparent Anthropic Messages API proxy.

use axum::{
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::Value;
use tracing::{error, warn};

use crate::schema::error::ErrorResponse;
use crate::server::AppState;

/// POST /v1/messages — proxy requests to an Anthropic-compatible provider.
///
/// Accepts native Anthropic Messages API format and forwards it transparently
/// to the configured `anthropic` or `claude-subscription` provider, injecting
/// the appropriate authentication headers. The response (streaming or not) is
/// byte-proxied back to the client without modification.
pub async fn messages(State(state): State<AppState>, Json(mut body): Json<Value>) -> Response {
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
    let provider_info = state
        .model_registry
        .iter()
        .find(|(virtual_name, _, _)| virtual_name == &model_name);

    let (provider_name, provider_model) = match provider_info {
        Some((_, pname, pmodel)) => (pname.clone(), pmodel.clone()),
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

    // Only providers that speak the Anthropic Messages API are allowed here.
    if provider_name != "anthropic" && provider_name != "claude-subscription" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::invalid_request(format!(
                "Provider '{}' does not support the Anthropic Messages API. \
                 Use /v1/chat/completions instead.",
                provider_name
            ))),
        )
            .into_response();
    }

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

    let upstream = match provider.proxy_messages(body, is_stream).await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, provider = %provider_name, "Messages proxy error");
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return (status, Json(ErrorResponse::internal(e.to_string()))).into_response();
        }
    };

    // Byte-proxy the upstream response — no parsing or rewriting.
    let upstream_status = upstream.status();
    let content_type = if is_stream {
        "text/event-stream"
    } else {
        "application/json"
    };

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

    /// A stub provider that only knows its name and exposes no real functionality.
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
        // proxy_messages uses the default (Unsupported) implementation.
    }

    fn make_app(
        providers: Vec<Box<dyn Provider>>,
        registry: Vec<(String, String, String)>,
    ) -> Router {
        let state = AppState {
            providers: Arc::new(providers),
            model_registry: Arc::new(registry),
        };
        Router::new()
            .route("/v1/messages", post(messages))
            .with_state(state)
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
    async fn test_non_messages_provider_returns_400() {
        let app = make_app(
            vec![Box::new(StubProvider {
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
            r#"{"model": "gpt-4o", "messages": [], "max_tokens": 100}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
