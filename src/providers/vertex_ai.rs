//! Google Vertex AI provider (Gemini models).
//!
//! Translates between the unified OpenAI chat-completions schema and the
//! Vertex AI `generateContent` / `streamGenerateContent` REST API.
//!
//! # Supported endpoints
//! - Regional: `https://{location}-aiplatform.googleapis.com/v1/…`
//! - Global:   `https://aiplatform.googleapis.com/v1/…`
//!
//! # Authentication
//! A GCP OAuth2 Bearer token is obtained via [`VertexTokenManager`], which
//! supports service-account JSON and ADC `authorized_user` credentials.
//!
//! # Format translation
//! ```text
//! OpenAI ChatCompletions  ←→  Gemini generateContent
//! system message          →   systemInstruction
//! user / assistant        →   contents[].role (user / model)
//! temperature             →   generationConfig.temperature
//! max_tokens              →   generationConfig.maxOutputTokens
//! top_p                   →   generationConfig.topP
//! stop                    →   generationConfig.stopSequences
//! ```

// Deserialization structs mirror the Vertex AI wire format exactly.
// Fields are read by serde even when not directly accessed in Rust code.
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

use crate::auth::vertex::VertexTokenManager;
use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Choice, ChunkChoice, Delta,
    Message, ModelInfo, Usage,
};

use super::{Provider, ProviderError};

/// Default GCP region for Vertex AI endpoints.
const DEFAULT_LOCATION: &str = "us-central1";

// ── Gemini API request types ──────────────────────────────────────────────────

/// Top-level `generateContent` request body.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstruction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

/// A single turn in the conversation.
#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

/// A content part (text only — images/blobs not translated here).
#[derive(Debug, Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

/// System instruction container.
#[derive(Debug, Serialize)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

/// Generation configuration parameters.
#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

// ── Gemini API response types ─────────────────────────────────────────────────

/// `generateContent` response (both streaming and non-streaming share this shape).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    #[serde(default)]
    prompt_token_count: u32,
    #[serde(default)]
    candidates_token_count: u32,
    #[serde(default)]
    total_token_count: u32,
}

// ── Provider struct ───────────────────────────────────────────────────────────

/// Vertex AI provider.
pub struct VertexAiProvider {
    client: Client,
    token_manager: Arc<VertexTokenManager>,
    /// GCP project ID.
    project_id: String,
    /// GCP region or "global".
    location: String,
    /// Known models (from config).
    model_names: Vec<String>,
}

impl VertexAiProvider {
    /// Create a new Vertex AI provider.
    pub fn new(
        token_manager: Arc<VertexTokenManager>,
        project_id: String,
        location: String,
        model_names: Vec<String>,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()?,
            token_manager,
            project_id,
            location,
            model_names,
        })
    }

    /// Build the Vertex AI endpoint URL for a given model and action.
    ///
    /// - Regional: `https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:{action}`
    /// - Global:   `https://aiplatform.googleapis.com/v1/projects/{project}/locations/global/publishers/google/models/{model}:{action}`
    fn endpoint_url(&self, model: &str, action: &str) -> String {
        if self.location == "global" {
            format!(
                "https://aiplatform.googleapis.com/v1/projects/{}/locations/global/publishers/google/models/{}:{}",
                self.project_id, model, action
            )
        } else {
            format!(
                "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/google/models/{}:{}",
                self.location, self.project_id, self.location, model, action
            )
        }
    }

    /// Build the Vertex AI endpoint URL for a Claude model (Anthropic publisher).
    ///
    /// - Non-streaming: `…/publishers/anthropic/models/{model}:rawPredict`
    /// - Streaming:     `…/publishers/anthropic/models/{model}:streamRawPredict`
    fn claude_endpoint_url(&self, model: &str, action: &str) -> String {
        if self.location == "global" {
            format!(
                "https://aiplatform.googleapis.com/v1/projects/{}/locations/global/publishers/anthropic/models/{}:{}",
                self.project_id, model, action
            )
        } else {
            format!(
                "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/anthropic/models/{}:{}",
                self.location, self.project_id, self.location, model, action
            )
        }
    }
}

// ── Translation helpers ───────────────────────────────────────────────────────

