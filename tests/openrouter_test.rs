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

use rausu::providers::{Capability, Provider, ProviderError};
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
        .route(
            "/v1/messages",
            post(rausu::server::routes::messages::messages),
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

// ── Phase 1 Hardening: Capability-based routing tests ───────────────────────

/// A stub provider with configurable capabilities for testing capability-based
/// routing. Succeeds for chat completions and optionally responses.
struct CapabilityStubProvider {
    provider_name: &'static str,
    caps: &'static [Capability],
    /// If true, records received request fields for verification.
    received_tools: std::sync::Arc<std::sync::Mutex<Option<Value>>>,
    received_response_format: std::sync::Arc<std::sync::Mutex<Option<Value>>>,
}

impl CapabilityStubProvider {
    fn new(provider_name: &'static str, caps: &'static [Capability]) -> Self {
        Self {
            provider_name,
            caps,
            received_tools: std::sync::Arc::new(std::sync::Mutex::new(None)),
            received_response_format: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

#[async_trait]
impl Provider for CapabilityStubProvider {
    fn name(&self) -> &str {
        self.provider_name
    }

    fn capabilities(&self) -> &'static [Capability] {
        self.caps
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        // Record what we received for verification
        if let Some(tools) = &req.tools {
            *self.received_tools.lock().unwrap() =
                Some(serde_json::to_value(tools).unwrap());
        }
        if let Some(fmt) = &req.response_format {
            *self.received_response_format.lock().unwrap() = Some(fmt.clone());
        }

        Ok(ChatCompletionResponse {
            id: "chatcmpl-cap-test".to_string(),
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

    async fn proxy_responses(
        &self,
        _body: Value,
        _is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let http_resp = http::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(r#"{"id":"resp_cap_test"}"#))
            .unwrap();
        Ok(reqwest::Response::from(http_resp))
    }

    async fn proxy_messages(
        &self,
        _body: Value,
        _is_stream: bool,
        _client_betas: Option<String>,
    ) -> Result<reqwest::Response, ProviderError> {
        let http_resp = http::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(
                r#"{"type":"message","content":[]}"#,
            ))
            .unwrap();
        Ok(reqwest::Response::from(http_resp))
    }
}

// ── Capability declaration tests ────────────────────────────────────────────

#[test]
fn test_capability_declaration_openrouter_stub() {
    let p = StubOpenRouterProvider {
        model_names: vec!["m".to_string()],
    };
    assert!(p.has_capability(Capability::ChatCompletions));
    assert!(p.has_capability(Capability::Streaming));
    assert!(p.has_capability(Capability::Responses));
    assert!(p.has_capability(Capability::Tools));
    assert!(p.has_capability(Capability::ResponseFormat));
    assert!(!p.has_capability(Capability::MessagesApi));
}

#[test]
fn test_capability_declaration_default() {
    // UnsupportedProvider uses default capabilities (ChatCompletions + Streaming only)
    let p = UnsupportedProvider;
    assert!(p.has_capability(Capability::ChatCompletions));
    assert!(p.has_capability(Capability::Streaming));
    assert!(!p.has_capability(Capability::Tools));
    assert!(!p.has_capability(Capability::ResponseFormat));
    assert!(!p.has_capability(Capability::Responses));
    assert!(!p.has_capability(Capability::MessagesApi));
}

// ── Capability-based prefilter: unsupported capability error ────────────────

#[tokio::test]
async fn test_capability_prefilter_tools_unsupported_returns_422() {
    // Provider does not declare Tools capability; request includes tools.
    // Should return 422 unsupported_capability, not attempt the call.
    use Capability::*;
    let app = make_chat_app(
        vec![Box::new(CapabilityStubProvider::new(
            "no-tools-provider",
            &[ChatCompletions, Streaming], // no Tools
        ))],
        vec![registry_entry("m", "no-tools-provider", "m")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "Hi"}],
            "tools": [{"type": "function", "function": {"name": "f", "parameters": {}}}]
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 422, "expected 422 for unsupported capability");

    let body = body_json(resp).await;
    assert_eq!(body["error"]["type"], "unsupported_capability");
    assert_eq!(body["error"]["code"], "unsupported_capability");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tools"),
        "error message should name the missing capability"
    );
}

#[tokio::test]
async fn test_capability_prefilter_response_format_unsupported_returns_422() {
    use Capability::*;
    let app = make_chat_app(
        vec![Box::new(CapabilityStubProvider::new(
            "no-fmt-provider",
            &[ChatCompletions, Streaming], // no ResponseFormat
        ))],
        vec![registry_entry("m", "no-fmt-provider", "m")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "Hi"}],
            "response_format": {"type": "json_object"}
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 422);

    let body = body_json(resp).await;
    assert_eq!(body["error"]["type"], "unsupported_capability");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("response_format"),
    );
}

#[tokio::test]
async fn test_capability_prefilter_both_tools_and_format_unsupported() {
    use Capability::*;
    let app = make_chat_app(
        vec![Box::new(CapabilityStubProvider::new(
            "basic-provider",
            &[ChatCompletions, Streaming],
        ))],
        vec![registry_entry("m", "basic-provider", "m")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "Hi"}],
            "tools": [{"type": "function", "function": {"name": "f", "parameters": {}}}],
            "response_format": {"type": "json_object"}
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 422);

    let body = body_json(resp).await;
    assert_eq!(body["error"]["type"], "unsupported_capability");
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("tools"), "should mention tools");
    assert!(msg.contains("response_format"), "should mention response_format");
}

