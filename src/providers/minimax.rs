//! MiniMax provider implementation.
//!
//! MiniMax exposes both an Anthropic-compatible and an OpenAI-compatible API
//! under the same `api.minimax.io` host. This provider routes internally:
//!
//! - Downstream `chat.completions` → MiniMax OpenAI-compatible endpoint
//!   (`{base_url}/v1/chat/completions`)
//! - Downstream `messages` → MiniMax Anthropic-compatible endpoint
//!   (`{base_url}/anthropic/v1/messages`)
//! - Downstream `responses` → bridged through the OpenAI-compatible endpoint
//!   via Rausu's existing Responses↔ChatCompletions transform layer
//!
//! # Supported capabilities
//!
//! | Capability | Support |
//! |---|---|
//! | `chat_completions` | Native (OpenAI-compatible) |
//! | `streaming` | SSE streaming on both paths |
//! | `responses_api` | Bridged via Chat Completions transform |
//! | `tools` | Tool calling (both paths) |
//! | `messages_api` | Native Anthropic-compatible passthrough |
//!
//! # Unsupported inputs
//!
//! MiniMax's Anthropic-compatible endpoint does **not** support image or
//! document content blocks. Requests containing such blocks are rejected with
//! [`ProviderError::Unsupported`] before reaching the upstream, consistent
//! with Rausu's no-silent-downgrade policy.

use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use tracing::{debug, error};

use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ModelInfo,
};

use super::{Capability, Provider, ProviderError};

/// Default MiniMax API root.
const DEFAULT_API_BASE: &str = "https://api.minimax.io";

/// Anthropic API version header value required by MiniMax's compatible endpoint.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// MiniMax provider — wraps both the OpenAI-compatible and Anthropic-compatible
/// upstream protocols behind a single Rausu provider instance.
pub struct MiniMaxProvider {
    client: Client,
    api_key: String,
    /// Root URL e.g. `https://api.minimax.io`.
    api_base: String,
    /// Known model names (from config).
    model_names: Vec<String>,
}

impl MiniMaxProvider {
    /// Create a new MiniMax provider instance.
    ///
    /// `api_base` overrides the default `https://api.minimax.io`.  All other
    /// endpoint paths are derived from this root.
    pub fn new(api_key: String, api_base: Option<String>, model_names: Vec<String>) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build minimax HTTP client"),
            api_key,
            api_base: api_base.unwrap_or_else(|| DEFAULT_API_BASE.to_string()),
            model_names,
        }
    }

    /// URL for the OpenAI-compatible chat completions endpoint.
    fn openai_chat_url(&self) -> String {
        format!("{}/v1/chat/completions", self.api_base)
    }

    /// URL for the Anthropic-compatible messages endpoint.
    fn anthropic_messages_url(&self) -> String {
        format!("{}/anthropic/v1/messages", self.api_base)
    }
}

// ── Content validation ────────────────────────────────────────────────────────

/// Return `true` if any message in the Anthropic-format body contains an
/// image or document content block.
///
/// MiniMax's Anthropic-compatible endpoint does not support these block types,
/// so callers must reject such requests before hitting the upstream.
fn contains_unsupported_content_blocks(body: &Value) -> bool {
    let Some(messages) = body.get("messages").and_then(|v| v.as_array()) else {
        return false;
    };
    for msg in messages {
        let Some(content) = msg.get("content") else {
            continue;
        };
        let Some(blocks) = content.as_array() else {
            continue;
        };
        for block in blocks {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("image") | Some("document") => return true,
                _ => {}
            }
        }
    }
    false
}

// ── Provider implementation ───────────────────────────────────────────────────

#[async_trait]
impl Provider for MiniMaxProvider {
    fn name(&self) -> &str {
        "minimax"
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[
            Capability::ChatCompletions,
            Capability::Streaming,
            Capability::Responses,
            Capability::Tools,
            Capability::MessagesApi,
        ]
    }

