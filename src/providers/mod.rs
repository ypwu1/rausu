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

pub mod anthropic;
pub mod openai;

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
    pub fn status_code(&self) -> u16 {
        match self {
            ProviderError::ProviderResponse { status, .. } => *status,
            ProviderError::Http(_) => 502,
            ProviderError::Serialisation(_) => 500,
            ProviderError::Unsupported(_) => 405,
            ProviderError::Internal(_) => 500,
        }
    }
}

/// Core provider trait.
///
/// All providers must implement this trait to be usable by the gateway.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Returns the provider name (e.g. "openai", "anthropic").
    fn name(&self) -> &str;

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
}