/// Extract plain text from an OpenAI message content value.
fn extract_text(content: &Option<Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                // Support {type: "text", text: "..."} content parts.
                if p.get("type").and_then(Value::as_str) == Some("text") {
                    p.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Translate an OpenAI `ChatCompletionRequest` into a Gemini `GenerateContentRequest`.
fn translate_request(req: &ChatCompletionRequest) -> GenerateContentRequest {
    let mut system_text: Option<String> = None;
    let mut contents: Vec<GeminiContent> = Vec::new();

    for msg in &req.messages {
        match msg.role.as_str() {
            "system" => {
                let text = extract_text(&msg.content);
                if !text.is_empty() {
                    system_text = Some(text);
                }
            }
            "assistant" => {
                let text = extract_text(&msg.content);
                contents.push(GeminiContent {
                    role: "model".to_string(),
                    parts: vec![GeminiPart { text }],
                });
            }
            _ => {
                // "user" and any other roles map to "user".
                let text = extract_text(&msg.content);
                contents.push(GeminiContent {
                    role: "user".to_string(),
                    parts: vec![GeminiPart { text }],
                });
            }
        }
    }

    let system_instruction = system_text.map(|text| GeminiSystemInstruction {
        parts: vec![GeminiPart { text }],
    });

    // Only include generationConfig if at least one field is set.
    let stop_sequences = match &req.stop {
        Some(Value::String(s)) => Some(vec![s.clone()]),
        Some(Value::Array(arr)) => Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
        ),
        _ => None,
    };

    let has_gen_config = req.temperature.is_some()
        || req.max_tokens.is_some()
        || req.top_p.is_some()
        || stop_sequences.is_some();

    let generation_config = if has_gen_config {
        Some(GenerationConfig {
            temperature: req.temperature,
            max_output_tokens: req.max_tokens,
            top_p: req.top_p,
            stop_sequences,
        })
    } else {
        None
    };

    GenerateContentRequest {
        contents,
        system_instruction,
        generation_config,
    }
}

/// Map a Gemini `finishReason` string to an OpenAI `finish_reason`.
fn map_finish_reason(reason: Option<&str>) -> Option<String> {
    reason.map(|r| match r {
        "STOP" => "stop".to_string(),
        "MAX_TOKENS" => "length".to_string(),
        "SAFETY" => "content_filter".to_string(),
        other => other.to_lowercase(),
    })
}

/// Generate a new chat completion ID.
fn new_completion_id() -> String {
    format!("chatcmpl-{}", Uuid::new_v4().simple())
}

/// Translate a full (non-streaming) Gemini response into OpenAI format.
fn translate_response(resp: GenerateContentResponse, model: &str) -> ChatCompletionResponse {
    let candidate = resp.candidates.into_iter().next();

    let text = candidate
        .as_ref()
        .and_then(|c| c.content.as_ref())
        .and_then(|c| c.parts.first())
        .map(|p| p.text.clone())
        .unwrap_or_default();

    let finish_reason = candidate
        .as_ref()
        .and_then(|c| c.finish_reason.as_deref())
        .and_then(|r| map_finish_reason(Some(r)));

    let usage = resp
        .usage_metadata
        .map(|u| Usage {
            prompt_tokens: u.prompt_token_count,
            completion_tokens: u.candidates_token_count,
            total_tokens: u.total_token_count,
        })
        .unwrap_or_default();

    ChatCompletionResponse {
        id: new_completion_id(),
        object: "chat.completion".to_string(),
        created: Utc::now().timestamp(),
        model: model.to_string(),
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
        usage,
    }
}

/// Parse one or more Gemini SSE lines into OpenAI streaming chunks.
///
/// Gemini streaming format: each SSE event is `data: {json}\n\n`.
/// The JSON has the same shape as a non-streaming `generateContent` response.
fn parse_gemini_sse(
    text: &str,
    id: &str,
    model: &str,
    created: i64,
    is_first_chunk: &mut bool,
) -> Vec<Result<ChatCompletionChunk, ProviderError>> {
    let mut results = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };

        // Skip the SSE heartbeat / keep-alive.
        if data == "[DONE]" {
            break;
        }

        match serde_json::from_str::<GenerateContentResponse>(data) {
            Ok(resp) => {
                let candidate = resp.candidates.into_iter().next();

                let content_text = candidate
                    .as_ref()
                    .and_then(|c| c.content.as_ref())
                    .and_then(|c| c.parts.first())
                    .map(|p| p.text.clone());

                let finish_reason = candidate
                    .as_ref()
                    .and_then(|c| c.finish_reason.as_deref())
                    .and_then(|r| map_finish_reason(Some(r)));

                let delta = if *is_first_chunk {
                    *is_first_chunk = false;
                    Delta {
                        role: Some("assistant".to_string()),
                        content: content_text,
                        tool_calls: None,
                    }
                } else {
                    Delta {
                        role: None,
                        content: content_text,
                        tool_calls: None,
                    }
                };

                results.push(Ok(ChatCompletionChunk {
                    id: id.to_string(),
                    object: "chat.completion.chunk".to_string(),
                    created,
                    model: model.to_string(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta,
                        finish_reason,
                    }],
                }));
            }
            Err(e) => {
                results.push(Err(ProviderError::Serialisation(e)));
            }
        }
    }

    results
}

