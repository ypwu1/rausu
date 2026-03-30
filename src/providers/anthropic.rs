//! Anthropic provider implementation.
//!
//! Translates between OpenAI format and Anthropic Messages API.

// Deserialization structs mirror the Anthropic API wire format exactly.
// Fields are read by serde even when not directly accessed in Rust code.
#![allow(dead_code)]

use std::pin::Pin;

use async_trait::async_trait;
use chrono::Utc;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error};
use uuid::Uuid;

use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Choice, ChunkChoice, Delta,
    Message, ModelInfo, Usage,
};

use super::{Provider, ProviderError};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Anthropic provider.
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    /// Known models (from config).
    model_names: Vec<String>,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider instance.
    pub fn new(api_key: String, model_names: Vec<String>) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build anthropic HTTP client"),
            api_key,
            model_names,
        }
    }
}

// ── Anthropic API types ──────────────────────────────────────────────────────

/// Anthropic API request body.
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
}

/// A message in the Anthropic API.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: Value,
}

/// An Anthropic tool definition.
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicTool {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: Value,
}

/// Anthropic API non-streaming response.
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<AnthropicContent>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

/// A content block in an Anthropic response.
#[derive(Debug, Deserialize)]
struct AnthropicContent {
    r#type: String,
    #[serde(default)]
    text: String,
}

/// Token usage from Anthropic.
#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ── Streaming event types ────────────────────────────────────────────────────

/// Anthropic SSE event.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicEvent {
    MessageStart {
        message: AnthropicMessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: AnthropicContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: AnthropicMessageDeltaData,
        usage: Option<AnthropicStreamUsage>,
    },
    MessageStop,
    Ping,
    Error {
        error: AnthropicErrorBody,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    id: String,
    model: String,
    usage: AnthropicStreamUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    r#type: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDeltaData {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorBody {
    r#type: String,
    message: String,
}

// ── Translation helpers ──────────────────────────────────────────────────────

/// Translate an OpenAI request to an Anthropic request.
fn translate_request(req: &ChatCompletionRequest) -> AnthropicRequest {
    let mut system: Option<String> = None;
    let mut messages: Vec<AnthropicMessage> = Vec::new();

    for msg in &req.messages {
        if msg.role == "system" {
            // Extract text content from system messages
            system = match &msg.content {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Array(parts)) => {
                    let text = parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                }
                _ => None,
            };
        } else {
            let content = match &msg.content {
                Some(v) => v.clone(),
                None => Value::String(String::new()),
            };
            messages.push(AnthropicMessage {
                role: msg.role.clone(),
                content,
            });
        }
    }

    // Convert stop sequences
    let stop_sequences = match &req.stop {
        Some(Value::String(s)) => Some(vec![s.clone()]),
        Some(Value::Array(arr)) => Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
        ),
        _ => None,
    };

    // Convert tools
    let tools = req.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.function.name.clone(),
                description: t.function.description.clone(),
                input_schema: t
                    .function
                    .parameters
                    .clone()
                    .unwrap_or(Value::Object(serde_json::Map::new())),
            })
            .collect()
    });

    AnthropicRequest {
        model: req.model.clone(),
        messages,
        max_tokens: req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        system,
        temperature: req.temperature,
        stream: req.stream,
        stop_sequences,
        tools,
    }
}

/// Map an Anthropic stop reason to an OpenAI finish reason.
fn map_stop_reason(stop_reason: Option<&str>) -> Option<String> {
    stop_reason.map(|r| match r {
        "end_turn" => "stop".to_string(),
        "max_tokens" => "length".to_string(),
        "tool_use" => "tool_calls".to_string(),
        other => other.to_string(),
    })
}

/// Generate a new chat completion ID.
fn new_completion_id() -> String {
    format!("chatcmpl-{}", Uuid::new_v4().simple())
}

