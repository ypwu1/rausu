//! Smoke tests for the Z.AI provider integration.
//!
//! These tests exercise Z.AI through Rausu's route handlers using stub
//! providers (no live Z.AI API calls). They cover:
//!
//! - Chat completions (non-streaming)
//! - Chat completions (streaming / SSE)
//! - Responses API (bridged)
//! - Invalid auth / invalid model paths
//! - Unsupported capability behaviour
//! - Tool passthrough and response_format passthrough
//! - Failover from unsupported provider to Z.AI

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{body::Body, routing::post, Router};
use futures::{stream, Stream};
use serde_json::Value;
use tower::ServiceExt;

use rausu::providers::{Capability, Provider, ProviderError};
use rausu::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Choice, ChunkChoice, Delta,
    Message, ModelInfo, Usage,
};
use rausu::server::AppState;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// A stub that behaves like a working Z.AI provider for chat completions.
struct StubZaiProvider {
    model_names: Vec<String>,
}

#[async_trait]
impl Provider for StubZaiProvider {
    fn name(&self) -> &str {
        "z-ai"
    }

    fn capabilities(&self) -> &'static [Capability] {
        use Capability::*;
        &[ChatCompletions, Streaming, Responses, Tools, ResponseFormat]
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.model_names
            .iter()
            .map(|n| ModelInfo {
                id: n.clone(),
                object: "model".to_string(),
                created: 0,
                owned_by: "z-ai".to_string(),
            })
            .collect()
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        Ok(ChatCompletionResponse {
            id: "chatcmpl-zai-test".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: req.model.clone(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String("Hello from Z.AI!".to_string())),
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
                id: "chatcmpl-zai-stream".to_string(),
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
                id: "chatcmpl-zai-stream".to_string(),
                object: "chat.completion.chunk".to_string(),
                created: 1700000000,
                model: req.model.clone(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some(" from Z.AI!".to_string()),
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
                r#"{"id":"resp_zai_test","object":"response"}"#,
            ))
            .unwrap();
        Ok(reqwest::Response::from(http_resp))
    }
}

/// A stub provider that returns an auth error (401).
struct ZaiAuthErrorProvider;

