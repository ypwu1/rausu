//! GitHub Copilot provider implementation.
//!
//! GitHub Copilot exposes an OpenAI-compatible chat completions endpoint at
//! `https://api.githubcopilot.com/chat/completions` and a native Anthropic
//! Messages API endpoint at `https://api.githubcopilot.com/v1/messages`.
//! Authentication uses a two-step exchange: a GitHub OAuth token is exchanged
//! for a short-lived Copilot API bearer token via [`CopilotTokenManager`].
//!
//! # Supported endpoints
//!
//! | Route | Support |
//! |-------|---------|
//! | `POST /v1/chat/completions` | ✅ full (streaming + non-streaming) |
//! | `GET /v1/models` | ✅ lists configured model names |
//! | `POST /v1/messages` | ✅ Claude: native passthrough; others: protocol-translated |
//! | `POST /v1/responses` | ✅ Claude: protocol-bridged via `/v1/messages`; others: passthrough |
//!
//! # Model routing
//!
//! Claude models (name starts with `claude`) are forwarded directly to Copilot's
//! `/v1/messages` endpoint as native Anthropic Messages API requests.  All other
//! models are translated to OpenAI ChatCompletions format and sent to
//! `/chat/completions`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use tracing::{debug, error};
use uuid::Uuid;

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

// ── Protocol translation ───────────────────────────────────────────────────────

/// Extract plain text from an Anthropic `system` field, which may be a string
/// or an array of content blocks.
fn extract_system_text(system: &serde_json::Value) -> String {
    match system {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                    block
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Translate an Anthropic Messages API request body to OpenAI ChatCompletions format.
///
/// - `system` (string or content-block array) → prepended `{"role":"system"}` message
/// - `stop_sequences` → `stop`
/// - All other compatible fields (`max_tokens`, `temperature`, `top_p`, `stream`) are
///   forwarded as-is.
/// - Anthropic-only fields (`anthropic_version`, `metadata`, `tool_choice`, `tools`)
///   are dropped.
fn anthropic_to_openai(body: &serde_json::Value) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    // Prepend system message if present.
    if let Some(system) = body.get("system") {
        let text = extract_system_text(system);
        if !text.is_empty() {
            messages.push(serde_json::json!({"role": "system", "content": text}));
        }
    }

    // Append user/assistant messages.
    if let Some(serde_json::Value::Array(msgs)) = body.get("messages") {
        messages.extend_from_slice(msgs);
    }

    let mut req = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "messages": messages,
    });

    for field in &["max_tokens", "temperature", "top_p", "stream"] {
        if let Some(v) = body.get(*field) {
            req[*field] = v.clone();
        }
    }

    // stop_sequences → stop
    if let Some(v) = body.get("stop_sequences") {
        req["stop"] = v.clone();
    }

    req
}

/// Map an OpenAI `finish_reason` string to an Anthropic `stop_reason`.
fn map_finish_reason(reason: &str) -> &'static str {
    match reason {
        "length" => "max_tokens",
        _ => "end_turn",
    }
}

/// Translate an OpenAI ChatCompletions non-streaming response to Anthropic Messages format.
fn openai_to_anthropic_response(openai: &serde_json::Value, model: &str) -> serde_json::Value {
    let id = openai
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| {
            if id.starts_with("msg_") {
                id.to_string()
            } else {
                format!("msg_{}", id.trim_start_matches("chatcmpl-"))
            }
        })
        .unwrap_or_else(|| format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")));

    let content_text = openai
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let finish_reason = openai
        .pointer("/choices/0/finish_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("stop");

    let input_tokens = openai
        .pointer("/usage/prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = openai
        .pointer("/usage/completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    serde_json::json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": content_text}],
        "model": model,
        "stop_reason": map_finish_reason(finish_reason),
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens
        }
    })
}