// ── Provider implementation ───────────────────────────────────────────────────

#[async_trait]
impl Provider for VertexAiProvider {
    fn name(&self) -> &str {
        "vertex-ai"
    }

    fn capabilities(&self) -> &'static [super::Capability] {
        use super::Capability::*;
        &[ChatCompletions, Streaming, MessagesApi]
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let token = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;

        let gemini_req = translate_request(&req);
        let url = self.endpoint_url(&req.model, "generateContent");

        debug!(model = %req.model, url = %url, "Sending non-streaming request to Vertex AI");
        debug!(
            request_body = %serde_json::to_string_pretty(&gemini_req).unwrap_or_default(),
            "Vertex AI Gemini outgoing request payload"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(&gemini_req)
            .send()
            .await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "Vertex AI error response");
            return Err(ProviderError::ProviderResponse {
                status,
                message: body,
            });
        }

        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        let preview = if body_bytes.len() > 4096 {
            format!(
                "{}... (truncated, {} bytes total)",
                String::from_utf8_lossy(&body_bytes[..4096]),
                body_bytes.len()
            )
        } else {
            String::from_utf8_lossy(&body_bytes).to_string()
        };
        debug!(
            status = status,
            response_body = %preview,
            "Vertex AI Gemini upstream response"
        );

        let gemini_resp: GenerateContentResponse =
            serde_json::from_slice(&body_bytes).map_err(ProviderError::Serialisation)?;
        Ok(translate_response(gemini_resp, &req.model))
    }

    async fn chat_completions_stream(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        let token = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;

        let gemini_req = translate_request(&req);
        // Streaming endpoint: streamGenerateContent?alt=sse
        let url = format!(
            "{}?alt=sse",
            self.endpoint_url(&req.model, "streamGenerateContent")
        );

        debug!(model = %req.model, url = %url, "Sending streaming request to Vertex AI");
        debug!(
            request_body = %serde_json::to_string_pretty(&gemini_req).unwrap_or_default(),
            "Vertex AI Gemini outgoing streaming request payload"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(&gemini_req)
            .send()
            .await?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "Vertex AI streaming error response");
            return Err(ProviderError::ProviderResponse {
                status,
                message: body,
            });
        }

        debug!(
            status = status,
            content_type = ?response.headers().get("content-type"),
            "Vertex AI Gemini upstream streaming response (body not logged)"
        );

        let model = req.model.clone();
        let completion_id = new_completion_id();
        let created = Utc::now().timestamp();
        let byte_stream = response.bytes_stream();

        // Track whether the first chunk has been emitted (to include the role).
        let mut is_first = true;

        let stream = byte_stream.flat_map(move |result| {
            let id = completion_id.clone();
            let model_name = model.clone();
            let events: Vec<Result<ChatCompletionChunk, ProviderError>> = match result {
                Err(e) => vec![Err(ProviderError::Http(e))],
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    parse_gemini_sse(&text, &id, &model_name, created, &mut is_first)
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
                owned_by: if name.starts_with("claude-") {
                    "anthropic".to_string()
                } else {
                    "google".to_string()
                },
            })
            .collect()
    }

    /// Proxy a raw Anthropic Messages API request to Claude on Vertex AI.
    ///
    /// Only Claude models (names starting with `claude-`) are supported here.
    /// The request body is forwarded as-is; GCP OAuth replaces the API-key auth.
    async fn proxy_messages(
        &self,
        body: serde_json::Value,
        is_stream: bool,
        _client_betas: Option<String>,
    ) -> Result<reqwest::Response, ProviderError> {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if !model.starts_with("claude-") {
            return Err(ProviderError::Unsupported(format!(
                "Model '{}' is not a Claude model; use /v1/chat/completions for Gemini models",
                model
            )));
        }

        let token = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;

        let action = if is_stream {
            "streamRawPredict"
        } else {
            "rawPredict"
        };
        let url = self.claude_endpoint_url(&model, action);

        // Vertex AI rawPredict requires specific body transformations:
        // 1. `anthropic_version` must be in the body (Claude Code sends it as
        //    an HTTP header instead).
        // 2. `model` must NOT be in the body — the model is specified in the
        //    URL path. Sending it in the body causes 501 UNIMPLEMENTED.
        // 3. Strip unsupported fields from `cache_control` objects (e.g.
        //    `scope`) that Claude Code sends but Vertex rejects with
        //    "Extra inputs are not permitted".
        let mut body = body;
        if body.get("anthropic_version").is_none() {
            body["anthropic_version"] = Value::String("vertex-2023-10-16".to_string());
        }
        // Remove `model` from the body — Vertex takes it from the URL path.
        if let Some(obj) = body.as_object_mut() {
            obj.remove("model");
        }
        // Strip unsupported fields from cache_control throughout the body.
        strip_cache_control_extras(&mut body);

        debug!(
            model = %model,
            url = %url,
            stream = is_stream,
            "Forwarding Messages API request to Claude on Vertex AI"
        );
        debug!(
            url = %url,
            request_body = %serde_json::to_string_pretty(&body).unwrap_or_default(),
            "Vertex AI Claude outgoing request payload"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !is_stream {
            let status = response.status();
            let ct = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();
            let body_bytes = response
                .bytes()
                .await
                .map_err(|e| ProviderError::Internal(e.to_string()))?;
            let preview = if body_bytes.len() > 4096 {
                format!(
                    "{}... (truncated, {} bytes total)",
                    String::from_utf8_lossy(&body_bytes[..4096]),
                    body_bytes.len()
                )
            } else {
                String::from_utf8_lossy(&body_bytes).to_string()
            };
            debug!(
                status = %status,
                content_type = %ct,
                response_body = %preview,
                "Vertex AI Claude upstream response"
            );
            let rebuilt = http::Response::builder()
                .status(status.as_u16())
                .header("content-type", ct)
                .body(body_bytes)
                .unwrap();
            Ok(reqwest::Response::from(rebuilt))
        } else {
            debug!(
                status = %response.status().as_u16(),
                content_type = ?response.headers().get("content-type"),
                "Vertex AI Claude upstream streaming response (body not logged)"
            );
            Ok(response)
        }
    }
}

