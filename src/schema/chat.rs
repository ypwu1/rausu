//! OpenAI-compatible chat completion schema types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role: system, user, assistant, or tool.
    pub role: String,
    /// Message content (text or structured content parts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    /// Tool call ID (for tool role messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool calls made by the assistant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Optional name field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A tool call made by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool call ID.
    pub id: String,
    /// Type: always "function".
    pub r#type: String,
    /// Function call details.
    pub function: FunctionCall,
}

/// Function call details within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Function name.
    pub name: String,
    /// JSON-encoded function arguments.
    pub arguments: String,
}

/// A tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    /// Type: always "function".
    pub r#type: String,
    /// Function definition.
    pub function: FunctionDefinition,
}

/// Function definition for a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Function name.
    pub name: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// OpenAI-compatible chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    /// Model name (virtual or provider-specific).
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Sampling temperature (0.0–2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Maximum tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Whether to stream the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// Top-p nucleus sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Number of completions to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// Stop sequences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Value>,
    /// Presence penalty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// Frequency penalty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// User identifier for abuse detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Tools available to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    /// Tool choice setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    /// Response format constraint (e.g. `{"type": "json_object"}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    /// Seed for deterministic sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Log probabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,
    /// Top log probabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u32>,
}

/// Token usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Tokens in the completion.
    pub completion_tokens: u32,
    /// Total tokens.
    pub total_tokens: u32,
}

/// A completion choice in a non-streaming response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// Zero-based index.
    pub index: u32,
    /// The generated message.
    pub message: Message,
    /// Reason generation stopped.
    pub finish_reason: Option<String>,
}

/// OpenAI-compatible chat completion response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// Unique completion ID.
    pub id: String,
    /// Object type: always "chat.completion".
    /// Some providers (e.g. GitHub Copilot) omit this field.
    #[serde(default = "default_chat_completion_object")]
    pub object: String,
    /// Unix timestamp of creation.
    /// Some providers (e.g. GitHub Copilot) omit this field.
    #[serde(default)]
    pub created: i64,
    /// Model that generated the response.
    pub model: String,
    /// Completion choices.
    pub choices: Vec<Choice>,
    /// Token usage.
    pub usage: Usage,
}

/// Delta content in a streaming chunk choice.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Delta {
    /// Role (only in the first chunk).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Incremental text content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls delta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<Value>>,
}

/// A choice within a streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    /// Zero-based index.
    pub index: u32,
    /// Incremental content delta.
    pub delta: Delta,
    /// Non-null only on the final chunk.
    pub finish_reason: Option<String>,
}

/// OpenAI-compatible streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    /// Unique completion ID (same across all chunks).
    pub id: String,
    /// Object type: always "chat.completion.chunk".
    /// Some providers (e.g. GitHub Copilot) omit this field.
    #[serde(default = "default_chat_completion_chunk_object")]
    pub object: String,
    /// Unix timestamp of creation.
    /// Some providers (e.g. GitHub Copilot) omit this field.
    #[serde(default)]
    pub created: i64,
    /// Model that generated the chunk.
    pub model: String,
    /// Streaming choices.
    pub choices: Vec<ChunkChoice>,
}

/// A model entry in the /v1/models response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model ID.
    pub id: String,
    /// Object type: always "model".
    pub object: String,
    /// Unix timestamp when model was created.
    pub created: i64,
    /// Organization that owns the model.
    pub owned_by: String,
}

/// Response for /v1/models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResponse {
    /// Object type: always "list".
    pub object: String,
    /// Available models.
    pub data: Vec<ModelInfo>,
}

// ── Default helpers for lenient deserialization ────────────────────────

fn default_chat_completion_object() -> String {
    "chat.completion".to_string()
}

fn default_chat_completion_chunk_object() -> String {
    "chat.completion.chunk".to_string()
}
