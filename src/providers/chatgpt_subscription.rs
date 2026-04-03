//! ChatGPT Subscription provider implementation.
//!
//! Bridges OpenAI-compatible Chat Completions requests to ChatGPT's Responses API
//! at `https://chatgpt.com/backend-api/codex/responses`, then converts the
//! streamed Responses API events back to Chat Completions format.
//!
//! # Authentication
//!
//! Uses [`ChatGptOAuthTokenManager`] to supply a Bearer access token and the
//! extracted `chatgpt-account-id` header.
//!
//! Required headers per request:
//! - `Authorization: Bearer <access_token>`
//! - `chatgpt-account-id: <account_id>`
//! - `OpenAI-Beta: responses=experimental`
//! - `originator: pi`
//! - `User-Agent: pi (<os> <release>; <arch>)`
//! - `Content-Type: application/json`
//!
//! # Request Conversion
//!
//! Chat Completions → Responses API:
//! - `messages[role=system]` → `instructions`
//! - `messages[role=user/assistant]` → `input` array
//! - `tools` → Responses tool format
//! - `temperature`, `tool_choice` → passed through
//! - `max_tokens`, `max_output_tokens`, `max_completion_tokens`, `metadata` → stripped
//! - Always added: `stream: true`, `store: false`, `text: {verbosity: "medium"}`,
//!   `include: ["reasoning.encrypted_content"]`, `tool_choice: "auto"`,
//!   `parallel_tool_calls: true`
//!
//! # Response Conversion
//!
//! Responses API events → Chat Completions:
//! - `response.output_text.delta` → `choices[0].delta.content`
//! - `response.function_call_arguments.delta` → tool call deltas
//! - `response.completed` / `response.done` → `finish_reason: "stop"` + usage
//! - `response.failed` / `error` → propagated as errors
//!
//! # Verified Models (as of 2026-03-30)
//!
//! - `gpt-5.4`
//! - `gpt-5.4-pro`
//! - `gpt-5.3-codex`
//! - `gpt-5.3-codex-spark`
//! - `gpt-5.3-instant`
//! - `gpt-5.3-chat-latest`

#![allow(dead_code)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error};
use uuid::Uuid;

use crate::auth::chatgpt_oauth::ChatGptOAuthTokenManager;
use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Choice, ChunkChoice, Delta,
    Message, ModelInfo, Usage,
};

use super::{Provider, ProviderError};

const RESPONSES_API_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

// Build the User-Agent string once at startup.
fn build_user_agent() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!("pi ({os} unknown; {arch})")
}

// ── Provider struct ───────────────────────────────────────────────────────────

/// ChatGPT Subscription provider.
///
/// Bridges Chat Completions API requests through a ChatGPT Plus/Pro/Max account.
pub struct ChatGptSubscriptionProvider {
    client: Client,
    token_manager: Arc<ChatGptOAuthTokenManager>,
    model_names: Vec<String>,
    user_agent: String,
}

impl ChatGptSubscriptionProvider {
    /// Create a new provider instance.
    pub fn new(token_manager: Arc<ChatGptOAuthTokenManager>, model_names: Vec<String>) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build chatgpt-subscription HTTP client"),
            token_manager,
            model_names,
            user_agent: build_user_agent(),
        }
    }
}

// ── Responses API request types ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ResponsesInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesTool>>,
    tool_choice: String,
    parallel_tool_calls: bool,
    stream: bool,
    store: bool,
    text: ResponsesText,
    include: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ResponsesInputItem {
    role: String,
    content: Value,
}

#[derive(Debug, Serialize)]
struct ResponsesTool {
    r#type: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct ResponsesText {
    verbosity: String,
}

// ── Responses API event types ─────────────────────────────────────────────────