/// Translate a buffered OpenAI ChatCompletions SSE text to Anthropic Messages SSE format.
///
/// Reads all `data: ` lines, emits the Anthropic event sequence
/// (`message_start` → `content_block_start` → N×`content_block_delta` →
/// `content_block_stop` → `message_delta` → `message_stop`), and returns the
/// result as a single `String`.
fn openai_sse_to_anthropic(sse_text: &str, model: &str) -> Result<String, ProviderError> {
    let mut output = String::new();
    let mut msg_id: Option<String> = None;
    let mut content_started = false;
    let mut stop_reason = "end_turn".to_string();
    let mut output_tokens: u64 = 0;

    for line in sse_text.lines() {
        let data = match line.trim().strip_prefix("data: ") {
            Some(d) => d,
            None => continue,
        };
        if data == "[DONE]" {
            break;
        }

        let chunk: serde_json::Value =
            serde_json::from_str(data).map_err(ProviderError::Serialisation)?;

        // Emit message_start on the first chunk that carries an id.
        if msg_id.is_none() {
            if let Some(id) = chunk.get("id").and_then(|v| v.as_str()) {
                let formatted = if id.starts_with("msg_") {
                    id.to_string()
                } else {
                    format!("msg_{}", id.trim_start_matches("chatcmpl-"))
                };
                msg_id = Some(formatted.clone());

                let event = serde_json::json!({
                    "type": "message_start",
                    "message": {
                        "id": formatted,
                        "type": "message",
                        "role": "assistant",
                        "content": [],
                        "model": model,
                        "usage": {"input_tokens": 0, "output_tokens": 0}
                    }
                });
                output.push_str("event: message_start\ndata: ");
                output.push_str(
                    &serde_json::to_string(&event).map_err(ProviderError::Serialisation)?,
                );
                output.push_str("\n\n");
            }
        }

        // Process the first non-empty choices entry.
        if let Some(choice) = chunk
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        {
            if let Some(delta) = choice.get("delta") {
                // Role delta → emit content_block_start.
                if delta.get("role").is_some() && !content_started {
                    content_started = true;
                    let event = serde_json::json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": {"type": "text", "text": ""}
                    });
                    output.push_str("event: content_block_start\ndata: ");
                    output.push_str(
                        &serde_json::to_string(&event).map_err(ProviderError::Serialisation)?,
                    );
                    output.push_str("\n\n");
                }

                // Text content delta.
                if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                    if !content_started {
                        content_started = true;
                        let event = serde_json::json!({
                            "type": "content_block_start",
                            "index": 0,
                            "content_block": {"type": "text", "text": ""}
                        });
                        output.push_str("event: content_block_start\ndata: ");
                        output.push_str(
                            &serde_json::to_string(&event).map_err(ProviderError::Serialisation)?,
                        );
                        output.push_str("\n\n");
                    }

                    let event = serde_json::json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "text_delta", "text": text}
                    });
                    output.push_str("event: content_block_delta\ndata: ");
                    output.push_str(
                        &serde_json::to_string(&event).map_err(ProviderError::Serialisation)?,
                    );
                    output.push_str("\n\n");
                }
            }

            // Capture finish_reason when non-null.
            if let Some(reason) = choice
                .get("finish_reason")
                .and_then(|v| v.as_str())
                .filter(|r| !r.is_empty())
            {
                stop_reason = map_finish_reason(reason).to_string();
            }
        }

        // Usage may appear at the chunk level (some providers include it here).
        if let Some(usage) = chunk.get("usage") {
            if let Some(v) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                output_tokens = v;
            }
        }
    }

    // content_block_stop
    if content_started {
        let event = serde_json::json!({"type": "content_block_stop", "index": 0});
        output.push_str("event: content_block_stop\ndata: ");
        output.push_str(&serde_json::to_string(&event).map_err(ProviderError::Serialisation)?);
        output.push_str("\n\n");
    }

    // message_delta
    let event = serde_json::json!({
        "type": "message_delta",
        "delta": {"stop_reason": stop_reason},
        "usage": {"output_tokens": output_tokens}
    });
    output.push_str("event: message_delta\ndata: ");
    output.push_str(&serde_json::to_string(&event).map_err(ProviderError::Serialisation)?);
    output.push_str("\n\n");

    // message_stop
    let event = serde_json::json!({"type": "message_stop"});
    output.push_str("event: message_stop\ndata: ");
    output.push_str(&serde_json::to_string(&event).map_err(ProviderError::Serialisation)?);
    output.push_str("\n\n");

    Ok(output)
}

