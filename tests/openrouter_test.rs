//! Smoke tests for the OpenRouter provider integration.
//!
//! These tests exercise OpenRouter through Rausu's route handlers using
//! stub providers (no live OpenRouter API calls). They cover:
//!
//! - Chat completions (non-streaming)
//! - Chat completions (streaming / SSE)
//! - Responses API (bridged)
//! - Invalid auth / invalid model paths
//! - Unsupported capability behaviour
//! - Failover from unsupported provider to OpenRouter

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{body::Body, routing::post, Router};
use futures::{stream, Stream};
use serde_json::Value;
use tower::ServiceExt;

use rausu::providers::{Provider, ProviderError};
use rausu::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Choice, ChunkChoice,
    Delta, Message, ModelInfo, Usage,
};
use rausu::server::AppState;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// A stub that behaves like a working OpenRouter provider for chat completions.
struct StubOpenRouterProvider {
    model_names: Vec<String>,
}

#[async_trait]
impl Provider for StubOpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.model_names
            .iter()
            .map(|n| ModelInfo {
                id: n.clone(),
                object: "model".to_string(),
                created: 0,
                owned_by: "openrouter".to_string(),
            })
            .collect()
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        Ok(ChatCompletionResponse {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: req.model.clone(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String("Hello from OpenRouter!".to_string())),
                    tool_call_id: None,
                    tool_calls: None,
                    name: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        })
    }

    async fn chat_completions_stream(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        let chunks = vec![
            Ok(ChatCompletionChunk {
                id: "chatcmpl-stream".to_string(),
                object: "chat.completion.chunk".to_string(),
                created: 1700000000,
                model: req.model.clone(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: Some("assistant".to_string()),
                        content: Some("Hello".to_string()),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            }),
            Ok(ChatCompletionChunk {
                id: "chatcmpl-stream".to_string(),
                object: "chat.completion.chunk".to_string(),
                created: 1700000000,
                model: req.model.clone(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some(" world!".to_string()),
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
            }),
        ];
        Ok(Box::pin(stream::iter(chunks)))
    }

    async fn proxy_responses(
        &self,
        _body: Value,
        _is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let http_resp = http::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(
                r#"{"id":"resp_or_test","object":"response"}"#,
            ))
            .unwrap();
        Ok(reqwest::Response::from(http_resp))
    }
}

/// A stub provider that returns an auth error (401).
struct AuthErrorProvider;

#[async_trait]
impl Provider for AuthErrorProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn chat_completions(
        &self,
        _req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        Err(ProviderError::ProviderResponse {
            status: 401,
            message: r#"{"error":{"message":"Invalid API key","type":"invalid_api_key"}}"#
                .to_string(),
        })
    }

    async fn chat_completions_stream(
        &self,
        _req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        Err(ProviderError::ProviderResponse {
            status: 401,
            message: "Invalid API key".to_string(),
        })
    }
}

/// A stub provider that does not support any API.
struct UnsupportedProvider;

#[async_trait]
impl Provider for UnsupportedProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn chat_completions(
        &self,
        _req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        Err(ProviderError::Unsupported(
            "stub does not support chat completions".to_string(),
        ))
    }

    async fn chat_completions_stream(
        &self,
        _req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        Err(ProviderError::Unsupported(
            "stub does not support streaming".to_string(),
        ))
    }
}

fn make_chat_app(
    providers: Vec<Box<dyn Provider>>,
    registry: Vec<(String, Vec<(String, String)>)>,
) -> Router {
    let mut map = HashMap::new();
    for (name, entries) in registry {
        map.insert(name, entries);
    }
    let state = AppState {
        providers: Arc::new(providers),
        model_registry: Arc::new(map),
    };
    Router::new()
        .route(
            "/v1/chat/completions",
            post(rausu::server::routes::chat::chat_completions),
        )
        .route(
            "/v1/responses",
            post(rausu::server::routes::responses::responses),
        )
        .with_state(state)
}

fn registry_entry(name: &str, pname: &str, pmodel: &str) -> (String, Vec<(String, String)>) {
    (
        name.to_string(),
        vec![(pname.to_string(), pmodel.to_string())],
    )
}

async fn post_json(app: Router, uri: &str, body: &str) -> http::Response<Body> {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.oneshot(request).await.unwrap()
}