/// SSE events emitted by the ChatGPT Responses API.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesEvent {
    /// Streaming text delta.
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        delta: String,
        #[serde(default)]
        item_id: Option<String>,
        #[serde(default)]
        output_index: Option<u32>,
        #[serde(default)]
        content_index: Option<u32>,
    },
    /// Text block complete.
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        #[serde(default)]
        text: Option<String>,
    },
    /// Tool call arguments delta.
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        delta: String,
        #[serde(default)]
        item_id: Option<String>,
        #[serde(default)]
        output_index: Option<u32>,
        #[serde(default)]
        call_id: Option<String>,
    },
    /// Tool call arguments complete.
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        #[serde(default)]
        arguments: Option<String>,
        #[serde(default)]
        call_id: Option<String>,
    },
    /// Response completed — includes usage.
    #[serde(rename = "response.completed")]
    Completed { response: ResponsesCompletedPayload },
    /// Response done (alias for completed without usage).
    #[serde(rename = "response.done")]
    Done {
        #[serde(default)]
        response: Option<ResponsesCompletedPayload>,
    },
    /// Response failed.
    #[serde(rename = "response.failed")]
    Failed {
        #[serde(default)]
        error: Option<ResponsesErrorBody>,
    },
    /// Top-level error event.
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        message: Option<String>,
    },
}

#[derive(Debug, Deserialize, Default)]
struct ResponsesCompletedPayload {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<ResponsesUsage>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ResponsesErrorBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

// ── Translation helpers ───────────────────────────────────────────────────────

/// Convert a Chat Completions request to a Responses API request body.
///
/// Strips unsupported fields: `max_tokens`, `max_output_tokens`,
/// `max_completion_tokens`, `metadata`.
fn translate_request(req: &ChatCompletionRequest) -> ResponsesRequest {
    let mut instructions: Option<String> = None;
    let mut input: Vec<ResponsesInputItem> = Vec::new();

    for msg in &req.messages {
        match msg.role.as_str() {
            "system" => {
                instructions = match &msg.content {
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
            }
            role => {
                let content = match &msg.content {
                    Some(v) => v.clone(),
                    None => Value::String(String::new()),
                };
                input.push(ResponsesInputItem {
                    role: role.to_string(),
                    content,
                });
            }
        }
    }

    let tools = req.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|t| ResponsesTool {
                r#type: "function".to_string(),
                name: t.function.name.clone(),
                description: t.function.description.clone(),
                parameters: t
                    .function
                    .parameters
                    .clone()
                    .unwrap_or(Value::Object(serde_json::Map::new())),
            })
            .collect()
    });

    // tool_choice: respect caller's choice if it is a plain string, else default to "auto"
    let tool_choice = match &req.tool_choice {
        Some(Value::String(s)) => s.clone(),
        _ => "auto".to_string(),
    };

    // ChatGPT Codex Responses API requires `instructions` to be non-null.
    // Default to a minimal system prompt when the caller doesn't provide one.
    let instructions = instructions.or_else(|| Some("You are a helpful assistant.".to_string()));

    ResponsesRequest {
        model: req.model.clone(),
        input,
        instructions,
        temperature: req.temperature,
        tools,
        tool_choice,
        parallel_tool_calls: true,
        stream: true,
        store: false,
        text: ResponsesText {
            verbosity: "medium".to_string(),
        },
        include: vec!["reasoning.encrypted_content".to_string()],
    }
}

fn new_completion_id() -> String {
    format!("chatcmpl-{}", Uuid::new_v4().simple())
}

/// Parse a block of SSE text into zero or more `ChatCompletionChunk` results.
fn parse_responses_sse_chunk(
    text: &str,
    id: &str,
    model: &str,
    created: i64,
) -> Vec<Result<ChatCompletionChunk, ProviderError>> {
    let mut results = Vec::new();
    let mut data_line: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if let Some(stripped) = line.strip_prefix("data: ") {
            data_line = Some(stripped.to_string());
        } else if line.is_empty() {
            if let Some(data) = data_line.take() {
                if data == "[DONE]" {
                    continue;
                }
                match serde_json::from_str::<ResponsesEvent>(&data) {
                    Ok(event) => {
                        let chunks = responses_event_to_openai_chunks(event, id, model, created);
                        results.extend(chunks);
                    }
                    Err(_) => {
                        // Unknown event type — skip silently to stay forward-compatible.
                    }
                }
            }
        }
    }

    // Handle data that arrives without a trailing blank line.
    if let Some(data) = data_line {
        if data != "[DONE]" {
            if let Ok(event) = serde_json::from_str::<ResponsesEvent>(&data) {
                let chunks = responses_event_to_openai_chunks(event, id, model, created);
                results.extend(chunks);
            }
        }
    }

    results
}

