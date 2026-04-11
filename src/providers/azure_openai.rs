//! Azure OpenAI provider implementation.
//!
//! Azure OpenAI uses a different URL structure and authentication header from
//! standard OpenAI:
//!
//! - **Auth header:** `api-key: <key>` (not `Authorization: Bearer <key>`)
//! - **URL pattern:** `{base_url}/openai/deployments/{deployment}/chat/completions?api-version={version}`
//! - **`model` in config** is the Azure deployment name, used in the URL path
//!   rather than sent in the request body.
//!
//! The Responses API is bridged through Chat Completions using Rausu's existing
//! transform layer, the same strategy used by the `openai`, `deepseek`,
//! `openrouter`, `moonshot`, and `z-ai` providers.
//!
//! # Supported capabilities
//!
//! | Capability | Support |
//! |---|---|
//! | `chat_completions` | Native (OpenAI-compatible via Azure) |
//! | `streaming` | SSE streaming |
//! | `responses_api` | Bridged via Chat Completions transform |
//! | `tools` | Tool calling passthrough |
//! | `response_format` | Structured output passthrough |

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use serde_json::Value;
use tracing::{debug, error};

use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ModelInfo,
};

use super::{Capability, Provider, ProviderError};

/// Default Azure OpenAI API version when none is configured.
const DEFAULT_API_VERSION: &str = "2024-12-01-preview";

/// Azure OpenAI provider.
pub struct AzureOpenAiProvider {
    client: Client,
    api_key: String,
    /// Azure resource endpoint, e.g. `https://my-resource.openai.azure.com`
    base_url: String,
    /// Azure API version query parameter.
    api_version: String,
    /// Azure deployment name (the `model` field from config).
    deployment_name: String,
    /// Virtual model names (from config).
    model_names: Vec<String>,
}

impl AzureOpenAiProvider {
    /// Create a new Azure OpenAI provider instance.
    ///
    /// `base_url` is required and must point to the Azure resource endpoint.
    /// `deployment_name` is the Azure deployment name (from config `model` field).
    /// `api_version` defaults to [`DEFAULT_API_VERSION`] when `None`.
    pub fn new(
        api_key: String,
        base_url: String,
        deployment_name: String,
        api_version: Option<String>,
        model_names: Vec<String>,
    ) -> Result<Self, ProviderError> {
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()?,
            api_key,
            base_url,
            api_version: api_version.unwrap_or_else(|| DEFAULT_API_VERSION.to_string()),
            deployment_name,
            model_names,
        })
    }

    /// Build the chat completions URL for this Azure deployment.
    fn chat_completions_url(&self) -> String {
        format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            self.base_url, self.deployment_name, self.api_version
        )
    }

    /// Strip the `model` field from a request body before sending to Azure.
    ///
    /// Azure OpenAI determines the model from the deployment name in the URL,
    /// not from the request body. Including it can cause errors on some API versions.
    fn strip_model_field(mut body: Value) -> Value {
        if let Some(obj) = body.as_object_mut() {
            obj.remove("model");
        }
        body
    }
}

#[async_trait]
impl Provider for AzureOpenAiProvider {
    fn name(&self) -> &str {
        "azure-openai"
    }

    fn capabilities(&self) -> &'static [Capability] {
        use Capability::*;
        &[ChatCompletions, Streaming, Responses, Tools, ResponseFormat]
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let url = self.chat_completions_url();
        debug!(url = %url, model = %req.model, "Sending non-streaming request to Azure OpenAI");

        // Serialise, strip model field, then send.
        let body = serde_json::to_value(&req).map_err(ProviderError::Serialisation)?;
        let body = Self::strip_model_field(body);