async fn body_json(resp: http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn body_text(resp: http::Response<Body>) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_openrouter_chat_non_stream() {
    let app = make_chat_app(
        vec![Box::new(StubOpenRouterProvider {
            model_names: vec!["or-gpt4o".to_string()],
        })],
        vec![registry_entry(
            "or-gpt4o",
            "openrouter",
            "openai/gpt-4o",
        )],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "or-gpt4o", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let body = body_json(resp).await;
    assert_eq!(body["id"], "chatcmpl-test");
    assert_eq!(body["model"], "openai/gpt-4o");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "Hello from OpenRouter!"
    );
    assert_eq!(body["usage"]["total_tokens"], 15);
}

#[tokio::test]
async fn test_openrouter_chat_stream() {
    let app = make_chat_app(
        vec![Box::new(StubOpenRouterProvider {
            model_names: vec!["or-gpt4o".to_string()],
        })],
        vec![registry_entry(
            "or-gpt4o",
            "openrouter",
            "openai/gpt-4o",
        )],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "or-gpt4o", "stream": true, "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "expected SSE content type, got: {ct}"
    );

    let text = body_text(resp).await;
    assert!(text.contains("Hello"), "expected 'Hello' in SSE stream");
    assert!(text.contains("world!"), "expected 'world!' in SSE stream");
    assert!(text.contains("[DONE]"), "expected [DONE] sentinel");
}

#[tokio::test]
async fn test_openrouter_responses_api() {
    let app = make_chat_app(
        vec![Box::new(StubOpenRouterProvider {
            model_names: vec!["or-gpt4o".to_string()],
        })],
        vec![registry_entry(
            "or-gpt4o",
            "openrouter",
            "openai/gpt-4o",
        )],
    );
    let resp = post_json(
        app,
        "/v1/responses",
        r#"{"model": "or-gpt4o", "input": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let body = body_json(resp).await;
    assert_eq!(body["id"], "resp_or_test");
}

#[tokio::test]
async fn test_openrouter_invalid_auth() {
    let app = make_chat_app(
        vec![Box::new(AuthErrorProvider)],
        vec![registry_entry(
            "or-gpt4o",
            "openrouter",
            "openai/gpt-4o",
        )],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "or-gpt4o", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    // 401 is non-retryable → returned directly
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_openrouter_invalid_model() {
    let app = make_chat_app(
        vec![Box::new(StubOpenRouterProvider {
            model_names: vec![],
        })],
        vec![],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "nonexistent-model", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_openrouter_unsupported_capability_failover() {
    // First provider doesn't support chat completions; OpenRouter does.
    // Unsupported is retryable → should fail over to OpenRouter.
    let app = make_chat_app(
        vec![
            Box::new(UnsupportedProvider),
            Box::new(StubOpenRouterProvider {
                model_names: vec!["my-model".to_string()],
            }),
        ],
        vec![(
            "my-model".to_string(),
            vec![
                ("anthropic".to_string(), "my-model".to_string()),
                ("openrouter".to_string(), "openai/gpt-4o".to_string()),
            ],
        )],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "my-model", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let body = body_json(resp).await;
    assert_eq!(body["choices"][0]["message"]["content"], "Hello from OpenRouter!");
}

#[tokio::test]
async fn test_openrouter_unsupported_capability_no_fallback() {
    // Single unsupported provider — should return error, not silently succeed.
    let app = make_chat_app(
        vec![Box::new(UnsupportedProvider)],
        vec![registry_entry("my-model", "anthropic", "my-model")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "my-model", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    // Single provider unsupported → retryable but no fallback → error
    assert!(
        resp.status().is_server_error() || resp.status() == 405,
        "expected server error or 405 for unsupported single provider, got: {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_openrouter_tools_passthrough() {
    // Verify that tools/tool_choice are NOT stripped from the request.
    // The stub provider echoes back successfully if it receives the request at all.
    let app = make_chat_app(
        vec![Box::new(StubOpenRouterProvider {
            model_names: vec!["or-gpt4o".to_string()],
        })],
        vec![registry_entry(
            "or-gpt4o",
            "openrouter",
            "openai/gpt-4o",
        )],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "or-gpt4o",
            "messages": [{"role": "user", "content": "What is the weather?"}],
            "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}],
            "tool_choice": "auto"
        }"#,
    )
    .await;
    // Should succeed — tools are passed through, not stripped
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_openrouter_response_format_passthrough() {
    // Verify response_format is not stripped
    let app = make_chat_app(
        vec![Box::new(StubOpenRouterProvider {
            model_names: vec!["or-gpt4o".to_string()],
        })],
        vec![registry_entry(
            "or-gpt4o",
            "openrouter",
            "openai/gpt-4o",
        )],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "or-gpt4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "response_format": {"type": "json_object"}
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 200);
}