fn responses_event_to_openai_chunks(
    event: ResponsesEvent,
    id: &str,
    model: &str,
    created: i64,
) -> Vec<Result<ChatCompletionChunk, ProviderError>> {
    match event {
        ResponsesEvent::OutputTextDelta { delta, .. } => {
            vec![Ok(ChatCompletionChunk {
                id: id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some(delta),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            })]
        }
        ResponsesEvent::FunctionCallArgumentsDelta {
            delta,
            output_index,
            call_id,
            ..
        } => {
            let tool_call_delta = serde_json::json!([{
                "index": output_index.unwrap_or(0),
                "id": call_id.unwrap_or_default(),
                "type": "function",
                "function": { "arguments": delta }
            }]);
            vec![Ok(ChatCompletionChunk {
                id: id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: None,
                        tool_calls: Some(tool_call_delta.as_array().cloned().unwrap_or_default()),
                    },
                    finish_reason: None,
                }],
            })]
        }
        ResponsesEvent::FunctionCallArgumentsDone { .. } => {
            // Emit a tool_calls finish chunk.
            vec![Ok(ChatCompletionChunk {
                id: id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta::default(),
                    finish_reason: Some("tool_calls".to_string()),
                }],
            })]
        }
        ResponsesEvent::Completed { response }
        | ResponsesEvent::Done {
            response: Some(response),
        } => {
            // Emit a stop chunk.
            let mut chunk = ChatCompletionChunk {
                id: response.id.unwrap_or_else(|| id.to_string()),
                object: "chat.completion.chunk".to_string(),
                created,
                model: response.model.unwrap_or_else(|| model.to_string()),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta::default(),
                    finish_reason: Some("stop".to_string()),
                }],
            };
            // Prefix the id the same way as non-streaming responses.
            if !chunk.id.starts_with("chatcmpl-") {
                chunk.id = format!("chatcmpl-{}", chunk.id);
            }
            vec![Ok(chunk)]
        }
        ResponsesEvent::Done { response: None } => {
            vec![Ok(ChatCompletionChunk {
                id: id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta::default(),
                    finish_reason: Some("stop".to_string()),
                }],
            })]
        }
        ResponsesEvent::Failed { error } => {
            let msg = error
                .and_then(|e| e.message)
                .unwrap_or_else(|| "response.failed".to_string());
            vec![Err(ProviderError::ProviderResponse {
                status: 500,
                message: msg,
            })]
        }
        ResponsesEvent::Error { code, message } => {
            let msg =
                message.unwrap_or_else(|| code.unwrap_or_else(|| "unknown error".to_string()));
            vec![Err(ProviderError::ProviderResponse {
                status: 500,
                message: msg,
            })]
        }
        // OutputTextDone — no streaming content to emit.
        ResponsesEvent::OutputTextDone { .. } => vec![],
    }
}

/// Aggregate a streaming Responses API response into a single
/// `ChatCompletionResponse` (for non-streaming callers).
fn aggregate_stream_to_response(
    chunks: Vec<ChatCompletionChunk>,
    model: &str,
    completion_id: &str,
    created: i64,
) -> ChatCompletionResponse {
    let mut text = String::new();
    let mut finish_reason: Option<String> = None;
    let mut final_id = completion_id.to_string();
    let mut final_model = model.to_string();

    for chunk in &chunks {
        final_id = chunk.id.clone();
        final_model = chunk.model.clone();
        for choice in &chunk.choices {
            if let Some(content) = &choice.delta.content {
                text.push_str(content);
            }
            if choice.finish_reason.is_some() {
                finish_reason = choice.finish_reason.clone();
            }
        }
    }

    ChatCompletionResponse {
        id: final_id,
        object: "chat.completion".to_string(),
        created,
        model: final_model,
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
        usage: Usage::default(),
    }
}

