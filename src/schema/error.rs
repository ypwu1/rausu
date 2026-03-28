//! OpenAI-compatible error response types.

use serde::{Deserialize, Serialize};

/// OpenAI-compatible error detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// Human-readable error message.
    pub message: String,
    /// Error type string (e.g. "invalid_request_error").
    pub r#type: String,
    /// Optional error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Optional parameter that caused the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

/// OpenAI-compatible error response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Error details.
    pub error: ErrorDetail,
}

impl ErrorResponse {
    /// Create a new error response.
    pub fn new(message: impl Into<String>, error_type: impl Into<String>) -> Self {
        Self {
            error: ErrorDetail {
                message: message.into(),
                r#type: error_type.into(),
                code: None,
                param: None,
            },
        }
    }

    /// Create an internal server error response.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(message, "internal_server_error")
    }

    /// Create an invalid request error response.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(message, "invalid_request_error")
    }
}