// ── Capability-based failover ──────────────────────────────────────────────

#[tokio::test]
async fn test_capability_failover_tools_first_lacks_second_has() {
    // First provider lacks Tools, second declares it. Request with tools
    // should skip the first and succeed on the second.
    use Capability::*;
    let app = make_chat_app(
        vec![
            Box::new(CapabilityStubProvider::new(
                "no-tools",
                &[ChatCompletions, Streaming],
            )),
            Box::new(CapabilityStubProvider::new(
                "has-tools",
                &[ChatCompletions, Streaming, Tools, ResponseFormat],
            )),
        ],
        vec![(
            "m".to_string(),
            vec![
                ("no-tools".to_string(), "m".to_string()),
                ("has-tools".to_string(), "m".to_string()),
            ],
        )],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "Hi"}],
            "tools": [{"type": "function", "function": {"name": "f", "parameters": {}}}]
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 200, "should failover to capable provider");
}

#[tokio::test]
async fn test_capability_failover_response_format() {
    use Capability::*;
    let app = make_chat_app(
        vec![
            Box::new(CapabilityStubProvider::new(
                "no-fmt",
                &[ChatCompletions, Streaming, Tools],
            )),
            Box::new(CapabilityStubProvider::new(
                "has-fmt",
                &[ChatCompletions, Streaming, Tools, ResponseFormat],
            )),
        ],
        vec![(
            "m".to_string(),
            vec![
                ("no-fmt".to_string(), "m".to_string()),
                ("has-fmt".to_string(), "m".to_string()),
            ],
        )],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "Hi"}],
            "response_format": {"type": "json_object"}
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 200);
}

// ── No silent stripping of capability-bearing fields ───────────────────────

#[tokio::test]
async fn test_no_silent_tools_stripping() {
    // Verify that the tools field is preserved in the request delivered to the provider.
    use Capability::*;
    let provider = CapabilityStubProvider::new(
        "openrouter",
        &[ChatCompletions, Streaming, Responses, Tools, ResponseFormat],
    );
    let tools_capture = provider.received_tools.clone();

    let app = make_chat_app(
        vec![Box::new(provider)],
        vec![registry_entry("m", "openrouter", "m")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "Hi"}],
            "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}]
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let captured = tools_capture.lock().unwrap();
    assert!(captured.is_some(), "tools should have been passed to provider");
    let tools_val = captured.as_ref().unwrap();
    assert!(tools_val.is_array());
    assert_eq!(tools_val.as_array().unwrap().len(), 1);
    assert_eq!(tools_val[0]["function"]["name"], "get_weather");
}

#[tokio::test]
async fn test_no_silent_response_format_stripping() {
    use Capability::*;
    let provider = CapabilityStubProvider::new(
        "openrouter",
        &[ChatCompletions, Streaming, Responses, Tools, ResponseFormat],
    );
    let fmt_capture = provider.received_response_format.clone();

    let app = make_chat_app(
        vec![Box::new(provider)],
        vec![registry_entry("m", "openrouter", "m")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "Hi"}],
            "response_format": {"type": "json_object"}
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let captured = fmt_capture.lock().unwrap();
    assert!(
        captured.is_some(),
        "response_format should have been passed to provider"
    );
    assert_eq!(captured.as_ref().unwrap()["type"], "json_object");
}

// ── Responses route capability checks ──────────────────────────────────────

#[tokio::test]
async fn test_responses_capability_prefilter_returns_422() {
    // Provider without Responses capability → should return 422, not 405
    use Capability::*;
    let app = make_chat_app(
        vec![Box::new(CapabilityStubProvider::new(
            "no-responses",
            &[ChatCompletions, Streaming, MessagesApi],
        ))],
        vec![registry_entry("m", "no-responses", "m")],
    );
    let resp = post_json(
        app,
        "/v1/responses",
        r#"{"model": "m", "input": "Hello"}"#,
    )
    .await;
    assert_eq!(resp.status(), 422);

    let body = body_json(resp).await;
    assert_eq!(body["error"]["type"], "unsupported_capability");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("responses_api"),
    );
}