/// Aggregate a Responses API SSE stream into a single JSON Value representing
/// the final response object (extracted from the `response.completed` event).
///
/// Falls back to a minimal synthetic response if no completed event is found.
fn aggregate_responses_sse_to_json(sse_text: &str) -> Value {
    let mut last_response: Option<Value> = None;
    let mut current_data: Option<String> = None;

    for line in sse_text.lines() {
        let line = line.trim();
        if let Some(stripped) = line.strip_prefix("data: ") {
            current_data = Some(stripped.to_string());
        } else if line.is_empty() {
            if let Some(data) = current_data.take() {
                if data == "[DONE]" {
                    continue;
                }
                if let Ok(parsed) = serde_json::from_str::<Value>(&data) {
                    let event_type = parsed
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if event_type == "response.completed" || event_type == "response.done" {
                        if let Some(resp) = parsed.get("response") {
                            last_response = Some(resp.clone());
                        }
                    }
                }
            }
        }
    }

    // Handle data without trailing blank line
    if let Some(data) = current_data {
        if data != "[DONE]" {
            if let Ok(parsed) = serde_json::from_str::<Value>(&data) {
                let event_type = parsed
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if event_type == "response.completed" || event_type == "response.done" {
                    if let Some(resp) = parsed.get("response") {
                        last_response = Some(resp.clone());
                    }
                }
            }
        }
    }

    last_response.unwrap_or_else(|| {
        serde_json::json!({
            "id": format!("resp_{}", Uuid::new_v4().simple()),
            "object": "response",
            "status": "completed",
            "output": [],
            "usage": {"input_tokens": 0, "output_tokens": 0}
        })
    })
}

// ── Provider implementation ───────────────────────────────────────────────────

#[async_trait]
impl Provider for ChatGptSubscriptionProvider {
    fn name(&self) -> &str {
        "chatgpt-subscription"
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        // Always stream internally; aggregate on behalf of the non-streaming caller.
        let stream = self.chat_completions_stream(req.clone()).await?;
        let chunks: Vec<_> = stream.filter_map(|r| async move { r.ok() }).collect().await;

        let completion_id = new_completion_id();
        let created = Utc::now().timestamp();
        Ok(aggregate_stream_to_response(
            chunks,
            &req.model,
            &completion_id,
            created,
        ))
    }

    async fn chat_completions_stream(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        let (token, account_id) = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;

        let responses_req = translate_request(&req);
        debug!(model = %req.model, "Sending streaming request via chatgpt-subscription");

        let mut builder = self
            .client
            .post(RESPONSES_API_URL)
            .header("Authorization", format!("Bearer {}", token))
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "pi")
            .header("User-Agent", &self.user_agent)
            .header("Content-Type", "application/json");

        if let Some(aid) = &account_id {
            builder = builder.header("chatgpt-account-id", aid);
        }