    // ── Chat Completions (OpenAI-compatible path) ─────────────────────────────

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let url = self.openai_chat_url();
        debug!(url = %url, model = %req.model, "Sending non-streaming request to MiniMax");

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "MiniMax chat completions error");
            return Err(ProviderError::ProviderResponse {
                status,
                message: body,
            });
        }

        let completion: ChatCompletionResponse = response.json().await?;
        Ok(completion)
    }

    async fn chat_completions_stream(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        let url = self.openai_chat_url();
        debug!(url = %url, model = %req.model, "Sending streaming request to MiniMax");

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "MiniMax streaming error");
            return Err(ProviderError::ProviderResponse {
                status,
                message: body,
            });
        }

        let byte_stream = response.bytes_stream();
        let chunk_stream = byte_stream.flat_map(|result| {
            let lines: Vec<Result<ChatCompletionChunk, ProviderError>> = match result {
                Err(e) => vec![Err(ProviderError::Http(e))],
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    text.lines()
                        .filter_map(|line| {
                            let data = line.trim().strip_prefix("data: ")?;
                            if data == "[DONE]" {
                                return None;
                            }
                            Some(
                                serde_json::from_str::<ChatCompletionChunk>(data)
                                    .map_err(ProviderError::Serialisation),
                            )
                        })
                        .collect()
                }
            };
            futures::stream::iter(lines)
        });

        Ok(Box::pin(chunk_stream))
    }

    fn models(&self) -> Vec<ModelInfo> {
        let now = chrono::Utc::now().timestamp();
        self.model_names
            .iter()
            .map(|name| ModelInfo {
                id: name.clone(),
                object: "model".to_string(),
                created: now,
                owned_by: "minimax".to_string(),
            })
            .collect()
    }

    // ── Messages API (Anthropic-compatible path) ──────────────────────────────

    async fn proxy_messages(
        &self,
        body: Value,
        _is_stream: bool,
        client_betas: Option<String>,
    ) -> Result<reqwest::Response, ProviderError> {
        // MiniMax's Anthropic-compatible endpoint does not support image or
        // document content blocks.  Reject before hitting upstream.
        if contains_unsupported_content_blocks(&body) {
            return Err(ProviderError::Unsupported(
                "MiniMax Anthropic-compatible endpoint does not support image or document content blocks".to_string(),
            ));
        }

        let url = self.anthropic_messages_url();
        debug!(
            url = %url,
            model = %body.get("model").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "Forwarding Messages API request via MiniMax Anthropic-compatible endpoint"
        );

        let mut builder = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");

        if let Some(betas) = client_betas {
            builder = builder.header("anthropic-beta", betas);
        }

        let response = builder.json(&body).send().await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let msg = response.text().await.unwrap_or_default();
            error!(status = status, body = %msg, "MiniMax messages error");
            return Err(ProviderError::ProviderResponse {
                status,
                message: msg,
            });
        }

        Ok(response)
    }

    // ── Responses API (bridged via OpenAI-compatible path) ───────────────────

    async fn proxy_responses(
        &self,
        body: Value,
        is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        use crate::transform;

        // MiniMax speaks OpenAI Chat Completions but not the Responses API
        // natively.  Bridge through Chat Completions the same way OpenAI and
        // OpenRouter do.
        let cc_body = transform::responses_to_chat_completions_request(&body);

        let url = self.openai_chat_url();
        debug!(url = %url, "Sending Responses→CC bridged request via MiniMax");

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&cc_body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let msg = response.text().await.unwrap_or_default();
            error!(status = status_code, body = %msg, "MiniMax Responses CC bridge error");
            return Err(ProviderError::ProviderResponse {
                status: status_code,
                message: msg,
            });
        }

        let http_resp = if is_stream {
            let byte_stream = response.bytes_stream();
            let converted_stream =
                transform::create_responses_sse_stream_from_chat_completions(byte_stream);
            let body = reqwest::Body::wrap_stream(converted_stream);
            http::Response::builder()
                .status(200u16)
                .header("content-type", "text/event-stream; charset=utf-8")
                .body(body)
                .map_err(|e| ProviderError::Internal(e.to_string()))?
        } else {
            let cc_resp: Value = response.json().await?;
            let responses_resp = transform::chat_completions_to_responses_response(&cc_resp);
            let json_str =
                serde_json::to_string(&responses_resp).map_err(ProviderError::Serialisation)?;
            http::Response::builder()
                .status(200u16)
                .header("content-type", "application/json")
                .body(reqwest::Body::from(json_str))
                .map_err(|e| ProviderError::Internal(e.to_string()))?
        };

        Ok(reqwest::Response::from(http_resp))
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> MiniMaxProvider {
        MiniMaxProvider::new("test-key".to_string(), None, vec!["minimax-text-01".to_string()])
    }

    // ── Construction and config ───────────────────────────────────────────────

    #[test]
    fn test_provider_name() {
        assert_eq!(make_provider().name(), "minimax");
    }

    #[test]
    fn test_default_api_base() {
        let p = make_provider();
        assert_eq!(p.api_base, DEFAULT_API_BASE);
    }

    #[test]
    fn test_custom_api_base() {
        let p = MiniMaxProvider::new(
            "key".to_string(),
            Some("https://custom.example.com".to_string()),
            vec![],
        );
        assert_eq!(p.api_base, "https://custom.example.com");
    }

    #[test]
    fn test_openai_chat_url() {
        let p = make_provider();
        assert_eq!(
            p.openai_chat_url(),
            "https://api.minimax.io/v1/chat/completions"
        );
    }

    #[test]
    fn test_anthropic_messages_url() {
        let p = make_provider();
        assert_eq!(
            p.anthropic_messages_url(),
            "https://api.minimax.io/anthropic/v1/messages"
        );
    }

    #[test]
    fn test_openai_chat_url_custom_base() {
        let p = MiniMaxProvider::new(
            "key".to_string(),
            Some("https://proxy.example.com".to_string()),
            vec![],
        );
        assert_eq!(
            p.openai_chat_url(),
            "https://proxy.example.com/v1/chat/completions"
        );
    }

    // ── Capability declaration ────────────────────────────────────────────────

    #[test]
    fn test_capabilities_declared() {
        let p = make_provider();
        assert!(p.has_capability(Capability::ChatCompletions));
        assert!(p.has_capability(Capability::Streaming));
        assert!(p.has_capability(Capability::Responses));
        assert!(p.has_capability(Capability::Tools));
        assert!(p.has_capability(Capability::MessagesApi));
    }

    #[test]
    fn test_response_format_not_declared() {
        // MiniMax does not declare ResponseFormat — it is not in its capability list.
        let p = make_provider();
        assert!(!p.has_capability(Capability::ResponseFormat));
    }

    // ── models() ─────────────────────────────────────────────────────────────

    #[test]
    fn test_models_owned_by_minimax() {
        let p = MiniMaxProvider::new(
            "key".to_string(),
            None,
            vec!["minimax-text-01".to_string(), "abab6.5s-chat".to_string()],
        );
        let models = p.models();
        assert_eq!(models.len(), 2);
        for m in &models {
            assert_eq!(m.owned_by, "minimax");
            assert_eq!(m.object, "model");
        }
        assert_eq!(models[0].id, "minimax-text-01");
        assert_eq!(models[1].id, "abab6.5s-chat");
    }

    #[test]
    fn test_models_empty() {
        let p = MiniMaxProvider::new("key".to_string(), None, vec![]);
        assert!(p.models().is_empty());
    }

    // ── Content block validation ──────────────────────────────────────────────

    #[test]
    fn test_no_unsupported_blocks_plain_text() {
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": "Hello"}]
        });
        assert!(!contains_unsupported_content_blocks(&body));
    }

    #[test]
    fn test_no_unsupported_blocks_text_array() {
        let body = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "Hello"}]
            }]
        });
        assert!(!contains_unsupported_content_blocks(&body));
    }

    #[test]
    fn test_detects_image_block() {
        let body = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What is in this image?"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc"}}
                ]
            }]
        });
        assert!(contains_unsupported_content_blocks(&body));
    }

    #[test]
    fn test_detects_document_block() {
        let body = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "abc"}}
                ]
            }]
        });
        assert!(contains_unsupported_content_blocks(&body));
    }

    #[test]
    fn test_no_unsupported_blocks_tool_use() {
        // tool_use blocks are not image/document — should be allowed through
        let body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_1", "name": "fn", "input": {}}
                ]
            }]
        });
        assert!(!contains_unsupported_content_blocks(&body));
    }

    #[test]
    fn test_no_messages_field() {
        let body = serde_json::json!({"model": "minimax-text-01"});
        assert!(!contains_unsupported_content_blocks(&body));
    }

    // ── proxy_messages rejects unsupported content ────────────────────────────

    #[tokio::test]
    async fn test_proxy_messages_rejects_image_content() {
        let p = make_provider();
        let body = serde_json::json!({
            "model": "minimax-text-01",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc"}}
                ]
            }]
        });
        let result = p.proxy_messages(body, false, None).await;
        assert!(matches!(result, Err(ProviderError::Unsupported(_))));
    }

    #[tokio::test]
    async fn test_proxy_messages_rejects_document_content() {
        let p = make_provider();
        let body = serde_json::json!({
            "model": "minimax-text-01",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "abc"}}
                ]
            }]
        });
        let result = p.proxy_messages(body, false, None).await;
        assert!(matches!(result, Err(ProviderError::Unsupported(_))));
    }

    // ── SSE parsing (mirrors OpenRouter / OpenAI pattern) ────────────────────

    #[test]
    fn test_sse_done_line_is_filtered() {
        // The streaming path skips `data: [DONE]` lines.
        let text = "data: [DONE]\n";
        let chunks: Vec<_> = text
            .lines()
            .filter_map(|line| {
                let data = line.trim().strip_prefix("data: ")?;
                if data == "[DONE]" {
                    return None;
                }
                Some(serde_json::from_str::<ChatCompletionChunk>(data))
            })
            .collect();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_sse_valid_chunk_parsed() {
        let chunk_json = serde_json::json!({
            "id": "chatcmpl-abc",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "minimax-text-01",
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": "Hi"},
                "finish_reason": null
            }]
        });
        let text = format!("data: {}\n", chunk_json);
        let data = text.trim().strip_prefix("data: ").unwrap();
        let chunk: ChatCompletionChunk = serde_json::from_str(data).unwrap();
        assert_eq!(chunk.id, "chatcmpl-abc");
        assert_eq!(chunk.model, "minimax-text-01");
    }

    // ── Unsupported error is retryable (lets router skip to next provider) ───

    #[test]
    fn test_unsupported_error_is_retryable() {
        let e = ProviderError::Unsupported("image not supported".to_string());
        assert!(e.is_retryable());
        assert_eq!(e.status_code(), 405);
    }
}
