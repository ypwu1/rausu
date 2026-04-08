//! DeepSeek provider implementation.
//!
//! DeepSeek exposes an OpenAI-compatible API at `https://api.deepseek.com`.
//! This provider forwards requests using standard Bearer-token authentication
//! and supports chat completions (streaming + non-streaming), tool calling,
//! and structured output via `response_format`.
//!
//! **Base URL note:** DeepSeek's official documentation lists
//! `https://api.deepseek.com` as the primary base URL. The `/v1` prefix
//! (`https://api.deepseek.com/v1`) is also accepted for OpenAI client-library
//! compatibility but is **not** a model-version signal.
//!
//! The Responses API is bridged through Chat Completions using Rausu's existing
//! transform layer, the same strategy used by the `openrouter`, `openai`,
//! `moonshot`, and `z-ai` providers.
//!
//! # Supported capabilities
//!
//! | Capability | Support |
//! |---|---|
//! | `chat_completions` | Native (OpenAI-compatible) |
//! | `streaming` | SSE streaming |
//! | `responses_api` | Bridged via Chat Completions transform |
//! | `tools` | Tool calling passthrough |
//! | `response_format` | Structured output passthrough |

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

/// Default DeepSeek API base URL.
///
/// DeepSeek also accepts `https://api.deepseek.com/v1` for OpenAI client-library
/// compatibility, but the `/v1` path is not a model-version signal. We use the
/// canonical base URL here and append `/chat/completions` at call time.
const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";

/// DeepSeek provider.
pub struct DeepSeekProvider {
    client: Client,
    api_key: String,
    base_url: String,
    /// Known models (from config).
    model_names: Vec<String>,
}

impl DeepSeekProvider {
    /// Create a new DeepSeek provider instance.
    pub fn new(api_key: String, base_url: Option<String>, model_names: Vec<String>) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build deepseek HTTP client"),
            api_key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model_names,
        }
    }
}

#[async_trait]
impl Provider for DeepSeekProvider {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn capabilities(&self) -> &'static [Capability] {
        use Capability::*;
        &[ChatCompletions, Streaming, Responses, Tools, ResponseFormat]
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url);
        debug!(url = %url, model = %req.model, "Sending non-streaming request to DeepSeek");

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
            error!(status = status, body = %body, "DeepSeek error response");
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
        let url = format!("{}/chat/completions", self.base_url);
        debug!(url = %url, model = %req.model, "Sending streaming request to DeepSeek");

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
            error!(status = status, body = %body, "DeepSeek streaming error response");
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

    async fn proxy_responses(
        &self,
        body: Value,
        is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        use crate::transform;

        // DeepSeek speaks the OpenAI Chat Completions format but does not natively
        // support the Responses API. Bridge through Chat Completions, the same
        // strategy used by the openrouter, openai, moonshot, and z-ai providers.
        let cc_body = transform::responses_to_chat_completions_request(&body);

        let url = format!("{}/chat/completions", self.base_url);
        debug!(url = %url, "Sending Responses→CC bridged request via deepseek");

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
            error!(status = status_code, body = %msg, "DeepSeek Responses CC bridge error");
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

    fn models(&self) -> Vec<ModelInfo> {
        let now = chrono::Utc::now().timestamp();
        self.model_names
            .iter()
            .map(|name| ModelInfo {
                id: name.clone(),
                object: "model".to_string(),
                created: now,
                owned_by: "deepseek".to_string(),
            })
            .collect()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> DeepSeekProvider {
        DeepSeekProvider::new(
            "test-key".to_string(),
            None,
            vec!["deepseek-chat".to_string()],
        )
    }

    // ── Construction and config ───────────────────────────────────────────────

    #[test]
    fn test_provider_name() {
        assert_eq!(make_provider().name(), "deepseek");
    }

    #[test]
    fn test_default_base_url() {
        let p = make_provider();
        assert_eq!(p.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn test_custom_base_url() {
        let p = DeepSeekProvider::new(
            "key".to_string(),
            Some("https://custom.example.com".to_string()),
            vec![],
        );
        assert_eq!(p.base_url, "https://custom.example.com");
    }

    #[test]
    fn test_chat_completions_url() {
        let p = make_provider();
        let url = format!("{}/chat/completions", p.base_url);
        assert_eq!(url, "https://api.deepseek.com/chat/completions");
    }

    // ── Capability declaration ────────────────────────────────────────────────

    #[test]
    fn test_capabilities_declared() {
        let p = make_provider();
        assert!(p.has_capability(Capability::ChatCompletions));
        assert!(p.has_capability(Capability::Streaming));
        assert!(p.has_capability(Capability::Responses));
        assert!(p.has_capability(Capability::Tools));
        assert!(p.has_capability(Capability::ResponseFormat));
    }

    #[test]
    fn test_messages_api_not_declared() {
        let p = make_provider();
        assert!(!p.has_capability(Capability::MessagesApi));
    }

    // ── models() ─────────────────────────────────────────────────────────────

    #[test]
    fn test_models_owned_by_deepseek() {
        let p = DeepSeekProvider::new(
            "key".to_string(),
            None,
            vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
        );
        let models = p.models();
        assert_eq!(models.len(), 2);
        for m in &models {
            assert_eq!(m.owned_by, "deepseek");
            assert_eq!(m.object, "model");
        }
        assert_eq!(models[0].id, "deepseek-chat");
        assert_eq!(models[1].id, "deepseek-reasoner");
    }

    #[test]
    fn test_models_empty() {
        let p = DeepSeekProvider::new("key".to_string(), None, vec![]);
        assert!(p.models().is_empty());
    }

    // ── SSE parsing ─────────────────────────────────────────────────────────

    #[test]
    fn test_sse_done_line_is_filtered() {
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
            "id": "chatcmpl-deepseek",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": "Hi"},
                "finish_reason": null
            }]
        });
        let text = format!("data: {}\n", chunk_json);
        let data = text.trim().strip_prefix("data: ").unwrap();
        let chunk: ChatCompletionChunk = serde_json::from_str(data).unwrap();
        assert_eq!(chunk.id, "chatcmpl-deepseek");
        assert_eq!(chunk.model, "deepseek-chat");
    }

    // ── Unsupported error retryability ───────────────────────────────────────

    #[test]
    fn test_unsupported_error_is_retryable() {
        let e = ProviderError::Unsupported("not supported".to_string());
        assert!(e.is_retryable());
        assert_eq!(e.status_code(), 405);
    }
}