// ── Vertex body sanitisation helpers ──────────────────────────────────────────

/// Recursively strip fields from `cache_control` objects that Vertex AI does
/// not accept (e.g. `scope`).  Claude Code may send:
///
/// ```json
/// {"cache_control": {"type": "ephemeral", "scope": "turn"}}
/// ```
///
/// Vertex rejects the extra `scope` key with 400 "Extra inputs are not
/// permitted".  We keep only `type` inside every `cache_control` object we
/// encounter anywhere in the JSON tree.
fn strip_cache_control_extras(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(cc) = map.get_mut("cache_control") {
                if let Some(cc_obj) = cc.as_object_mut() {
                    // Keep only the `type` key — everything else is unsupported.
                    cc_obj.retain(|k, _| k == "type");
                }
            }
            for v in map.values_mut() {
                strip_cache_control_extras(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                strip_cache_control_extras(v);
            }
        }
        _ => {}
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::chat::Message;

    fn msg(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: Some(Value::String(content.to_string())),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }
    }

    fn make_request(messages: Vec<Message>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "gemini-2.5-pro".to_string(),
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
            response_format: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
        }
    }

    // ── translate_request ─────────────────────────────────────────────────────

    #[test]
    fn test_system_goes_to_system_instruction() {
        let req = make_request(vec![
            msg("system", "You are helpful."),
            msg("user", "Hello"),
        ]);
        let gemini = translate_request(&req);
        assert!(gemini.system_instruction.is_some());
        assert_eq!(
            gemini.system_instruction.unwrap().parts[0].text,
            "You are helpful."
        );
        assert_eq!(gemini.contents.len(), 1);
        assert_eq!(gemini.contents[0].role, "user");
    }

    #[test]
    fn test_assistant_maps_to_model_role() {
        let req = make_request(vec![
            msg("user", "Hi"),
            msg("assistant", "Hello!"),
            msg("user", "How are you?"),
        ]);
        let gemini = translate_request(&req);
        assert_eq!(gemini.contents[0].role, "user");
        assert_eq!(gemini.contents[1].role, "model");
        assert_eq!(gemini.contents[2].role, "user");
    }

    #[test]
    fn test_no_system_message() {
        let req = make_request(vec![msg("user", "Hello")]);
        let gemini = translate_request(&req);
        assert!(gemini.system_instruction.is_none());
    }

    #[test]
    fn test_generation_config_temperature() {
        let mut req = make_request(vec![msg("user", "Hi")]);
        req.temperature = Some(0.7);
        let gemini = translate_request(&req);
        assert!(gemini.generation_config.is_some());
        assert_eq!(gemini.generation_config.unwrap().temperature, Some(0.7));
    }

    #[test]
    fn test_generation_config_max_tokens() {
        let mut req = make_request(vec![msg("user", "Hi")]);
        req.max_tokens = Some(1024);
        let gemini = translate_request(&req);
        let gc = gemini.generation_config.unwrap();
        assert_eq!(gc.max_output_tokens, Some(1024));
    }

    #[test]
    fn test_generation_config_top_p() {
        let mut req = make_request(vec![msg("user", "Hi")]);
        req.top_p = Some(0.9);
        let gemini = translate_request(&req);
        let gc = gemini.generation_config.unwrap();
        assert_eq!(gc.top_p, Some(0.9));
    }

    #[test]
    fn test_stop_string_becomes_sequence() {
        let mut req = make_request(vec![msg("user", "Hi")]);
        req.stop = Some(Value::String("END".to_string()));
        let gemini = translate_request(&req);
        let gc = gemini.generation_config.unwrap();
        assert_eq!(gc.stop_sequences, Some(vec!["END".to_string()]));
    }

    #[test]
    fn test_stop_array_becomes_sequences() {
        let mut req = make_request(vec![msg("user", "Hi")]);
        req.stop = Some(Value::Array(vec![
            Value::String("A".to_string()),
            Value::String("B".to_string()),
        ]));
        let gemini = translate_request(&req);
        let gc = gemini.generation_config.unwrap();
        assert_eq!(
            gc.stop_sequences,
            Some(vec!["A".to_string(), "B".to_string()])
        );
    }

    #[test]
    fn test_no_generation_config_when_no_params() {
        let req = make_request(vec![msg("user", "Hi")]);
        let gemini = translate_request(&req);
        assert!(gemini.generation_config.is_none());
    }

    #[test]
    fn test_content_parts_extracted() {
        let parts_content = Value::Array(vec![
            serde_json::json!({"type": "text", "text": "Hello "}),
            serde_json::json!({"type": "text", "text": "world"}),
        ]);
        let msg = Message {
            role: "user".to_string(),
            content: Some(parts_content),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        };
        let req = make_request(vec![msg]);
        let gemini = translate_request(&req);
        assert_eq!(gemini.contents[0].parts[0].text, "Hello world");
    }

    // ── map_finish_reason ─────────────────────────────────────────────────────

    #[test]
    fn test_finish_reason_stop() {
        assert_eq!(map_finish_reason(Some("STOP")), Some("stop".to_string()));
    }

    #[test]
    fn test_finish_reason_max_tokens() {
        assert_eq!(
            map_finish_reason(Some("MAX_TOKENS")),
            Some("length".to_string())
        );
    }

    #[test]
    fn test_finish_reason_safety() {
        assert_eq!(
            map_finish_reason(Some("SAFETY")),
            Some("content_filter".to_string())
        );
    }

    #[test]
    fn test_finish_reason_none() {
        assert_eq!(map_finish_reason(None), None);
    }

    #[test]
    fn test_finish_reason_unknown_lowercased() {
        assert_eq!(
            map_finish_reason(Some("RECITATION")),
            Some("recitation".to_string())
        );
    }

    // ── translate_response ────────────────────────────────────────────────────

    #[test]
    fn test_translate_response_basic() {
        let resp = GenerateContentResponse {
            candidates: vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: "model".to_string(),
                    parts: vec![GeminiPart {
                        text: "Hello, world!".to_string(),
                    }],
                }),
                finish_reason: Some("STOP".to_string()),
            }],
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: 10,
                candidates_token_count: 5,
                total_token_count: 15,
            }),
        };

        let openai = translate_response(resp, "gemini-2.5-pro");
        assert!(openai.id.starts_with("chatcmpl-"));
        assert_eq!(openai.object, "chat.completion");
        assert_eq!(openai.model, "gemini-2.5-pro");
        assert_eq!(openai.choices[0].finish_reason, Some("stop".to_string()));
        assert_eq!(openai.usage.prompt_tokens, 10);
        assert_eq!(openai.usage.completion_tokens, 5);
        assert_eq!(openai.usage.total_tokens, 15);
        if let Some(Value::String(text)) = &openai.choices[0].message.content {
            assert_eq!(text, "Hello, world!");
        } else {
            panic!("Expected string content");
        }
    }

    #[test]
    fn test_translate_response_empty_candidates() {
        let resp = GenerateContentResponse {
            candidates: vec![],
            usage_metadata: None,
        };
        let openai = translate_response(resp, "gemini-2.5-pro");
        // Should produce an empty-text assistant message without panicking.
        assert_eq!(openai.choices.len(), 1);
        assert_eq!(
            openai.choices[0].message.content,
            Some(Value::String(String::new()))
        );
        assert_eq!(openai.usage.total_tokens, 0);
    }

    // ── endpoint_url ──────────────────────────────────────────────────────────

    fn make_provider() -> VertexAiProvider {
        let mgr = VertexTokenManager::new(None);
        VertexAiProvider::new(
            mgr,
            "my-project".to_string(),
            "us-central1".to_string(),
            vec!["gemini-2.5-pro".to_string()],
        )
        .unwrap()
    }

    #[test]
    fn test_endpoint_url_regional() {
        let p = make_provider();
        let url = p.endpoint_url("gemini-2.5-pro", "generateContent");
        assert_eq!(
            url,
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.5-pro:generateContent"
        );
    }

    #[test]
    fn test_endpoint_url_global() {
        let mgr = VertexTokenManager::new(None);
        let p = VertexAiProvider::new(mgr, "my-project".to_string(), "global".to_string(), vec![])
            .unwrap();
        let url = p.endpoint_url("gemini-2.5-pro", "generateContent");
        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global/publishers/google/models/gemini-2.5-pro:generateContent"
        );
    }

    #[test]
    fn test_streaming_endpoint_url() {
        let p = make_provider();
        let url = format!(
            "{}?alt=sse",
            p.endpoint_url("gemini-2.5-pro", "streamGenerateContent")
        );
        assert!(url.contains("streamGenerateContent"));
        assert!(url.ends_with("?alt=sse"));
    }

    // ── parse_gemini_sse ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_sse_first_chunk_has_role() {
        let data = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "Hi"}]},
                "finishReason": null
            }]
        });
        let line = format!("data: {}", data);
        let mut is_first = true;
        let results = parse_gemini_sse(&line, "id1", "gemini-2.5-pro", 0, &mut is_first);
        assert_eq!(results.len(), 1);
        let chunk = results[0].as_ref().unwrap();
        assert_eq!(chunk.choices[0].delta.role, Some("assistant".to_string()));
        assert_eq!(chunk.choices[0].delta.content, Some("Hi".to_string()));
        assert!(!is_first); // flag flipped
    }

    #[test]
    fn test_parse_sse_subsequent_chunk_no_role() {
        let data = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": " there"}]},
                "finishReason": null
            }]
        });
        let line = format!("data: {}", data);
        let mut is_first = false;
        let results = parse_gemini_sse(&line, "id1", "gemini-2.5-pro", 0, &mut is_first);
        let chunk = results[0].as_ref().unwrap();
        assert_eq!(chunk.choices[0].delta.role, None);
        assert_eq!(chunk.choices[0].delta.content, Some(" there".to_string()));
    }

    #[test]
    fn test_parse_sse_final_chunk_has_finish_reason() {
        let data = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": ""}]},
                "finishReason": "STOP"
            }]
        });
        let line = format!("data: {}", data);
        let mut is_first = false;
        let results = parse_gemini_sse(&line, "id1", "gemini-2.5-pro", 0, &mut is_first);
        let chunk = results[0].as_ref().unwrap();
        assert_eq!(chunk.choices[0].finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_parse_sse_ignores_non_data_lines() {
        let text = "event: something\ncomment: ignored\n";
        let mut is_first = true;
        let results = parse_gemini_sse(text, "id1", "gemini-2.5-pro", 0, &mut is_first);
        assert!(results.is_empty());
    }

    // ── models() ─────────────────────────────────────────────────────────────

    #[test]
    fn test_models_owned_by_google() {
        let p = make_provider();
        let models = p.models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-2.5-pro");
        assert_eq!(models[0].owned_by, "google");
        assert_eq!(models[0].object, "model");
    }

    #[test]
    fn test_models_owned_by_anthropic_for_claude() {
        let mgr = VertexTokenManager::new(None);
        let p = VertexAiProvider::new(
            mgr,
            "my-project".to_string(),
            "us-east5".to_string(),
            vec![
                "claude-sonnet-4-6".to_string(),
                "gemini-2.5-pro".to_string(),
            ],
        )
        .unwrap();
        let models = p.models();
        let claude = models.iter().find(|m| m.id == "claude-sonnet-4-6").unwrap();
        let gemini = models.iter().find(|m| m.id == "gemini-2.5-pro").unwrap();
        assert_eq!(claude.owned_by, "anthropic");
        assert_eq!(gemini.owned_by, "google");
    }

    // ── claude_endpoint_url ───────────────────────────────────────────────────

    #[test]
    fn test_claude_endpoint_url_regional_raw_predict() {
        let mgr = VertexTokenManager::new(None);
        let p = VertexAiProvider::new(
            mgr,
            "my-project".to_string(),
            "us-east5".to_string(),
            vec![],
        )
        .unwrap();
        let url = p.claude_endpoint_url("claude-sonnet-4-6", "rawPredict");
        assert_eq!(
            url,
            "https://us-east5-aiplatform.googleapis.com/v1/projects/my-project/locations/us-east5/publishers/anthropic/models/claude-sonnet-4-6:rawPredict"
        );
    }

    #[test]
    fn test_claude_endpoint_url_regional_stream_raw_predict() {
        let mgr = VertexTokenManager::new(None);
        let p = VertexAiProvider::new(
            mgr,
            "my-project".to_string(),
            "us-east5".to_string(),
            vec![],
        )
        .unwrap();
        let url = p.claude_endpoint_url("claude-sonnet-4-6", "streamRawPredict");
        assert_eq!(
            url,
            "https://us-east5-aiplatform.googleapis.com/v1/projects/my-project/locations/us-east5/publishers/anthropic/models/claude-sonnet-4-6:streamRawPredict"
        );
    }

    #[test]
    fn test_claude_endpoint_url_global() {
        let mgr = VertexTokenManager::new(None);
        let p = VertexAiProvider::new(mgr, "my-project".to_string(), "global".to_string(), vec![])
            .unwrap();
        let url = p.claude_endpoint_url("claude-haiku-4-5-20251001", "rawPredict");
        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global/publishers/anthropic/models/claude-haiku-4-5-20251001:rawPredict"
        );
    }

    // ── strip_cache_control_extras ────────────────────────────────────────

    #[test]
    fn test_strip_cache_control_scope() {
        let mut body = serde_json::json!({
            "system": [
                {
                    "type": "text",
                    "text": "Hello",
                    "cache_control": {"type": "ephemeral", "scope": "turn"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "Hi",
                            "cache_control": {"type": "ephemeral", "scope": "session", "extra": true}
                        }
                    ]
                }
            ]
        });
        strip_cache_control_extras(&mut body);

        // scope and extra should be stripped; type should remain
        let sys_cc = &body["system"][0]["cache_control"];
        assert_eq!(sys_cc, &serde_json::json!({"type": "ephemeral"}));

        let msg_cc = &body["messages"][0]["content"][0]["cache_control"];
        assert_eq!(msg_cc, &serde_json::json!({"type": "ephemeral"}));
    }

    #[test]
    fn test_strip_cache_control_no_cache_control() {
        let mut body = serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}]
        });
        let expected = body.clone();
        strip_cache_control_extras(&mut body);
        assert_eq!(body, expected);
    }
}