// ── Body sanitisation ─────────────────────────────────────────────────────────

/// Recursively strip fields from `cache_control` objects that the upstream API
/// does not accept (e.g. `scope`).  Claude Code may send:
///
/// ```json
/// {"cache_control": {"type": "ephemeral", "scope": "turn"}}
/// ```
///
/// Copilot's `/v1/messages` endpoint rejects the extra `scope` key.  We keep
/// only `type` inside every `cache_control` object we encounter anywhere in the
/// JSON tree.
fn strip_cache_control_extras(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(cc) = map.get_mut("cache_control") {
                if let Some(cc_obj) = cc.as_object_mut() {
                    cc_obj.retain(|k, _| k == "type");
                }
            }
            for v in map.values_mut() {
                strip_cache_control_extras(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                strip_cache_control_extras(v);
            }
        }
        _ => {}
    }
}

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

    fn capabilities(&self) -> &'static [super::Capability] {
        use super::Capability::*;
        &[ChatCompletions, Streaming, Responses, MessagesApi, Tools]
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
        Ok(super::parse_sse_stream(byte_stream))
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

    /// Forward an Anthropic Messages API request to GitHub Copilot.
    ///
    /// **Claude models** (name starts with `claude`) are forwarded directly to
    /// Copilot's native `/v1/messages` endpoint — no protocol translation needed.
    ///
    /// **Non-Claude models** are translated to OpenAI ChatCompletions format via
    /// [`anthropic_to_openai`] and sent to `/chat/completions`, with the response
    /// translated back to Anthropic format.
    async fn proxy_messages(
        &self,
        body: serde_json::Value,
        is_stream: bool,
        _client_betas: Option<String>,
    ) -> Result<reqwest::Response, ProviderError> {
        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("gpt-4o")
            .to_string();

        let (api_token, endpoint) = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(format!("Copilot auth failed: {e}")))?;

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ProviderError::Internal(format!("Failed to build HTTP client: {e}")))?;

        // Claude models → native Anthropic passthrough to /v1/messages
        if model.starts_with("claude") {
            let mut body = body;
            strip_cache_control_extras(&mut body);

            let url = format!("{}/v1/messages", endpoint);
            debug!(model = %model, url = %url, stream = is_stream, "Copilot messages proxy: native passthrough");

            let response = client
                .post(&url)
                .bearer_auth(&api_token)
                .header("User-Agent", USER_AGENT)
                .header("Editor-Version", EDITOR_VERSION)
                .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
                .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
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
                    .unwrap_or("application/json")
                    .to_string();
                let body_bytes = response
                    .bytes()
                    .await
                    .map_err(|e| ProviderError::Internal(e.to_string()))?;
                let rebuilt = http::Response::builder()
                    .status(status.as_u16())
                    .header("content-type", ct)
                    .body(body_bytes)
                    .map_err(|e| ProviderError::Internal(e.to_string()))?;
                return Ok(reqwest::Response::from(rebuilt));
            }

            // Streaming: pass through the SSE stream directly.
            return Ok(response);
        }

        // Non-Claude models → translate Anthropic → OpenAI → Anthropic
        let openai_body = anthropic_to_openai(&body);
        debug!(model = %model, stream = is_stream, "Copilot messages proxy: translating Anthropic → OpenAI");

        let url = format!("{}/chat/completions", endpoint);
        let upstream = client
            .post(&url)
            .bearer_auth(&api_token)
            .header("User-Agent", USER_AGENT)
            .header("Editor-Version", EDITOR_VERSION)
            .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
            .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
            .json(&openai_body)
            .send()
            .await?;

        let status = upstream.status().as_u16();
        if !upstream.status().is_success() {
            let body_text = upstream.text().await.unwrap_or_default();
            error!(
                status = status,
                "GitHub Copilot messages proxy error response"
            );
            return Err(ProviderError::ProviderResponse {
                status,
                message: body_text,
            });
        }

        let http_resp = if is_stream {
            let sse_text = upstream.text().await?;
            let anthropic_sse = openai_sse_to_anthropic(&sse_text, &model)?;
            http::Response::builder()
                .status(200u16)
                .header("content-type", "text/event-stream; charset=utf-8")
                .body(Bytes::from(anthropic_sse))
                .map_err(|e| ProviderError::Internal(e.to_string()))?
        } else {
            let openai_resp: serde_json::Value = upstream.json().await?;
            let anthropic_resp = openai_to_anthropic_response(&openai_resp, &model);
            let json =
                serde_json::to_string(&anthropic_resp).map_err(ProviderError::Serialisation)?;
            http::Response::builder()
                .status(200u16)
                .header("content-type", "application/json")
                .body(Bytes::from(json))
                .map_err(|e| ProviderError::Internal(e.to_string()))?
        };

        Ok(reqwest::Response::from(http_resp))
    }

    /// Forward an OpenAI Responses API request to GitHub Copilot.
    ///
    /// **Claude models** (name starts with `claude`) are protocol-bridged:
    /// the Responses API request is converted to Messages API format, sent to
    /// Copilot's `/v1/messages` endpoint, and the response is converted back
    /// to Responses API format.
    ///
    /// **Non-Claude models** are sent as-is to Copilot's `/v1/responses` endpoint.
    async fn proxy_responses(
        &self,
        body: serde_json::Value,
        is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let (api_token, endpoint) = self
            .token_manager
            .get_token()
            .await
            .map_err(|e| ProviderError::Internal(format!("Copilot auth failed: {e}")))?;

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ProviderError::Internal(format!("Failed to build HTTP client: {e}")))?;

        // Claude models → convert Responses→Messages, send to /v1/messages,
        // convert the response back to Responses format.
        if model.starts_with("claude") {
            use crate::transform;

            let messages_body = transform::responses_to_messages_request(&body);
            let url = format!("{}/v1/messages", endpoint);
            debug!(model = %model, url = %url, stream = is_stream, "Copilot responses proxy: bridging Responses→Messages for Claude");

            let upstream = client
                .post(&url)
                .bearer_auth(&api_token)
                .header("User-Agent", USER_AGENT)
                .header("Editor-Version", EDITOR_VERSION)
                .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
                .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
                .header("content-type", "application/json")
                .json(&messages_body)
                .send()
                .await?;

            let status = upstream.status();
            if !status.is_success() {
                let status_code = status.as_u16();
                let msg = upstream.text().await.unwrap_or_default();
                error!(status = status_code, body = %msg, "github-copilot responses→messages bridge error");
                return Err(ProviderError::ProviderResponse {
                    status: status_code,
                    message: msg,
                });
            }

            let http_resp = if is_stream {
                // True streaming: convert Messages SSE → Responses SSE event-by-event.
                let byte_stream = upstream.bytes_stream();
                let converted_stream =
                    transform::create_responses_sse_stream_from_messages(byte_stream);
                let body = reqwest::Body::wrap_stream(converted_stream);
                http::Response::builder()
                    .status(200u16)
                    .header("content-type", "text/event-stream; charset=utf-8")
                    .body(body)
                    .map_err(|e| ProviderError::Internal(e.to_string()))?
            } else {
                // Non-streaming: parse Messages response, convert to Responses format.
                let messages_resp: serde_json::Value = upstream.json().await?;
                let responses_resp = transform::messages_to_responses_response(&messages_resp);
                let json =
                    serde_json::to_string(&responses_resp).map_err(ProviderError::Serialisation)?;
                http::Response::builder()
                    .status(200u16)
                    .header("content-type", "application/json")
                    .body(reqwest::Body::from(json))
                    .map_err(|e| ProviderError::Internal(e.to_string()))?
            };

            return Ok(reqwest::Response::from(http_resp));
        }

        // Non-Claude models → passthrough to /v1/responses
        let url = format!("{}/v1/responses", endpoint);
        debug!(url = %url, "Sending passthrough Responses API request via github-copilot");

        let response = client
            .post(&url)
            .bearer_auth(&api_token)
            .header("User-Agent", USER_AGENT)
            .header("Editor-Version", EDITOR_VERSION)
            .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
            .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let msg = response.text().await.unwrap_or_default();
            error!(status = status_code, body = %msg, "github-copilot responses proxy error");
            return Err(ProviderError::ProviderResponse {
                status: status_code,
                message: msg,
            });
        }

        Ok(response)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::copilot::CopilotTokenManager;
    use serde_json::json;

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

    // ── Request translation tests ──────────────────────────────────────────────

    #[test]
    fn test_anthropic_to_openai_string_system() {
        let body = json!({
            "model": "gpt-4o",
            "system": "You are helpful",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 100,
            "stream": false
        });
        let result = anthropic_to_openai(&body);

        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are helpful");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(result["max_tokens"], 100);
        assert_eq!(result["stream"], false);
    }

    #[test]
    fn test_anthropic_to_openai_array_system() {
        let body = json!({
            "model": "gpt-4o",
            "system": [
                {"type": "text", "text": "You are"},
                {"type": "text", "text": "helpful"}
            ],
            "messages": [{"role": "user", "content": "hello"}]
        });
        let result = anthropic_to_openai(&body);

        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are\nhelpful");
    }

    #[test]
    fn test_anthropic_to_openai_no_system() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let result = anthropic_to_openai(&body);

        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn test_anthropic_to_openai_stop_sequences() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [],
            "stop_sequences": ["END", "STOP"]
        });
        let result = anthropic_to_openai(&body);
        assert_eq!(result["stop"], json!(["END", "STOP"]));
    }

    #[test]
    fn test_anthropic_to_openai_strips_anthropic_fields() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [],
            "anthropic_version": "2023-06-01",
            "metadata": {"user_id": "abc"},
            "tool_choice": {"type": "auto"},
            "tools": []
        });
        let result = anthropic_to_openai(&body);
        assert!(result.get("anthropic_version").is_none());
        assert!(result.get("metadata").is_none());
        assert!(result.get("tool_choice").is_none());
        assert!(result.get("tools").is_none());
    }

    // ── Non-streaming response translation tests ───────────────────────────────

    #[test]
    fn test_openai_to_anthropic_response_basic() {
        let openai = json!({
            "id": "chatcmpl-abc123",
            "choices": [{"message": {"role": "assistant", "content": "Hi!"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        let result = openai_to_anthropic_response(&openai, "gpt-4o");

        assert!(result["id"].as_str().unwrap().starts_with("msg_"));
        assert_eq!(result["type"], "message");
        assert_eq!(result["role"], "assistant");
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "Hi!");
        assert_eq!(result["model"], "gpt-4o");
        assert_eq!(result["stop_reason"], "end_turn");
        assert_eq!(result["usage"]["input_tokens"], 10);
        assert_eq!(result["usage"]["output_tokens"], 5);
    }

    #[test]
    fn test_finish_reason_length_maps_to_max_tokens() {
        let openai = json!({
            "id": "chatcmpl-xyz",
            "choices": [{"message": {"role": "assistant", "content": "..."}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 100}
        });
        let result = openai_to_anthropic_response(&openai, "gpt-4o");
        assert_eq!(result["stop_reason"], "max_tokens");
    }

    #[test]
    fn test_finish_reason_stop_maps_to_end_turn() {
        let openai = json!({
            "id": "chatcmpl-xyz",
            "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2}
        });
        let result = openai_to_anthropic_response(&openai, "model");
        assert_eq!(result["stop_reason"], "end_turn");
    }

    #[test]
    fn test_response_id_prefixed_with_msg() {
        let openai = json!({
            "id": "chatcmpl-hello",
            "choices": [{"message": {"role": "assistant", "content": ""}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        });
        let result = openai_to_anthropic_response(&openai, "m");
        let id = result["id"].as_str().unwrap();
        assert!(id.starts_with("msg_"), "id={id}");
        assert!(id.contains("hello"), "id={id}");
    }

    // ── Streaming translation tests ────────────────────────────────────────────

    #[test]
    fn test_openai_sse_to_anthropic_basic() {
        let sse = concat!(
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"!\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n",
            "data: [DONE]\n",
        );

        let result = openai_sse_to_anthropic(sse, "gpt-4o").unwrap();

        assert!(result.contains("event: message_start\n"));
        assert!(result.contains("event: content_block_start\n"));
        assert!(result.contains("\"text\":\"Hi\""));
        assert!(result.contains("\"text\":\"!\""));
        assert!(result.contains("event: content_block_stop\n"));
        assert!(result.contains("\"stop_reason\":\"end_turn\""));
        assert!(result.contains("\"output_tokens\":2"));
        assert!(result.contains("event: message_delta\n"));
        assert!(result.contains("event: message_stop\n"));
    }

    #[test]
    fn test_openai_sse_to_anthropic_length_finish_reason() {
        let sse = concat!(
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"length\"}]}\n",
            "data: [DONE]\n",
        );
        let result = openai_sse_to_anthropic(sse, "gpt-4o").unwrap();
        assert!(result.contains("\"stop_reason\":\"max_tokens\""));
    }

    #[test]
    fn test_openai_sse_message_id_prefixed() {
        let sse = "data: {\"id\":\"chatcmpl-abc\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"index\":0,\"finish_reason\":null}]}\ndata: [DONE]\n";
        let result = openai_sse_to_anthropic(sse, "gpt-4o").unwrap();
        // The message_start event should have an id that starts with msg_
        assert!(result.contains("\"id\":\"msg_"));
    }

    // ── Cache control stripping tests ────────────────────────────────────────

    #[test]
    fn test_strip_cache_control_extras_removes_scope() {
        let mut body = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "hello",
                            "cache_control": {"type": "ephemeral", "scope": "turn"}
                        }
                    ]
                }
            ],
            "system": [
                {
                    "type": "text",
                    "text": "You are helpful",
                    "cache_control": {"type": "ephemeral", "scope": "turn"}
                }
            ]
        });
        strip_cache_control_extras(&mut body);

        // scope should be gone, type should remain
        let cc = &body["messages"][0]["content"][0]["cache_control"];
        assert_eq!(cc, &json!({"type": "ephemeral"}));

        let sys_cc = &body["system"][0]["cache_control"];
        assert_eq!(sys_cc, &json!({"type": "ephemeral"}));
    }

    #[test]
    fn test_strip_cache_control_extras_no_op_without_scope() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "hello"}]
        });
        let original = body.clone();
        strip_cache_control_extras(&mut body);
        assert_eq!(body, original);
    }

    #[test]
    fn test_openai_sse_content_without_role_chunk() {
        // Some models skip the role-only first chunk and go straight to content.
        let sse = concat!(
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}]}\n",
            "data: [DONE]\n",
        );
        let result = openai_sse_to_anthropic(sse, "gpt-4o").unwrap();
        assert!(result.contains("event: content_block_start\n"));
        assert!(result.contains("\"text\":\"Hello\""));
        assert!(result.contains("event: content_block_stop\n"));
    }
}