// ── Provider implementation ──────────────────────────────────────────────────

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let anthropic_req = translate_request(&req);
        debug!(model = %req.model, "Sending non-streaming request to Anthropic");

        let response = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&anthropic_req)
            .send()
            .await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "Anthropic error response");
            return Err(ProviderError::ProviderResponse {
                status,
                message: body,
            });
        }

        let anthropic_resp: AnthropicResponse = response.json().await?;
        Ok(translate_response(anthropic_resp, &req.model))
    }

    async fn chat_completions_stream(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        let anthropic_req = translate_request(&req);
        debug!(model = %req.model, "Sending streaming request to Anthropic");

        let response = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&anthropic_req)
            .send()
            .await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "Anthropic streaming error response");
            return Err(ProviderError::ProviderResponse {
                status,
                message: body,
            });
        }

        let model = req.model.clone();
        let byte_stream = response.bytes_stream();

        // State shared across chunks: completion_id and model name
        let completion_id = new_completion_id();
        let created = Utc::now().timestamp();

        let stream = byte_stream.flat_map(move |result| {
            let id = completion_id.clone();
            let model_name = model.clone();
            let events: Vec<Result<ChatCompletionChunk, ProviderError>> = match result {
                Err(e) => vec![Err(ProviderError::Http(e))],
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    parse_anthropic_sse_chunk(&text, &id, &model_name, created)
                }
            };
            futures::stream::iter(events)
        });

        Ok(Box::pin(stream))
    }

    fn models(&self) -> Vec<ModelInfo> {
        let now = Utc::now().timestamp();
        self.model_names
            .iter()
            .map(|name| ModelInfo {
                id: name.clone(),
                object: "model".to_string(),
                created: now,
                owned_by: "anthropic".to_string(),
            })
            .collect()
    }

    async fn proxy_messages(
        &self,
        body: serde_json::Value,
        _is_stream: bool,
        client_betas: Option<String>,
    ) -> Result<reqwest::Response, super::ProviderError> {
        debug!(
            model = %body.get("model").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "Forwarding Messages API request via anthropic"
        );
        let mut builder = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");
        if let Some(betas) = client_betas {
            builder = builder.header("anthropic-beta", betas);
        }
        let response = builder.json(&body).send().await?;
        Ok(response)
    }
}

/// Translate an Anthropic non-streaming response to OpenAI format.
fn translate_response(resp: AnthropicResponse, _original_model: &str) -> ChatCompletionResponse {
    let text = resp
        .content
        .iter()
        .filter(|c| c.r#type == "text")
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join("");

    let finish_reason = map_stop_reason(resp.stop_reason.as_deref());

    ChatCompletionResponse {
        id: format!("chatcmpl-{}", resp.id),
        object: "chat.completion".to_string(),
        created: Utc::now().timestamp(),
        model: resp.model,
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".to_string(),
                content: Some(Value::String(text)),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            },
            finish_reason,
        }],
        usage: Usage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
        },
    }
}

/// Parse one or more Anthropic SSE events from raw text, returning OpenAI chunks.
fn parse_anthropic_sse_chunk(
    text: &str,
    id: &str,
    model: &str,
    created: i64,
) -> Vec<Result<ChatCompletionChunk, ProviderError>> {
    let mut results = Vec::new();
    let mut event_type: Option<String> = None;
    let mut data_line: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if let Some(stripped) = line.strip_prefix("event: ") {
            event_type = Some(stripped.to_string());
        } else if let Some(stripped) = line.strip_prefix("data: ") {
            data_line = Some(stripped.to_string());
        } else if line.is_empty() {
            // End of event
            if let (Some(_etype), Some(data)) = (event_type.take(), data_line.take()) {
                match serde_json::from_str::<AnthropicEvent>(&data) {
                    Ok(event) => {
                        if let Some(chunk) =
                            anthropic_event_to_openai_chunk(event, id, model, created)
                        {
                            results.push(Ok(chunk));
                        }
                    }
                    Err(e) => {
                        results.push(Err(ProviderError::Serialisation(e)));
                    }
                }
            } else {
                event_type = None;
                data_line = None;
            }
        }
    }

    // Handle case where data arrives without a trailing blank line
    if let Some(data) = data_line {
        match serde_json::from_str::<AnthropicEvent>(&data) {
            Ok(event) => {
                if let Some(chunk) = anthropic_event_to_openai_chunk(event, id, model, created) {
                    results.push(Ok(chunk));
                }
            }
            Err(e) => {
                results.push(Err(ProviderError::Serialisation(e)));
            }
        }
    }

    results
}

