//! Provider trait and implementations.
//!
//! Each provider translates between the unified OpenAI schema and its own API format.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use thiserror::Error;

use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ModelInfo,
};

/// Capabilities a provider can declare.
///
/// Used by the router to pre-filter providers before attempting upstream calls,
/// ensuring unsupported requests fail fast with clear errors instead of relying
/// solely on runtime error-driven failover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Non-streaming chat completions.
    ChatCompletions,
    /// Streaming chat completions (SSE).
    Streaming,
    /// Responses API handling (native or bridged through chat completions).
    Responses,
    /// Tool calling passthrough (tools + tool_choice).
    Tools,
    /// Structured output via response_format.
    ResponseFormat,
    /// Anthropic Messages API.
    MessagesApi,
}

impl Capability {
    /// Human-readable name for error messages.
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::ChatCompletions => "chat_completions",
            Capability::Streaming => "streaming",
            Capability::Responses => "responses_api",
            Capability::Tools => "tools",
            Capability::ResponseFormat => "response_format",
            Capability::MessagesApi => "messages_api",
        }
    }
}

pub mod anthropic;
pub mod chatgpt_subscription;
pub mod claude_subscription;
pub mod github_copilot;
pub mod minimax;
pub mod openai;
pub mod openrouter;
pub mod vertex_ai;

/// Error type for provider operations.
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ProviderError {
    /// HTTP transport error.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// Provider returned a non-success HTTP status.
    #[error("Provider error {status}: {message}")]
    ProviderResponse {
        /// HTTP status code.
        status: u16,
        /// Error message from provider.
        message: String,
    },
    /// JSON serialisation/deserialisation error.
    #[error("Serialisation error: {0}")]
    Serialisation(#[from] serde_json::Error),
    /// Request is not supported by this provider.
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl ProviderError {
    /// Map the error to an appropriate HTTP status code.
    ///
    /// `Http` errors are mapped to 504 on timeout (upstream did not respond in time)
    /// and 502 for all other transport failures.
    pub fn status_code(&self) -> u16 {
        match self {
            ProviderError::ProviderResponse { status, .. } => *status,
            ProviderError::Http(e) => {
                if e.is_timeout() {
                    504
                } else {
                    502
                }
            }
            ProviderError::Serialisation(_) => 500,
            ProviderError::Unsupported(_) => 405,
            ProviderError::Internal(_) => 500,
        }
    }

    /// Whether this error is retryable and the request should be attempted
    /// on the next provider in priority order.
    ///
    /// Transport failures (`Http`) are always retryable. For upstream
    /// responses, only server-side and rate-limit statuses are retried.
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::Http(_) => true,
            ProviderError::ProviderResponse { status, .. } => is_retryable_status(*status),
            ProviderError::Unsupported(_) => true, // skip to next provider that supports the API
            _ => false,
        }
    }
}

/// Returns `true` for HTTP status codes that warrant failover to the next provider.
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Core provider trait.
///
/// All providers must implement this trait to be usable by the gateway.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Returns the provider name (e.g. "openai", "anthropic").
    fn name(&self) -> &str;

    /// Declare the capabilities this provider supports.
    ///
    /// The router uses this to pre-filter providers before attempting upstream
    /// calls. The default declares basic chat completions and streaming only;
    /// providers should override to advertise additional capabilities.
    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::ChatCompletions, Capability::Streaming]
    }

    /// Check whether this provider supports a specific capability.
    fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities().contains(&cap)
    }

    /// Perform a non-streaming chat completion.
    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError>;

    /// Perform a streaming chat completion, returning an SSE chunk stream.
    async fn chat_completions_stream(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    >;

    /// List the models available from this provider.
    fn models(&self) -> Vec<ModelInfo>;

    /// Forward a raw Anthropic Messages API request and return the upstream response.
    ///
    /// Only `anthropic` and `claude-subscription` providers override this.
    /// All others return [`ProviderError::Unsupported`] by default.
    ///
    /// `client_betas` is the raw value of the `anthropic-beta` header sent by the
    /// downstream client (e.g. Claude Code). Providers that set their own beta headers
    /// should merge this value with their required betas before forwarding.
    async fn proxy_messages(
        &self,
        _body: serde_json::Value,
        _is_stream: bool,
        _client_betas: Option<String>,
    ) -> Result<reqwest::Response, ProviderError> {
        Err(ProviderError::Unsupported(format!(
            "Provider '{}' does not support the Anthropic Messages API",
            self.name()
        )))
    }

    /// Forward a raw OpenAI Responses API request and return the upstream response.
    ///
    /// Providers that speak the Responses API (e.g. `openai`, `chatgpt-subscription`)
    /// override this method. All others return [`ProviderError::Unsupported`] by default,
    /// which the route translates to a 405 response.
    async fn proxy_responses(
        &self,
        _body: serde_json::Value,
        _is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        Err(ProviderError::Unsupported(format!(
            "Provider '{}' does not support the Responses API",
            self.name()
        )))
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_response_preserves_upstream_status() {
        let e = ProviderError::ProviderResponse {
            status: 429,
            message: "rate limited".to_string(),
        };
        assert_eq!(e.status_code(), 429);
    }

    #[test]
    fn test_serialisation_error_is_500() {
        let json_err = serde_json::from_str::<i32>("not-a-number").unwrap_err();
        assert_eq!(ProviderError::Serialisation(json_err).status_code(), 500);
    }

    #[test]
    fn test_unsupported_error_is_405() {
        assert_eq!(
            ProviderError::Unsupported("not supported".to_string()).status_code(),
            405
        );
    }

    #[test]
    fn test_internal_error_is_500() {
        assert_eq!(
            ProviderError::Internal("something broke".to_string()).status_code(),
            500
        );
    }

    #[test]
    fn test_is_retryable_status() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(504));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(404));
        assert!(!is_retryable_status(200));
    }

    #[test]
    fn test_provider_response_retryable() {
        let retryable = ProviderError::ProviderResponse {
            status: 429,
            message: "rate limited".to_string(),
        };
        assert!(retryable.is_retryable());

        let not_retryable = ProviderError::ProviderResponse {
            status: 400,
            message: "bad request".to_string(),
        };
        assert!(!not_retryable.is_retryable());
    }

    #[test]
    fn test_unsupported_is_retryable() {
        assert!(ProviderError::Unsupported("not supported".to_string()).is_retryable());
    }

    #[test]
    fn test_internal_error_is_not_retryable() {
        assert!(!ProviderError::Internal("broken".to_string()).is_retryable());
    }
}