#[tokio::test]
async fn test_responses_capability_failover_to_capable() {
    // First provider lacks Responses, second has it → should succeed.
    use Capability::*;
    let app = make_chat_app(
        vec![
            Box::new(CapabilityStubProvider::new(
                "no-responses",
                &[ChatCompletions, Streaming, MessagesApi],
            )),
            Box::new(CapabilityStubProvider::new(
                "has-responses",
                &[ChatCompletions, Streaming, Responses],
            )),
        ],
        vec![(
            "m".to_string(),
            vec![
                ("no-responses".to_string(), "m".to_string()),
                ("has-responses".to_string(), "m".to_string()),
            ],
        )],
    );
    let resp = post_json(
        app,
        "/v1/responses",
        r#"{"model": "m", "input": "Hello"}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);
}

// ── Messages route capability checks ───────────────────────────────────────

#[tokio::test]
async fn test_messages_capability_prefilter_returns_422() {
    // Provider without MessagesApi → should return 422
    use Capability::*;
    let app = make_chat_app(
        vec![Box::new(CapabilityStubProvider::new(
            "openrouter",
            &[ChatCompletions, Streaming, Responses, Tools, ResponseFormat],
        ))],
        vec![registry_entry("m", "openrouter", "m")],
    );
    let resp = post_json(
        app,
        "/v1/messages",
        r#"{"model": "m", "messages": [], "max_tokens": 100}"#,
    )
    .await;
    assert_eq!(resp.status(), 422);

    let body = body_json(resp).await;
    assert_eq!(body["error"]["type"], "unsupported_capability");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("messages_api"),
    );
}

#[tokio::test]
async fn test_messages_capability_failover_to_capable() {
    use Capability::*;
    let app = make_chat_app(
        vec![
            Box::new(CapabilityStubProvider::new(
                "openrouter",
                &[ChatCompletions, Streaming, Responses],
            )),
            Box::new(CapabilityStubProvider::new(
                "anthropic",
                &[ChatCompletions, Streaming, MessagesApi, Tools],
            )),
        ],
        vec![(
            "m".to_string(),
            vec![
                ("openrouter".to_string(), "m".to_string()),
                ("anthropic".to_string(), "m".to_string()),
            ],
        )],
    );
    let resp = post_json(
        app,
        "/v1/messages",
        r#"{"model": "m", "messages": [], "max_tokens": 100}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);
}

// ── Unsupported capability error contract ──────────────────────────────────

#[tokio::test]
async fn test_unsupported_capability_error_contract() {
    // Verify the error response shape matches the documented contract.
    use Capability::*;
    let app = make_chat_app(
        vec![Box::new(CapabilityStubProvider::new(
            "basic",
            &[ChatCompletions, Streaming],
        ))],
        vec![registry_entry("m", "basic", "m")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "Hi"}],
            "tools": [{"type": "function", "function": {"name": "f", "parameters": {}}}]
        }"#,
    )
    .await;
    assert_eq!(resp.status(), 422);

    let body = body_json(resp).await;
    // Verify all error contract fields
    assert!(body["error"].is_object(), "error should be an object");
    assert_eq!(body["error"]["type"], "unsupported_capability");
    assert_eq!(body["error"]["code"], "unsupported_capability");
    assert!(
        body["error"]["message"].is_string(),
        "message should be a string"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tools"),
        "message should name the missing capability"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("m"),
        "message should name the model"
    );
}

// ── Basic request without capability-bearing fields still works ─────────────

#[tokio::test]
async fn test_basic_request_without_tools_succeeds_on_any_provider() {
    // A request without tools/response_format should succeed even on a
    // provider that only declares basic capabilities.
    use Capability::*;
    let app = make_chat_app(
        vec![Box::new(CapabilityStubProvider::new(
            "basic",
            &[ChatCompletions, Streaming],
        ))],
        vec![registry_entry("m", "basic", "m")],
    );
    let resp = post_json(
        app,
        "/v1/chat/completions",
        r#"{"model": "m", "messages": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 200, "basic request should succeed");
}

// ── Responses bridge via OpenRouter with capability check ──────────────────

#[tokio::test]
async fn test_openrouter_responses_bridge_with_capability_check() {
    // OpenRouter declares Responses capability → should handle /v1/responses
    let app = make_chat_app(
        vec![Box::new(StubOpenRouterProvider {
            model_names: vec!["or-model".to_string()],
        })],
        vec![registry_entry("or-model", "openrouter", "openai/gpt-4o")],
    );
    let resp = post_json(
        app,
        "/v1/responses",
        r#"{"model": "or-model", "input": [{"role": "user", "content": "Hi"}]}"#,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["id"], "resp_or_test");
}