/// Convert a single Anthropic event to an OpenAI streaming chunk (if applicable).
fn anthropic_event_to_openai_chunk(
    event: AnthropicEvent,
    id: &str,
    model: &str,
    created: i64,
) -> Option<ChatCompletionChunk> {
    match event {
        AnthropicEvent::MessageStart { message } => {
            // Emit an initial chunk with the role
            Some(ChatCompletionChunk {
                id: id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: message.model,
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: Some("assistant".to_string()),
                        content: None,
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            })
        }
        AnthropicEvent::ContentBlockDelta { delta, .. } => match delta {
            AnthropicDelta::TextDelta { text } => Some(ChatCompletionChunk {
                id: id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some(text),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            }),
            AnthropicDelta::InputJsonDelta { .. } => None,
        },
        AnthropicEvent::MessageDelta { delta, .. } => {
            // Final chunk with finish_reason
            let finish_reason = map_stop_reason(delta.stop_reason.as_deref());
            Some(ChatCompletionChunk {
                id: id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta::default(),
                    finish_reason,
                }],
            })
        }
        // Ignore other events
        _ => None,
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::chat::Message;

    fn make_request(messages: Vec<Message>, model: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: model.to_string(),
            messages,
            temperature: None,
            max_tokens: None,
            stream: None,
            top_p: None,
            n: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
            tools: None,
            tool_choice: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
        }
    }

    #[test]
    fn test_translate_request_separates_system() {
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: Some(Value::String("You are helpful.".to_string())),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            },
            Message {
                role: "user".to_string(),
                content: Some(Value::String("Hello".to_string())),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            },
        ];
        let req = make_request(messages, "claude-3-5-sonnet-20241022");
        let anthropic_req = translate_request(&req);
        assert_eq!(anthropic_req.system, Some("You are helpful.".to_string()));
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user");
    }

    #[test]
    fn test_translate_request_default_max_tokens() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Hi".to_string())),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }];
        let req = make_request(messages, "claude-3-5-sonnet-20241022");
        let anthropic_req = translate_request(&req);
        assert_eq!(anthropic_req.max_tokens, DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn test_translate_request_custom_max_tokens() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Hi".to_string())),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }];
        let mut req = make_request(messages, "claude-3-5-sonnet-20241022");
        req.max_tokens = Some(1024);
        let anthropic_req = translate_request(&req);
        assert_eq!(anthropic_req.max_tokens, 1024);
    }

    #[test]
    fn test_map_stop_reason() {
        assert_eq!(map_stop_reason(Some("end_turn")), Some("stop".to_string()));
        assert_eq!(
            map_stop_reason(Some("max_tokens")),
            Some("length".to_string())
        );
        assert_eq!(
            map_stop_reason(Some("tool_use")),
            Some("tool_calls".to_string())
        );
        assert_eq!(map_stop_reason(Some("other")), Some("other".to_string()));
        assert_eq!(map_stop_reason(None), None);
    }

    #[test]
    fn test_translate_response() {
        let resp = AnthropicResponse {
            id: "msg_123".to_string(),
            model: "claude-3-5-sonnet-20241022".to_string(),
            content: vec![AnthropicContent {
                r#type: "text".to_string(),
                text: "Hello, world!".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };
        let openai_resp = translate_response(resp, "claude-3-5-sonnet-20241022");
        assert!(openai_resp.id.starts_with("chatcmpl-"));
        assert_eq!(openai_resp.object, "chat.completion");
        assert_eq!(
            openai_resp.choices[0].finish_reason,
            Some("stop".to_string())
        );
        if let Some(Value::String(text)) = &openai_resp.choices[0].message.content {
            assert_eq!(text, "Hello, world!");
        } else {
            panic!("Expected string content");
        }
        assert_eq!(openai_resp.usage.prompt_tokens, 10);
        assert_eq!(openai_resp.usage.completion_tokens, 5);
        assert_eq!(openai_resp.usage.total_tokens, 15);
    }

    #[test]
    fn test_translate_stop_sequences_string() {
        let mut req = make_request(
            vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("hi".to_string())),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            }],
            "claude-3-5-sonnet-20241022",
        );
        req.stop = Some(Value::String("STOP".to_string()));
        let anthropic_req = translate_request(&req);
        assert_eq!(anthropic_req.stop_sequences, Some(vec!["STOP".to_string()]));
    }

    #[test]
    fn test_translate_stop_sequences_array() {
        let mut req = make_request(
            vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("hi".to_string())),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            }],
            "claude-3-5-sonnet-20241022",
        );
        req.stop = Some(Value::Array(vec![
            Value::String("A".to_string()),
            Value::String("B".to_string()),
        ]));
        let anthropic_req = translate_request(&req);
        assert_eq!(
            anthropic_req.stop_sequences,
            Some(vec!["A".to_string(), "B".to_string()])
        );
    }
}
