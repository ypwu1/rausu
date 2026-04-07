//! OpenRouter provider implementation.
//!
//! OpenRouter aggregates multiple LLM providers behind a single
//! OpenAI-compatible API. This provider forwards requests to
//! `https://openrouter.ai/api/v1` (or a custom base URL) with
//! API-key authentication and optional provider-specific headers.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use serde_json::Value;
use tracing::{debug, error};

use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ModelInfo,
};

use super::{Provider, ProviderError};

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// OpenRouter provider.
pub struct OpenRouterProvider {
    client: Client,
    api_key: String,
    base_url: String,
    /// Known models (from config).
    model_names: Vec<String>,
}

impl OpenRouterProvider {
    /// Create a new OpenRouter provider instance.
    pub fn new(api_key: String, base_url: Option<String>, model_names: Vec<String>) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build openrouter HTTP client"),
            api_key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model_names,
        }
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url);
        debug!(url = %url, model = %req.model, "Sending non-streaming request to OpenRouter");

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
            error!(status = status, body = %body, "OpenRouter error response");
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
        debug!(url = %url, model = %req.model, "Sending streaming request to OpenRouter");

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
            error!(status = status, body = %body, "OpenRouter streaming error response");
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

        // OpenRouter speaks the OpenAI Chat Completions format but does not
        // natively support the Responses API. Bridge through Chat Completions,
        // the same strategy used by the generic OpenAI provider.
        let cc_body = transform::responses_to_chat_completions_request(&body);

        let url = format!("{}/chat/completions", self.base_url);
        debug!(url = %url, "Sending Responses→CC bridged request via openrouter");

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
            error!(status = status_code, body = %msg, "openrouter CC bridge proxy error");
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
                owned_by: "openrouter".to_string(),
            })
            .collect()
    }
}
