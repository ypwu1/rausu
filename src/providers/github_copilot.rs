//! GitHub Copilot provider implementation.
//!
//! GitHub Copilot exposes an OpenAI-compatible chat completions endpoint at
//! `https://api.githubcopilot.com/chat/completions`.  Authentication uses a
//! two-step exchange: a GitHub OAuth token is exchanged for a short-lived
//! Copilot API bearer token via [`CopilotTokenManager`].
//!
//! # Supported endpoints
//!
//! | Route | Support |
//! |-------|---------|
//! | `POST /v1/chat/completions` | ✅ full (streaming + non-streaming) |
//! | `GET /v1/models` | ✅ lists configured model names |
//! | `POST /v1/messages` | ❌ Copilot does not implement Anthropic Messages API |
//! | `POST /v1/responses` | ❌ Copilot does not implement OpenAI Responses API |
//!
//! # Model names
//!
//! Upstream Copilot model identifiers change over time.  Recommended values as
//! of 2025-Q1:
//!
//! - `gpt-4o` — default chat model
//! - `claude-3.5-sonnet` — Anthropic via Copilot
//! - `o1-mini` — reasoning model

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use tracing::{debug, error};

use crate::auth::copilot::CopilotTokenManager;
use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ModelInfo,
};

use super::{Provider, ProviderError};

/// User-Agent header value sent with all Copilot API requests.
const USER_AGENT: &str = "rausu/0.1 (github-copilot-provider)";

/// Editor-Version header — using a generic value to avoid Copilot version checks.
const EDITOR_VERSION: &str = "vscode/1.95.3";

/// Editor-Plugin-Version header.
const EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.22.4";

/// Copilot-Integration-Id header.
const COPILOT_INTEGRATION_ID: &str = "vscode-chat";

// ── Provider ──────────────────────────────────────────────────────────────────

/// GitHub Copilot chat completions provider.
pub struct GitHubCopilotProvider {
    token_manager: Arc<CopilotTokenManager>,
    /// Virtual model names served by this provider instance.
    model_names: Vec<String>,
}

impl GitHubCopilotProvider {
    /// Create a new provider instance.
    pub fn new(token_manager: Arc<CopilotTokenManager>, model_names: Vec<String>) -> Self {
        Self {
            token_manager,
            model_names,
        }
    }

    /// Build a pre-authenticated Reqwest `RequestBuilder` for the completions endpoint.
    async fn completions_request(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let (api_token, endpoint) = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(format!("Copilot auth failed: {e}")))?;

        let url = format!("{}/chat/completions", endpoint);
        debug!(url = %url, model = %req.model, "Sending request to GitHub Copilot");

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ProviderError::Internal(format!("Failed to build HTTP client: {e}")))?;

        let builder = client
            .post(&url)
            .bearer_auth(&api_token)
            .header("User-Agent", USER_AGENT)
            .header("Editor-Version", EDITOR_VERSION)
            .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
            .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
            .json(req);

        Ok(builder)
    }
}

#[async_trait]
impl Provider for GitHubCopilotProvider {
    fn name(&self) -> &str {
        "github-copilot"
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let builder = self.completions_request(&req).await?;
        let response = builder.send().await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, "GitHub Copilot error response");
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
        let builder = self.completions_request(&req).await?;
        let response = builder.send().await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, "GitHub Copilot streaming error response");
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
                owned_by: "github-copilot".to_string(),
            })
            .collect()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::copilot::CopilotTokenManager;

    fn make_provider(model_names: Vec<&str>) -> GitHubCopilotProvider {
        let mgr = CopilotTokenManager::new(None);
        GitHubCopilotProvider::new(mgr, model_names.into_iter().map(String::from).collect())
    }

    #[test]
    fn test_provider_name() {
        let p = make_provider(vec!["gpt-4o"]);
        assert_eq!(p.name(), "github-copilot");
    }

    #[test]
    fn test_models_list() {
        let p = make_provider(vec!["gpt-4o", "claude-3.5-sonnet"]);
        let models = p.models();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(models[0].owned_by, "github-copilot");
        assert_eq!(models[1].id, "claude-3.5-sonnet");
    }

    #[test]
    fn test_empty_model_list() {
        let p = make_provider(vec![]);
        assert!(p.models().is_empty());
    }
}