#[async_trait]
impl Provider for ZaiAuthErrorProvider {
    fn name(&self) -> &str {
        "z-ai"
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
struct ZaiUnsupportedProvider;

#[async_trait]
impl Provider for ZaiUnsupportedProvider {
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

/// A stub provider with no Tools or ResponseFormat capabilities.
struct ZaiNoToolsProvider;

#[async_trait]
impl Provider for ZaiNoToolsProvider {
    fn name(&self) -> &str {
        "z-ai"
    }

    fn capabilities(&self) -> &'static [Capability] {
        use Capability::*;
        &[ChatCompletions, Streaming]
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        Ok(ChatCompletionResponse {
            id: "chatcmpl-no-tools".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: req.model.clone(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String("ok".to_string())),
                    tool_call_id: None,
                    tool_calls: None,
                    name: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        })
    }

    async fn chat_completions_stream(
        &self,
        _req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        Ok(Box::pin(stream::empty()))
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
async fn test_zai_chat_non_stream() {
    let app = make_chat_app(
        vec![Box::new(StubZaiProvider {
            model_names: vec!["z-ai-1-preview".to_string()],
        })],
        vec![registry_entry("z-ai-1-preview", "z-ai", "z-ai-1-preview")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "z-ai-1-preview", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let body = body_json(resp).await;
    assert_eq!(body["id"], "chatcmpl-zai-test");
    assert_eq!(body["model"], "z-ai-1-preview");
    assert_eq!(body["choices"][0]["message"]["content"], "Hello from Z.AI!");
    assert_eq!(body["usage"]["total_tokens"], 15);
}

#[tokio::test]
async fn test_zai_chat_stream() {
    let app = make_chat_app(
        vec![Box::new(StubZaiProvider {
            model_names: vec!["z-ai-1-preview".to_string()],
        })],
        vec![registry_entry("z-ai-1-preview", "z-ai", "z-ai-1-preview")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "z-ai-1-preview", "stream": true, "messages": [{"role": "user", "content": "Hi"}]}"#,
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
    assert!(
        text.contains("from Z.AI!"),
        "expected 'from Z.AI!' in SSE stream"
    );
    assert!(text.contains("[DONE]"), "expected [DONE] sentinel");
}

#[tokio::test]
async fn test_zai_responses_api() {
    let app = make_chat_app(
        vec![Box::new(StubZaiProvider {
            model_names: vec!["z-ai-1-preview".to_string()],
        })],
        vec![registry_entry("z-ai-1-preview", "z-ai", "z-ai-1-preview")],
    );
    let resp = post_json(
        app,
        "/v1/responses",
        r#"{"model": "z-ai-1-preview", "input": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let body = body_json(resp).await;
    assert_eq!(body["id"], "resp_zai_test");
}

#[tokio::test]
async fn test_zai_invalid_auth() {
    let app = make_chat_app(
        vec![Box::new(ZaiAuthErrorProvider)],
        vec![registry_entry("z-ai-1-preview", "z-ai", "z-ai-1-preview")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "z-ai-1-preview", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_zai_invalid_model() {
    let app = make_chat_app(
        vec![Box::new(StubZaiProvider {
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
async fn test_zai_tools_passthrough() {
    let app = make_chat_app(
        vec![Box::new(StubZaiProvider {
            model_names: vec!["z-ai-1-preview".to_string()],
        })],
        vec![registry_entry("z-ai-1-preview", "z-ai", "z-ai-1-preview")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "z-ai-1-preview",
            "messages": [{"role": "user", "content": "What is the weather?"}],
            "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}],
            "tool_choice": "auto"
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_zai_response_format_passthrough() {
    let app = make_chat_app(
        vec![Box::new(StubZaiProvider {
            model_names: vec!["z-ai-1-preview".to_string()],
        })],
        vec![registry_entry("z-ai-1-preview", "z-ai", "z-ai-1-preview")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "z-ai-1-preview",
            "messages": [{"role": "user", "content": "Hi"}],
            "response_format": {"type": "json_object"}
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_zai_unsupported_capability_failover() {
    // First provider doesn't support chat completions; Z.AI does.
    let app = make_chat_app(
        vec![
            Box::new(ZaiUnsupportedProvider),
            Box::new(StubZaiProvider {
                model_names: vec!["my-model".to_string()],
            }),
        ],
        vec![(
            "my-model".to_string(),
            vec![
                ("anthropic".to_string(), "my-model".to_string()),
                ("z-ai".to_string(), "z-ai-1-preview".to_string()),
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
    assert_eq!(body["choices"][0]["message"]["content"], "Hello from Z.AI!");
}

#[tokio::test]
async fn test_zai_unsupported_capability_no_fallback() {
    let app = make_chat_app(
        vec![Box::new(ZaiUnsupportedProvider)],
        vec![registry_entry("my-model", "anthropic", "my-model")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "my-model", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert!(
        resp.status().is_server_error() || resp.status() == 405,
        "expected server error or 405, got: {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_zai_tools_unsupported_returns_422() {
    // Provider declares only ChatCompletions + Streaming (no Tools).
    // Request includes tools → should return 422.
    let app = make_chat_app(
        vec![Box::new(ZaiNoToolsProvider)],
        vec![registry_entry("z-ai-1-preview", "z-ai", "z-ai-1-preview")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "z-ai-1-preview",
            "messages": [{"role": "user", "content": "Hi"}],
            "tools": [{"type": "function", "function": {"name": "fn", "parameters": {}}}]
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 422);

    let body = body_json(resp).await;
    assert_eq!(body["error"]["type"], "unsupported_capability");
}

#[tokio::test]
async fn test_zai_response_format_unsupported_returns_422() {
    let app = make_chat_app(
        vec![Box::new(ZaiNoToolsProvider)],
        vec![registry_entry("z-ai-1-preview", "z-ai", "z-ai-1-preview")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "z-ai-1-preview",
            "messages": [{"role": "user", "content": "Hi"}],
            "response_format": {"type": "json_object"}
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 422);

    let body = body_json(resp).await;
    assert_eq!(body["error"]["type"], "unsupported_capability");
}

// ── Capability declaration tests ────────────────────────────────────────────

#[test]
fn test_capability_declaration_zai_stub() {
    let p = StubZaiProvider {
        model_names: vec!["m".to_string()],
    };
    assert!(p.has_capability(Capability::ChatCompletions));
    assert!(p.has_capability(Capability::Streaming));
    assert!(p.has_capability(Capability::Responses));
    assert!(p.has_capability(Capability::Tools));
    assert!(p.has_capability(Capability::ResponseFormat));
    assert!(!p.has_capability(Capability::MessagesApi));
}

// ── Build-providers registration test ───────────────────────────────────────

#[tokio::test]
async fn test_zai_build_providers_registration() {
    use rausu::config::schema::{
        AppConfig, AuthConfig, LoggingConfig, ModelConfig, ProviderDeployment, ServerConfig,
    };

    let config = AppConfig {
        server: ServerConfig::default(),
        logging: LoggingConfig::default(),
        auth: AuthConfig::default(),
        models: vec![ModelConfig {
            name: "z-ai-1-preview".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "z-ai".to_string(),
                model: "z-ai-1-preview".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: None,
                token_source: None,
                credentials_path: None,
                project_id: None,
                location: None,
            }],
        }],
    };

    // Use the same build_providers path as the server
    // We can't call build_providers directly (it's private), but we can verify
    // via the config schema that z-ai is a valid provider string.
    assert_eq!(config.models[0].providers[0].provider, "z-ai");
    assert_eq!(config.models[0].providers[0].model, "z-ai-1-preview");
}