        let response = builder.json(&responses_req).send().await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "chatgpt-subscription error response");
            return Err(ProviderError::ProviderResponse {
                status,
                message: body,
            });
        }

        let model = req.model.clone();
        let completion_id = new_completion_id();
        let created = Utc::now().timestamp();
        let byte_stream = response.bytes_stream();

        let stream = byte_stream.flat_map(move |result| {
            let id = completion_id.clone();
            let model_name = model.clone();
            let events: Vec<Result<ChatCompletionChunk, ProviderError>> = match result {
                Err(e) => vec![Err(ProviderError::Http(e))],
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    parse_responses_sse_chunk(&text, &id, &model_name, created)
                }
            };
            futures::stream::iter(events)
        });

        Ok(Box::pin(stream))
    }

    async fn proxy_responses(
        &self,
        body: serde_json::Value,
        _is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let (token, account_id) = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;

        debug!("Sending passthrough Responses API request via chatgpt-subscription");

        let mut builder = self
            .client
            .post(RESPONSES_API_URL)
            .header("Authorization", format!("Bearer {}", token))
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "pi")
            .header("User-Agent", &self.user_agent)
            .header("Content-Type", "application/json");

        if let Some(aid) = &account_id {
            builder = builder.header("chatgpt-account-id", aid);
        }

        let response = builder.json(&body).send().await?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            error!(status = status_code, body = %body, "chatgpt-subscription responses proxy error");
            return Err(ProviderError::ProviderResponse {
                status: status_code,
                message: body,
            });
        }

        Ok(response)
    }

    /// Forward an Anthropic Messages API request through ChatGPT subscription.
    ///
    /// Converts the Messages API request to Responses API format, sends it to
    /// the ChatGPT `/backend-api/codex/responses` endpoint, and converts the
    /// response back to Messages API format.
    async fn proxy_messages(
        &self,
        body: serde_json::Value,
        is_stream: bool,
        _client_betas: Option<String>,
    ) -> Result<reqwest::Response, ProviderError> {
        use bytes::Bytes;
        use crate::transform;

        let (token, account_id) = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;

        // Convert Messages → Responses request
        let mut responses_body = transform::messages_to_responses_request(&body);

        // ChatGPT Responses API requires stream: true, store: false, and other fields.
        responses_body["stream"] = serde_json::json!(true);
        responses_body["store"] = serde_json::json!(false);
        if responses_body.get("instructions").is_none()
            || responses_body["instructions"].is_null()
        {
            responses_body["instructions"] =
                serde_json::json!("You are a helpful assistant.");
        }

        debug!("Sending Messages→Responses bridged request via chatgpt-subscription");

        let mut builder = self
            .client
            .post(RESPONSES_API_URL)
            .header("Authorization", format!("Bearer {}", token))
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "pi")
            .header("User-Agent", &self.user_agent)
            .header("Content-Type", "application/json");

        if let Some(aid) = &account_id {
            builder = builder.header("chatgpt-account-id", aid);
        }

        let upstream = builder.json(&responses_body).send().await?;

        let status = upstream.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let msg = upstream.text().await.unwrap_or_default();
            error!(status = status_code, body = %msg, "chatgpt-subscription messages→responses bridge error");
            return Err(ProviderError::ProviderResponse {
                status: status_code,
                message: msg,
            });
        }

        let http_resp = if is_stream {
            // Buffer the Responses SSE stream and convert to Messages SSE.
            let sse_text = upstream.text().await?;
            let messages_sse = transform::convert_responses_sse_stream(&sse_text)
                .map_err(ProviderError::Serialisation)?;
            http::Response::builder()
                .status(200u16)
                .header("content-type", "text/event-stream; charset=utf-8")
                .body(Bytes::from(messages_sse))
                .map_err(|e| ProviderError::Internal(e.to_string()))?
        } else {
            // Buffer the Responses SSE (ChatGPT always streams), aggregate, then
            // convert to a non-streaming Messages response.
            let sse_text = upstream.text().await?;
            // Parse the SSE to extract the final response.completed event
            let responses_resp = aggregate_responses_sse_to_json(&sse_text);
            let messages_resp = transform::responses_to_messages_response(&responses_resp);
            let json = serde_json::to_string(&messages_resp)
                .map_err(ProviderError::Serialisation)?;
            http::Response::builder()
                .status(200u16)
                .header("content-type", "application/json")
                .body(Bytes::from(json))
                .map_err(|e| ProviderError::Internal(e.to_string()))?
        };

        Ok(reqwest::Response::from(http_resp))
    }

    fn models(&self) -> Vec<ModelInfo> {
        let now = Utc::now().timestamp();
        self.model_names
            .iter()
            .map(|name| ModelInfo {
                id: name.clone(),
                object: "model".to_string(),
                created: now,
                owned_by: "chatgpt-subscription".to_string(),
            })
            .collect()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::chatgpt_oauth::ChatGptTokenSource;
    use crate::schema::chat::{FunctionDefinition, Message, Tool};

    fn make_request(messages: Vec<Message>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "gpt-5.4".to_string(),
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
    fn test_provider_name() {
        let manager = ChatGptOAuthTokenManager::new(ChatGptTokenSource::Auto, None);
        let provider = ChatGptSubscriptionProvider::new(manager, vec!["gpt-5.4".to_string()]);
        assert_eq!(provider.name(), "chatgpt-subscription");
    }

    #[test]
    fn test_models_owned_by() {
        let manager = ChatGptOAuthTokenManager::new(ChatGptTokenSource::Auto, None);
        let provider = ChatGptSubscriptionProvider::new(
            manager,
            vec!["gpt-5.4".to_string(), "gpt-5.4-pro".to_string()],
        );
        let models = provider.models();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.4");
        assert_eq!(models[0].owned_by, "chatgpt-subscription");
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
        let req = make_request(messages);
        let resp_req = translate_request(&req);
        assert_eq!(resp_req.instructions, Some("You are helpful.".to_string()));
        assert_eq!(resp_req.input.len(), 1);
        assert_eq!(resp_req.input[0].role, "user");
    }

    #[test]
    fn test_translate_request_no_system() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Hi".to_string())),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }];
        let req = make_request(messages);
        let resp_req = translate_request(&req);
        // When no system message is provided, a default instruction is injected
        // because the ChatGPT Codex Responses API requires `instructions` to be non-null.
        assert_eq!(
            resp_req.instructions,
            Some("You are a helpful assistant.".to_string())
        );
        assert_eq!(resp_req.input.len(), 1);
    }

    #[test]
    fn test_translate_request_strips_max_tokens() {
        // max_tokens is not forwarded to the Responses API request body.
        let messages = vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Hi".to_string())),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }];
        let mut req = make_request(messages);
        req.max_tokens = Some(9999);
        let resp_req = translate_request(&req);
        // Verify by serialising — ResponsesRequest has no max_tokens field.
        let json = serde_json::to_value(&resp_req).unwrap();
        assert!(json.get("max_tokens").is_none());
        assert!(json.get("max_output_tokens").is_none());
        assert!(json.get("max_completion_tokens").is_none());
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn test_translate_request_always_sets_stream_true() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Hi".to_string())),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }];
        let req = make_request(messages);
        let resp_req = translate_request(&req);
        assert!(resp_req.stream);
        assert!(!resp_req.store);
    }

    #[test]
    fn test_translate_request_with_tools() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("What's the weather?".to_string())),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }];
        let mut req = make_request(messages);
        req.tools = Some(vec![Tool {
            r#type: "function".to_string(),
            function: FunctionDefinition {
                name: "get_weather".to_string(),
                description: Some("Get weather".to_string()),
                parameters: Some(serde_json::json!({"type": "object"})),
            },
        }]);
        let resp_req = translate_request(&req);
        let tools = resp_req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "get_weather");
        assert_eq!(tools[0].r#type, "function");
    }

    #[test]
    fn test_translate_request_tool_choice_default() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Hi".to_string())),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }];
        let req = make_request(messages);
        let resp_req = translate_request(&req);
        assert_eq!(resp_req.tool_choice, "auto");
    }

    #[test]
    fn test_parse_sse_text_delta() {
        let sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n";
        let id = "chatcmpl-test";
        let model = "gpt-5.4";
        let created = 1_000_000_i64;
        let chunks = parse_responses_sse_chunk(sse, id, model, created);
        assert_eq!(chunks.len(), 1);
        let chunk = chunks[0].as_ref().unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
    }

    #[test]
    fn test_parse_sse_completed() {
        let sse = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_abc\",\"model\":\"gpt-5.4\",\"status\":\"completed\"}}\n\n";
        let chunks = parse_responses_sse_chunk(sse, "chatcmpl-x", "gpt-5.4", 0);
        assert_eq!(chunks.len(), 1);
        let chunk = chunks[0].as_ref().unwrap();
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_parse_sse_error_event() {
        let sse = "data: {\"type\":\"error\",\"code\":\"rate_limit\",\"message\":\"too many requests\"}\n\n";
        let chunks = parse_responses_sse_chunk(sse, "chatcmpl-x", "gpt-5.4", 0);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_err());
    }

    #[test]
    fn test_aggregate_stream() {
        let created = 1_000_000_i64;
        let chunks = vec![
            ChatCompletionChunk {
                id: "chatcmpl-a".to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: "gpt-5.4".to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: Some("assistant".to_string()),
                        content: None,
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            },
            ChatCompletionChunk {
                id: "chatcmpl-a".to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: "gpt-5.4".to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some("Hello".to_string()),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            },
            ChatCompletionChunk {
                id: "chatcmpl-a".to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: "gpt-5.4".to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta::default(),
                    finish_reason: Some("stop".to_string()),
                }],
            },
        ];
        let resp = aggregate_stream_to_response(chunks, "gpt-5.4", "chatcmpl-a", created);
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String("Hello".to_string()))
        );
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    /// Verify that chatgpt-subscription config can coexist with claude-subscription in config.
    #[test]
    fn test_provider_coexistence() {
        let cg_manager = ChatGptOAuthTokenManager::new(ChatGptTokenSource::Auto, None);
        let cg_provider = ChatGptSubscriptionProvider::new(cg_manager, vec!["gpt-5.4".to_string()]);
        assert_eq!(cg_provider.name(), "chatgpt-subscription");

        // Different name from claude-subscription — no conflict.
        assert_ne!(cg_provider.name(), "claude-subscription");
    }
}