        let response = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "Azure OpenAI error response");
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
        let url = self.chat_completions_url();
        debug!(url = %url, model = %req.model, "Sending streaming request to Azure OpenAI");

        let body = serde_json::to_value(&req).map_err(ProviderError::Serialisation)?;
        let body = Self::strip_model_field(body);

        let response = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "Azure OpenAI streaming error response");
            return Err(ProviderError::ProviderResponse {
                status,
                message: body,
            });
        }

        let byte_stream = response.bytes_stream();
        Ok(super::parse_sse_stream(byte_stream))
    }

    async fn proxy_responses(
        &self,
        body: Value,
        is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        use crate::transform;

        // Azure OpenAI does not natively support the Responses API. Bridge
        // through Chat Completions, the same strategy used by the openai,
        // deepseek, openrouter, moonshot, and z-ai providers.
        let cc_body = transform::responses_to_chat_completions_request(&body);
        let cc_body = Self::strip_model_field(cc_body);

        let url = self.chat_completions_url();
        debug!(url = %url, "Sending Responses→CC bridged request via azure-openai");

        let response = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&cc_body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let msg = response.text().await.unwrap_or_default();
            error!(status = status_code, body = %msg, "Azure OpenAI Responses CC bridge error");
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
                owned_by: "azure-openai".to_string(),
            })
            .collect()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> AzureOpenAiProvider {
        AzureOpenAiProvider::new(
            "test-key".to_string(),
            "https://my-resource.openai.azure.com".to_string(),
            "gpt-4o-deployment".to_string(),
            None,
            vec!["gpt-4o".to_string()],
        )
        .unwrap()
    }

    // ── Construction and config ───────────────────────────────────────────────

    #[test]
    fn test_provider_name() {
        assert_eq!(make_provider().name(), "azure-openai");
    }

    #[test]
    fn test_default_api_version() {
        let p = make_provider();
        assert_eq!(p.api_version, DEFAULT_API_VERSION);
    }

    #[test]
    fn test_custom_api_version() {
        let p = AzureOpenAiProvider::new(
            "key".to_string(),
            "https://res.openai.azure.com".to_string(),
            "dep".to_string(),
            Some("2025-01-01-preview".to_string()),
            vec![],
        )
        .unwrap();
        assert_eq!(p.api_version, "2025-01-01-preview");
    }

    #[test]
    fn test_base_url_trailing_slash_stripped() {
        let p = AzureOpenAiProvider::new(
            "key".to_string(),
            "https://my-resource.openai.azure.com/".to_string(),
            "dep".to_string(),
            None,
            vec![],
        )
        .unwrap();
        assert_eq!(p.base_url, "https://my-resource.openai.azure.com");
    }

    // ── URL construction ─────────────────────────────────────────────────────

    #[test]
    fn test_chat_completions_url() {
        let p = make_provider();
        let url = p.chat_completions_url();
        assert_eq!(
            url,
            format!(
                "https://my-resource.openai.azure.com/openai/deployments/gpt-4o-deployment/chat/completions?api-version={}",
                DEFAULT_API_VERSION
            )
        );
    }

    #[test]
    fn test_chat_completions_url_custom_version() {
        let p = AzureOpenAiProvider::new(
            "key".to_string(),
            "https://res.openai.azure.com".to_string(),
            "my-deploy".to_string(),
            Some("2025-01-01-preview".to_string()),
            vec![],
        )
        .unwrap();
        assert_eq!(
            p.chat_completions_url(),
            "https://res.openai.azure.com/openai/deployments/my-deploy/chat/completions?api-version=2025-01-01-preview"
        );
    }

    // ── Model field stripping ────────────────────────────────────────────────

    #[test]
    fn test_strip_model_field() {
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}]
        });
        let stripped = AzureOpenAiProvider::strip_model_field(body);
        assert!(stripped.get("model").is_none());
        assert!(stripped.get("messages").is_some());
    }

    #[test]
    fn test_strip_model_field_no_model() {
        let body = serde_json::json!({"messages": []});
        let stripped = AzureOpenAiProvider::strip_model_field(body);
        assert!(stripped.get("model").is_none());
        assert!(stripped.get("messages").is_some());
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
    fn test_models_owned_by_azure_openai() {
        let p = AzureOpenAiProvider::new(
            "key".to_string(),
            "https://res.openai.azure.com".to_string(),
            "dep".to_string(),
            None,
            vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()],
        )
        .unwrap();
        let models = p.models();
        assert_eq!(models.len(), 2);
        for m in &models {
            assert_eq!(m.owned_by, "azure-openai");
            assert_eq!(m.object, "model");
        }
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(models[1].id, "gpt-4o-mini");
    }

    #[test]
    fn test_models_empty() {
        let p = AzureOpenAiProvider::new(
            "key".to_string(),
            "https://res.openai.azure.com".to_string(),
            "dep".to_string(),
            None,
            vec![],
        )
        .unwrap();
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
            "id": "chatcmpl-azure",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": "Hi"},
                "finish_reason": null
            }]
        });
        let text = format!("data: {}\n", chunk_json);
        let data = text.trim().strip_prefix("data: ").unwrap();
        let chunk: ChatCompletionChunk = serde_json::from_str(data).unwrap();
        assert_eq!(chunk.id, "chatcmpl-azure");
        assert_eq!(chunk.model, "gpt-4o");
    }

    // ── Unsupported error retryability ───────────────────────────────────────

    #[test]
    fn test_unsupported_error_is_retryable() {
        let e = ProviderError::Unsupported("not supported".to_string());
        assert!(e.is_retryable());
        assert_eq!(e.status_code(), 405);
    }
}
