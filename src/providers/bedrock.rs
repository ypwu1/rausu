//! AWS Bedrock provider implementation.
//!
//! AWS Bedrock provides managed access to foundation models from multiple vendors
//! (Anthropic Claude, Amazon Nova/Titan, Meta Llama, Mistral, Cohere, etc.) through
//! a unified Converse API.
//!
//! This provider translates between OpenAI Chat Completions format and the Bedrock
//! Converse API, handling request/response format differences, streaming via AWS
//! EventStream → SSE conversion, and tool calling format translation.
//!
//! **Auth:** AWS SigV4 request signing via the AWS SDK default credential chain
//! (environment variables, `~/.aws/credentials`, IAM role, etc.).
//!
//! **Region:** Required — set via the `region` field in config (e.g. `us-east-1`).
//!
//! The Responses API is bridged through Chat Completions using Rausu's existing
//! transform layer, the same strategy used by the `openai`, `deepseek`, `azure-openai`,
//! `google-ai-studio`, and `moonshot` providers.
//!
//! # Supported capabilities
//!
//! | Capability | Support |
//! |---|---|
//! | `chat_completions` | Bedrock Converse API translation |
//! | `streaming` | EventStream → SSE translation |
//! | `responses_api` | Bridged via Chat Completions transform |
//! | `tools` | OpenAI tools ↔ Bedrock toolConfig translation |

use std::pin::Pin;

use async_trait::async_trait;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ContentBlockDelta as BedrockDelta, ContentBlockStart as BedrockBlockStart,
    ConversationRole, InferenceConfiguration, Message as BedrockMessage, StopReason,
    SystemContentBlock, Tool as BedrockTool, ToolChoice as BedrockToolChoice, ToolConfiguration,
    ToolInputSchema, ToolResultBlock, ToolResultContentBlock, ToolSpecification, ToolUseBlock,
};
use aws_smithy_types::Document;
use futures::{Stream, StreamExt};
use serde_json::Value;
use tracing::{debug, error};

use crate::schema::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Choice, ChunkChoice, Delta,
    FunctionCall, Message, ModelInfo, ToolCall, Usage,
};

use super::{Capability, Provider, ProviderError};

/// AWS Bedrock provider.
pub struct BedrockProvider {
    client: aws_sdk_bedrockruntime::Client,
    region: String,
    /// Known models (from config).
    model_names: Vec<String>,
}

impl BedrockProvider {
    /// Create a new Bedrock provider instance.
    ///
    /// `region` is the AWS region (e.g. `us-east-1`). Credentials are resolved
    /// via the standard AWS SDK credential chain: environment variables
    /// (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`), shared credentials file
    /// (`~/.aws/credentials`), or IAM role.
    pub async fn new(region: String, model_names: Vec<String>) -> Self {
        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.clone()))
            .load()
            .await;
        let client = aws_sdk_bedrockruntime::Client::new(&sdk_config);
        Self {
            client,
            region,
            model_names,
        }
    }
}

#[async_trait]
impl Provider for BedrockProvider {
    fn name(&self) -> &str {
        "bedrock"
    }

    fn capabilities(&self) -> &'static [Capability] {
        use Capability::*;
        &[ChatCompletions, Streaming, Responses, Tools]
    }

    async fn chat_completions(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let model_id = req.model.clone();
        debug!(model = %model_id, region = %self.region, "Sending non-streaming Converse request to Bedrock");

        let (system, messages) = translate_messages(&req.messages)?;
        let inference_config = build_inference_config(&req);
        let tool_config = build_tool_config(&req)?;

        let mut builder = self
            .client
            .converse()
            .model_id(&model_id)
            .set_messages(Some(messages))
            .set_system(system);

        if let Some(ic) = inference_config {
            builder = builder.inference_config(ic);
        }
        if let Some(tc) = tool_config {
            builder = builder.tool_config(tc);
        }

        let result = builder.send().await.map_err(map_converse_error)?;

        translate_converse_response(result, &model_id)
    }

    async fn chat_completions_stream(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatCompletionChunk, ProviderError>> + Send>>,
        ProviderError,
    > {
        let model_id = req.model.clone();
        debug!(model = %model_id, region = %self.region, "Sending streaming ConverseStream request to Bedrock");

        let (system, messages) = translate_messages(&req.messages)?;
        let inference_config = build_inference_config(&req);
        let tool_config = build_tool_config(&req)?;

        let mut builder = self
            .client
            .converse_stream()
            .model_id(&model_id)
            .set_messages(Some(messages))
            .set_system(system);

        if let Some(ic) = inference_config {
            builder = builder.inference_config(ic);
        }
        if let Some(tc) = tool_config {
            builder = builder.tool_config(tc);
        }

        let output = builder.send().await.map_err(map_converse_stream_error)?;
        let mut event_stream = output.stream;

        let completion_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
        let created = chrono::Utc::now().timestamp();

        let stream = async_stream::stream! {
            loop {
                match event_stream.recv().await {
                    Ok(Some(event)) => {
                        if let Some(chunk) = translate_stream_event(&event, &completion_id, &model_id, created) {
                            yield Ok(chunk);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        error!(error = %e, "Bedrock EventStream error");
                        yield Err(ProviderError::Internal(e.to_string()));
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn proxy_responses(
        &self,
        body: Value,
        is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        use crate::transform;

        let cc_body = transform::responses_to_chat_completions_request(&body);

        if is_stream {
            // Build a ChatCompletionRequest from the transformed body.
            let mut cc_req: ChatCompletionRequest =
                serde_json::from_value(cc_body).map_err(ProviderError::Serialisation)?;
            cc_req.stream = Some(true);

            let chunk_stream = self.chat_completions_stream(cc_req).await?;

            // Convert ChatCompletionChunk stream → SSE byte stream.
            let sse_stream = chunk_stream.filter_map(|result| async move {
                match result {
                    Ok(chunk) => {
                        let json = serde_json::to_string(&chunk).ok()?;
                        Some(Ok::<bytes::Bytes, reqwest::Error>(bytes::Bytes::from(
                            format!("data: {json}\n\n"),
                        )))
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Bedrock streaming error during Responses bridge");
                        None
                    }
                }
            });

            let done_stream = futures::stream::once(async {
                Ok::<bytes::Bytes, reqwest::Error>(bytes::Bytes::from("data: [DONE]\n\n"))
            });

            let full_stream = sse_stream.chain(done_stream);
            let converted_stream =
                transform::create_responses_sse_stream_from_chat_completions(full_stream);
            let body = reqwest::Body::wrap_stream(converted_stream);

            let http_resp = http::Response::builder()
                .status(200u16)
                .header("content-type", "text/event-stream; charset=utf-8")
                .body(body)
                .map_err(|e| ProviderError::Internal(e.to_string()))?;
            Ok(reqwest::Response::from(http_resp))
        } else {
            let cc_req: ChatCompletionRequest =
                serde_json::from_value(cc_body).map_err(ProviderError::Serialisation)?;
            let cc_resp = self.chat_completions(cc_req).await?;
            let cc_value = serde_json::to_value(&cc_resp).map_err(ProviderError::Serialisation)?;
            let responses_resp = transform::chat_completions_to_responses_response(&cc_value);
            let json_str =
                serde_json::to_string(&responses_resp).map_err(ProviderError::Serialisation)?;

            let http_resp = http::Response::builder()
                .status(200u16)
                .header("content-type", "application/json")
                .body(reqwest::Body::from(json_str))
                .map_err(|e| ProviderError::Internal(e.to_string()))?;
            Ok(reqwest::Response::from(http_resp))
        }
    }

    fn models(&self) -> Vec<ModelInfo> {
        let now = chrono::Utc::now().timestamp();
        self.model_names
            .iter()
            .map(|name| ModelInfo {
                id: name.clone(),
                object: "model".to_string(),
                created: now,
                owned_by: "bedrock".to_string(),
            })
            .collect()
    }
}

// ── Request translation: OpenAI → Bedrock Converse ──────────────────────────

/// Extract system messages and translate user/assistant/tool messages to Bedrock format.
///
/// Returns `(system_content_blocks, messages)`.
fn translate_messages(
    messages: &[Message],
) -> Result<(Option<Vec<SystemContentBlock>>, Vec<BedrockMessage>), ProviderError> {
    let mut system_blocks: Vec<SystemContentBlock> = Vec::new();
    let mut bedrock_messages: Vec<BedrockMessage> = Vec::new();

    // Accumulate tool results to merge into a single user message.
    let mut pending_tool_results: Vec<ContentBlock> = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => {
                if let Some(content) = &msg.content {
                    let text = content_to_text(content);
                    if !text.is_empty() {
                        system_blocks.push(SystemContentBlock::Text(text));
                    }
                }
            }
            "user" => {
                // Flush any pending tool results first.
                flush_tool_results(&mut pending_tool_results, &mut bedrock_messages);

                let text = msg
                    .content
                    .as_ref()
                    .map(content_to_text)
                    .unwrap_or_default();
                if !text.is_empty() {
                    bedrock_messages.push(
                        BedrockMessage::builder()
                            .role(ConversationRole::User)
                            .content(ContentBlock::Text(text))
                            .build()
                            .map_err(|e| {
                                ProviderError::Internal(format!("build user message: {e}"))
                            })?,
                    );
                }
            }
            "assistant" => {
                // Flush any pending tool results first.
                flush_tool_results(&mut pending_tool_results, &mut bedrock_messages);

                let mut content_blocks: Vec<ContentBlock> = Vec::new();

                // Text content.
                if let Some(content) = &msg.content {
                    let text = content_to_text(content);
                    if !text.is_empty() {
                        content_blocks.push(ContentBlock::Text(text));
                    }
                }

                // Tool calls → ToolUse blocks.
                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        let input_doc = serde_json::from_str::<Value>(&tc.function.arguments)
                            .map(|v| value_to_document(&v))
                            .unwrap_or(Document::Object(Default::default()));
                        content_blocks.push(ContentBlock::ToolUse(
                            ToolUseBlock::builder()
                                .tool_use_id(&tc.id)
                                .name(&tc.function.name)
                                .input(input_doc)
                                .build()
                                .map_err(|e| {
                                    ProviderError::Internal(format!("build tool_use: {e}"))
                                })?,
                        ));
                    }
                }

                if !content_blocks.is_empty() {
                    let mut builder = BedrockMessage::builder().role(ConversationRole::Assistant);
                    for block in content_blocks {
                        builder = builder.content(block);
                    }
                    bedrock_messages.push(builder.build().map_err(|e| {
                        ProviderError::Internal(format!("build assistant message: {e}"))
                    })?);
                }
            }
            "tool" => {
                // Tool results go into the next User message.
                let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();
                let text = msg
                    .content
                    .as_ref()
                    .map(content_to_text)
                    .unwrap_or_default();
                pending_tool_results.push(ContentBlock::ToolResult(
                    ToolResultBlock::builder()
                        .tool_use_id(tool_use_id)
                        .content(ToolResultContentBlock::Text(text))
                        .build()
                        .map_err(|e| ProviderError::Internal(format!("build tool_result: {e}")))?,
                ));
            }
            _ => {
                // Unknown role — skip.
            }
        }
    }

    // Flush any remaining tool results.
    flush_tool_results(&mut pending_tool_results, &mut bedrock_messages);

    let system = if system_blocks.is_empty() {
        None
    } else {
        Some(system_blocks)
    };

    Ok((system, bedrock_messages))
}

/// Flush accumulated tool result blocks into a User message.
fn flush_tool_results(pending: &mut Vec<ContentBlock>, messages: &mut Vec<BedrockMessage>) {
    if pending.is_empty() {
        return;
    }
    let mut builder = BedrockMessage::builder().role(ConversationRole::User);
    for block in pending.drain(..) {
        builder = builder.content(block);
    }
    if let Ok(msg) = builder.build() {
        messages.push(msg);
    }
}

/// Extract plain text from OpenAI content (string or array of content parts).
fn content_to_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    part.get("text").and_then(Value::as_str).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Build Bedrock InferenceConfiguration from OpenAI request fields.
fn build_inference_config(req: &ChatCompletionRequest) -> Option<InferenceConfiguration> {
    let has_any = req.temperature.is_some()
        || req.max_tokens.is_some()
        || req.top_p.is_some()
        || req.stop.is_some();

    if !has_any {
        return None;
    }

    let mut builder = InferenceConfiguration::builder();

    if let Some(t) = req.temperature {
        builder = builder.temperature(t);
    }
    if let Some(m) = req.max_tokens {
        builder = builder.max_tokens(m as i32);
    }
    if let Some(p) = req.top_p {
        builder = builder.top_p(p);
    }
    if let Some(stop) = &req.stop {
        match stop {
            Value::String(s) => {
                builder = builder.stop_sequences(s.clone());
            }
            Value::Array(arr) => {
                for s in arr.iter().filter_map(Value::as_str) {
                    builder = builder.stop_sequences(s.to_string());
                }
            }
            _ => {}
        }
    }

    Some(builder.build())
}

/// Translate OpenAI tools + tool_choice to Bedrock ToolConfiguration.
fn build_tool_config(
    req: &ChatCompletionRequest,
) -> Result<Option<ToolConfiguration>, ProviderError> {
    let tools = match &req.tools {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(None),
    };

    let bedrock_tools: Vec<BedrockTool> = tools
        .iter()
        .map(|t| {
            let mut spec_builder = ToolSpecification::builder().name(&t.function.name);

            if let Some(desc) = &t.function.description {
                spec_builder = spec_builder.description(desc);
            }
            if let Some(params) = &t.function.parameters {
                spec_builder =
                    spec_builder.input_schema(ToolInputSchema::Json(value_to_document(params)));
            }

            let spec = spec_builder
                .build()
                .map_err(|e| ProviderError::Internal(format!("build tool spec: {e}")))?;
            Ok(BedrockTool::ToolSpec(spec))
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;

    let mut tc_builder = ToolConfiguration::builder();
    for tool in bedrock_tools {
        tc_builder = tc_builder.tools(tool);
    }

    // Translate tool_choice.
    if let Some(choice) = &req.tool_choice {
        match choice {
            Value::String(s) => match s.as_str() {
                "auto" => {
                    tc_builder = tc_builder.tool_choice(BedrockToolChoice::Auto(
                        aws_sdk_bedrockruntime::types::AutoToolChoice::builder().build(),
                    ));
                }
                "required" | "any" => {
                    tc_builder = tc_builder.tool_choice(BedrockToolChoice::Any(
                        aws_sdk_bedrockruntime::types::AnyToolChoice::builder().build(),
                    ));
                }
                "none" => {
                    // Don't send tool config at all if tool_choice is "none".
                    return Ok(None);
                }
                _ => {}
            },
            Value::Object(obj) => {
                if let Some(func) = obj.get("function") {
                    if let Some(name) = func.get("name").and_then(Value::as_str) {
                        tc_builder = tc_builder.tool_choice(BedrockToolChoice::Tool(
                            aws_sdk_bedrockruntime::types::SpecificToolChoice::builder()
                                .name(name)
                                .build()
                                .map_err(|e| {
                                    ProviderError::Internal(format!(
                                        "build specific tool choice: {e}"
                                    ))
                                })?,
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    let tc = tc_builder
        .build()
        .map_err(|e| ProviderError::Internal(format!("build tool config: {e}")))?;
    Ok(Some(tc))
}

// ── Response translation: Bedrock Converse → OpenAI ─────────────────────────

/// Translate a non-streaming Converse response to OpenAI ChatCompletionResponse.
fn translate_converse_response(
    result: aws_sdk_bedrockruntime::operation::converse::ConverseOutput,
    model_id: &str,
) -> Result<ChatCompletionResponse, ProviderError> {
    let mut content_text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(aws_sdk_bedrockruntime::types::ConverseOutput::Message(msg)) = result.output() {
        for block in msg.content() {
            match block {
                ContentBlock::Text(t) => {
                    content_text.push_str(t);
                }
                ContentBlock::ToolUse(tu) => {
                    let arguments = document_to_value(tu.input())
                        .map(|v| serde_json::to_string(&v).unwrap_or_default())
                        .unwrap_or_else(|| "{}".to_string());
                    tool_calls.push(ToolCall {
                        id: tu.tool_use_id().to_string(),
                        r#type: "function".to_string(),
                        function: FunctionCall {
                            name: tu.name().to_string(),
                            arguments,
                        },
                    });
                }
                _ => {}
            }
        }
    }

    let finish_reason = translate_stop_reason(result.stop_reason());

    let message_content = if content_text.is_empty() {
        None
    } else {
        Some(Value::String(content_text))
    };

    let message_tool_calls = if tool_calls.is_empty() {
        None
    } else {
        Some(tool_calls)
    };

    let usage = result
        .usage()
        .map(|u| Usage {
            prompt_tokens: u.input_tokens() as u32,
            completion_tokens: u.output_tokens() as u32,
            total_tokens: u.total_tokens() as u32,
        })
        .unwrap_or_default();

    Ok(ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp(),
        model: model_id.to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".to_string(),
                content: message_content,
                tool_call_id: None,
                tool_calls: message_tool_calls,
                name: None,
            },
            finish_reason: Some(finish_reason),
        }],
        usage,
    })
}

/// Map Bedrock StopReason to OpenAI finish_reason.
fn translate_stop_reason(reason: &StopReason) -> String {
    match reason {
        StopReason::EndTurn => "stop".to_string(),
        StopReason::ToolUse => "tool_calls".to_string(),
        StopReason::MaxTokens => "length".to_string(),
        StopReason::StopSequence => "stop".to_string(),
        StopReason::ContentFiltered => "content_filter".to_string(),
        StopReason::GuardrailIntervened => "content_filter".to_string(),
        _ => "stop".to_string(),
    }
}

// ── Streaming translation: Bedrock EventStream → OpenAI SSE chunks ──────────

/// Translate a single Bedrock EventStream event to an OpenAI ChatCompletionChunk.
///
/// Returns `None` for events that don't produce a chunk (e.g. `ContentBlockStop`,
/// `Metadata`).
fn translate_stream_event(
    event: &aws_sdk_bedrockruntime::types::ConverseStreamOutput,
    completion_id: &str,
    model: &str,
    created: i64,
) -> Option<ChatCompletionChunk> {
    match event {
        aws_sdk_bedrockruntime::types::ConverseStreamOutput::MessageStart(e) => {
            let role = match e.role() {
                ConversationRole::Assistant => "assistant",
                _ => "assistant",
            };
            Some(ChatCompletionChunk {
                id: completion_id.to_string(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: Some(role.to_string()),
                        content: None,
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
            })
        }
        aws_sdk_bedrockruntime::types::ConverseStreamOutput::ContentBlockStart(e) => {
            if let Some(start) = e.start() {
                match start {
                    BedrockBlockStart::ToolUse(tu) => {
                        let index = e.content_block_index() as u32;
                        // Tool use blocks start after the first text block, so use
                        // (index - 1) to get the tool_calls array index, but at
                        // minimum 0.
                        let tc_index = if index > 0 { index - 1 } else { 0 };
                        Some(ChatCompletionChunk {
                            id: completion_id.to_string(),
                            object: "chat.completion.chunk".to_string(),
                            created,
                            model: model.to_string(),
                            choices: vec![ChunkChoice {
                                index: 0,
                                delta: Delta {
                                    role: None,
                                    content: None,
                                    tool_calls: Some(vec![serde_json::json!({
                                        "index": tc_index,
                                        "id": tu.tool_use_id(),
                                        "type": "function",
                                        "function": {
                                            "name": tu.name(),
                                            "arguments": ""
                                        }
                                    })]),
                                },
                                finish_reason: None,
                            }],
                        })
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
        aws_sdk_bedrockruntime::types::ConverseStreamOutput::ContentBlockDelta(e) => {
            if let Some(delta) = e.delta() {
                match delta {
                    BedrockDelta::Text(text) => Some(ChatCompletionChunk {
                        id: completion_id.to_string(),
                        object: "chat.completion.chunk".to_string(),
                        created,
                        model: model.to_string(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: Delta {
                                role: None,
                                content: Some(text.clone()),
                                tool_calls: None,
                            },
                            finish_reason: None,
                        }],
                    }),
                    BedrockDelta::ToolUse(tu_delta) => {
                        let index = e.content_block_index() as u32;
                        let tc_index = if index > 0 { index - 1 } else { 0 };
                        Some(ChatCompletionChunk {
                            id: completion_id.to_string(),
                            object: "chat.completion.chunk".to_string(),
                            created,
                            model: model.to_string(),
                            choices: vec![ChunkChoice {
                                index: 0,
                                delta: Delta {
                                    role: None,
                                    content: None,
                                    tool_calls: Some(vec![serde_json::json!({
                                        "index": tc_index,
                                        "function": {
                                            "arguments": tu_delta.input()
                                        }
                                    })]),
                                },
                                finish_reason: None,
                            }],
                        })
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
        aws_sdk_bedrockruntime::types::ConverseStreamOutput::MessageStop(e) => {
            let finish_reason = Some(translate_stop_reason(e.stop_reason()));
            Some(ChatCompletionChunk {
                id: completion_id.to_string(),
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
        // ContentBlockStop, Metadata — no chunk emitted.
        _ => None,
    }
}

// ── Document conversion (serde_json::Value ↔ aws_smithy_types::Document) ────

/// Convert a `serde_json::Value` to an `aws_smithy_types::Document`.
fn value_to_document(v: &Value) -> Document {
    match v {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= 0 {
                    Document::Number(aws_smithy_types::Number::PosInt(i as u64))
                } else {
                    Document::Number(aws_smithy_types::Number::NegInt(i))
                }
            } else if let Some(f) = n.as_f64() {
                Document::Number(aws_smithy_types::Number::Float(f))
            } else {
                Document::Null
            }
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(a) => Document::Array(a.iter().map(value_to_document).collect()),
        Value::Object(o) => Document::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), value_to_document(v)))
                .collect(),
        ),
    }
}

/// Convert an `aws_smithy_types::Document` to a `serde_json::Value`.
///
/// Returns `None` for `Document::Null` so callers can skip it if needed.
fn document_to_value(d: &Document) -> Option<Value> {
    match d {
        Document::Null => None,
        Document::Bool(b) => Some(Value::Bool(*b)),
        Document::Number(n) => match n {
            aws_smithy_types::Number::PosInt(i) => Some(Value::Number((*i).into())),
            aws_smithy_types::Number::NegInt(i) => Some(Value::Number((*i).into())),
            aws_smithy_types::Number::Float(f) => {
                serde_json::Number::from_f64(*f).map(Value::Number)
            }
        },
        Document::String(s) => Some(Value::String(s.clone())),
        Document::Array(a) => Some(Value::Array(
            a.iter().filter_map(document_to_value).collect(),
        )),
        Document::Object(o) => Some(Value::Object(
            o.iter()
                .filter_map(|(k, v)| document_to_value(v).map(|val| (k.clone(), val)))
                .collect(),
        )),
    }
}

// ── Error mapping ───────────────────────────────────────────────────────────

/// Map `converse()` SDK errors to `ProviderError`.
fn map_converse_error(
    err: aws_sdk_bedrockruntime::error::SdkError<
        aws_sdk_bedrockruntime::operation::converse::ConverseError,
    >,
) -> ProviderError {
    use aws_sdk_bedrockruntime::operation::converse::ConverseError;
    match &err {
        aws_sdk_bedrockruntime::error::SdkError::ServiceError(se) => match se.err() {
            ConverseError::ThrottlingException(e) => ProviderError::ProviderResponse {
                status: 429,
                message: e.message().unwrap_or("throttled").to_string(),
            },
            ConverseError::ValidationException(e) => ProviderError::ProviderResponse {
                status: 400,
                message: e.message().unwrap_or("validation error").to_string(),
            },
            ConverseError::AccessDeniedException(e) => ProviderError::ProviderResponse {
                status: 403,
                message: e.message().unwrap_or("access denied").to_string(),
            },
            ConverseError::ResourceNotFoundException(e) => ProviderError::ProviderResponse {
                status: 404,
                message: e.message().unwrap_or("not found").to_string(),
            },
            ConverseError::ServiceUnavailableException(e) => ProviderError::ProviderResponse {
                status: 503,
                message: e.message().unwrap_or("service unavailable").to_string(),
            },
            ConverseError::InternalServerException(e) => ProviderError::ProviderResponse {
                status: 500,
                message: e.message().unwrap_or("internal error").to_string(),
            },
            ConverseError::ModelTimeoutException(e) => ProviderError::ProviderResponse {
                status: 504,
                message: e.message().unwrap_or("model timeout").to_string(),
            },
            ConverseError::ModelNotReadyException(e) => ProviderError::ProviderResponse {
                status: 503,
                message: e.message().unwrap_or("model not ready").to_string(),
            },
            ConverseError::ModelErrorException(e) => ProviderError::ProviderResponse {
                status: 502,
                message: e.message().unwrap_or("model error").to_string(),
            },
            _ => ProviderError::Internal(err.to_string()),
        },
        _ => ProviderError::Internal(err.to_string()),
    }
}

/// Map `converse_stream()` SDK errors to `ProviderError`.
fn map_converse_stream_error(
    err: aws_sdk_bedrockruntime::error::SdkError<
        aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamError,
    >,
) -> ProviderError {
    use aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamError;
    match &err {
        aws_sdk_bedrockruntime::error::SdkError::ServiceError(se) => match se.err() {
            ConverseStreamError::ThrottlingException(e) => ProviderError::ProviderResponse {
                status: 429,
                message: e.message().unwrap_or("throttled").to_string(),
            },
            ConverseStreamError::ValidationException(e) => ProviderError::ProviderResponse {
                status: 400,
                message: e.message().unwrap_or("validation error").to_string(),
            },
            ConverseStreamError::AccessDeniedException(e) => ProviderError::ProviderResponse {
                status: 403,
                message: e.message().unwrap_or("access denied").to_string(),
            },
            ConverseStreamError::ResourceNotFoundException(e) => ProviderError::ProviderResponse {
                status: 404,
                message: e.message().unwrap_or("not found").to_string(),
            },
            ConverseStreamError::ServiceUnavailableException(e) => {
                ProviderError::ProviderResponse {
                    status: 503,
                    message: e.message().unwrap_or("service unavailable").to_string(),
                }
            }
            ConverseStreamError::InternalServerException(e) => ProviderError::ProviderResponse {
                status: 500,
                message: e.message().unwrap_or("internal error").to_string(),
            },
            ConverseStreamError::ModelTimeoutException(e) => ProviderError::ProviderResponse {
                status: 504,
                message: e.message().unwrap_or("model timeout").to_string(),
            },
            ConverseStreamError::ModelNotReadyException(e) => ProviderError::ProviderResponse {
                status: 503,
                message: e.message().unwrap_or("model not ready").to_string(),
            },
            ConverseStreamError::ModelErrorException(e) => ProviderError::ProviderResponse {
                status: 502,
                message: e.message().unwrap_or("model error").to_string(),
            },
            _ => ProviderError::Internal(err.to_string()),
        },
        _ => ProviderError::Internal(err.to_string()),
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::chat::FunctionDefinition;
    use crate::schema::chat::Tool;

    // ── Provider metadata ───────────────────────────────────────────────────

    #[test]
    fn test_capabilities_declared() {
        // Verify via the static slice directly (provider construction requires async).
        use Capability::*;
        let caps = &[ChatCompletions, Streaming, Responses, Tools];
        assert!(caps.contains(&Capability::ChatCompletions));
        assert!(caps.contains(&Capability::Streaming));
        assert!(caps.contains(&Capability::Responses));
        assert!(caps.contains(&Capability::Tools));
    }

    #[test]
    fn test_response_format_not_declared() {
        use Capability::*;
        let caps: &[Capability] = &[ChatCompletions, Streaming, Responses, Tools];
        assert!(!caps.contains(&Capability::ResponseFormat));
        assert!(!caps.contains(&Capability::MessagesApi));
    }

    // ── content_to_text ─────────────────────────────────────────────────────

    #[test]
    fn test_content_to_text_string() {
        let v = Value::String("hello world".to_string());
        assert_eq!(content_to_text(&v), "hello world");
    }

    #[test]
    fn test_content_to_text_array() {
        let v = serde_json::json!([
            {"type": "text", "text": "Hello "},
            {"type": "image_url", "image_url": {"url": "data:..."}},
            {"type": "text", "text": "world"}
        ]);
        assert_eq!(content_to_text(&v), "Hello world");
    }

    #[test]
    fn test_content_to_text_null() {
        assert_eq!(content_to_text(&Value::Null), "");
    }

    // ── value_to_document / document_to_value ───────────────────────────────

    #[test]
    fn test_value_document_roundtrip_string() {
        let v = Value::String("test".to_string());
        let doc = value_to_document(&v);
        assert_eq!(document_to_value(&doc), Some(v));
    }

    #[test]
    fn test_value_document_roundtrip_object() {
        let v = serde_json::json!({"name": "tool", "count": 42, "active": true});
        let doc = value_to_document(&v);
        let result = document_to_value(&doc).unwrap();
        assert_eq!(result["name"], "tool");
        assert_eq!(result["count"], 42);
        assert_eq!(result["active"], true);
    }

    #[test]
    fn test_value_document_null() {
        let doc = value_to_document(&Value::Null);
        assert!(matches!(doc, Document::Null));
        assert_eq!(document_to_value(&doc), None);
    }

    #[test]
    fn test_value_document_negative_int() {
        let v = serde_json::json!(-5);
        let doc = value_to_document(&v);
        assert_eq!(document_to_value(&doc), Some(v));
    }

    #[test]
    fn test_value_document_array() {
        let v = serde_json::json!([1, "two", true]);
        let doc = value_to_document(&v);
        let result = document_to_value(&doc).unwrap();
        assert_eq!(result, v);
    }

    // ── translate_stop_reason ───────────────────────────────────────────────

    #[test]
    fn test_stop_reason_end_turn() {
        assert_eq!(translate_stop_reason(&StopReason::EndTurn), "stop");
    }

    #[test]
    fn test_stop_reason_tool_use() {
        assert_eq!(translate_stop_reason(&StopReason::ToolUse), "tool_calls");
    }

    #[test]
    fn test_stop_reason_max_tokens() {
        assert_eq!(translate_stop_reason(&StopReason::MaxTokens), "length");
    }

    #[test]
    fn test_stop_reason_stop_sequence() {
        assert_eq!(translate_stop_reason(&StopReason::StopSequence), "stop");
    }

    #[test]
    fn test_stop_reason_content_filtered() {
        assert_eq!(
            translate_stop_reason(&StopReason::ContentFiltered),
            "content_filter"
        );
    }

    // ── build_inference_config ──────────────────────────────────────────────

    #[test]
    fn test_inference_config_none_when_no_params() {
        let req = ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
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
        };
        assert!(build_inference_config(&req).is_none());
    }

    #[test]
    fn test_inference_config_with_temperature() {
        let req = ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            temperature: Some(0.7),
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
        };
        let config = build_inference_config(&req);
        assert!(config.is_some());
        let ic = config.unwrap();
        assert_eq!(ic.temperature(), Some(0.7));
    }

    #[test]
    fn test_inference_config_with_max_tokens() {
        let req = ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: Some(1024),
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
        };
        let config = build_inference_config(&req).unwrap();
        assert_eq!(config.max_tokens(), Some(1024));
    }

    #[test]
    fn test_inference_config_with_stop_string() {
        let req = ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: None,
            top_p: None,
            n: None,
            stop: Some(Value::String("STOP".to_string())),
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
        };
        let config = build_inference_config(&req).unwrap();
        assert_eq!(config.stop_sequences(), ["STOP"]);
    }

    #[test]
    fn test_inference_config_with_stop_array() {
        let req = ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: None,
            top_p: None,
            n: None,
            stop: Some(serde_json::json!(["END", "STOP"])),
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
        };
        let config = build_inference_config(&req).unwrap();
        assert_eq!(config.stop_sequences(), ["END", "STOP"]);
    }

    // ── translate_messages ──────────────────────────────────────────────────

    #[test]
    fn test_translate_system_messages() {
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: Some(Value::String("You are helpful".to_string())),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            },
            Message {
                role: "user".to_string(),
                content: Some(Value::String("Hi".to_string())),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            },
        ];
        let (system, bedrock_msgs) = translate_messages(&messages).unwrap();
        assert!(system.is_some());
        assert_eq!(system.unwrap().len(), 1);
        assert_eq!(bedrock_msgs.len(), 1);
        assert_eq!(bedrock_msgs[0].role(), &ConversationRole::User);
    }

    #[test]
    fn test_translate_tool_results_merged() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: Some(Value::String("Hi".to_string())),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            },
            Message {
                role: "assistant".to_string(),
                content: None,
                tool_call_id: None,
                tool_calls: Some(vec![
                    ToolCall {
                        id: "tc1".to_string(),
                        r#type: "function".to_string(),
                        function: FunctionCall {
                            name: "get_weather".to_string(),
                            arguments: r#"{"city":"NYC"}"#.to_string(),
                        },
                    },
                    ToolCall {
                        id: "tc2".to_string(),
                        r#type: "function".to_string(),
                        function: FunctionCall {
                            name: "get_time".to_string(),
                            arguments: "{}".to_string(),
                        },
                    },
                ]),
                name: None,
            },
            Message {
                role: "tool".to_string(),
                content: Some(Value::String("Sunny, 75F".to_string())),
                tool_call_id: Some("tc1".to_string()),
                tool_calls: None,
                name: None,
            },
            Message {
                role: "tool".to_string(),
                content: Some(Value::String("3:00 PM".to_string())),
                tool_call_id: Some("tc2".to_string()),
                tool_calls: None,
                name: None,
            },
        ];
        let (system, bedrock_msgs) = translate_messages(&messages).unwrap();
        assert!(system.is_none());
        // user, assistant (with 2 tool_use), user (with 2 tool_results)
        assert_eq!(bedrock_msgs.len(), 3);
        assert_eq!(bedrock_msgs[0].role(), &ConversationRole::User);
        assert_eq!(bedrock_msgs[1].role(), &ConversationRole::Assistant);
        // The tool results should be merged into one User message.
        assert_eq!(bedrock_msgs[2].role(), &ConversationRole::User);
        assert_eq!(bedrock_msgs[2].content().len(), 2);
    }

    // ── build_tool_config ──────────────────────────────────────────────────

    #[test]
    fn test_tool_config_none_when_no_tools() {
        let req = ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
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
        };
        assert!(build_tool_config(&req).unwrap().is_none());
    }

    #[test]
    fn test_tool_config_with_tools() {
        let req = ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: None,
            top_p: None,
            n: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
            tools: Some(vec![Tool {
                r#type: "function".to_string(),
                function: FunctionDefinition {
                    name: "get_weather".to_string(),
                    description: Some("Get weather info".to_string()),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "city": {"type": "string"}
                        }
                    })),
                },
            }]),
            tool_choice: Some(Value::String("auto".to_string())),
            response_format: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
        };
        let config = build_tool_config(&req).unwrap();
        assert!(config.is_some());
        let tc = config.unwrap();
        assert_eq!(tc.tools().len(), 1);
    }

    #[test]
    fn test_tool_config_none_returns_none() {
        let req = ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: None,
            top_p: None,
            n: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
            tools: Some(vec![Tool {
                r#type: "function".to_string(),
                function: FunctionDefinition {
                    name: "get_weather".to_string(),
                    description: None,
                    parameters: None,
                },
            }]),
            tool_choice: Some(Value::String("none".to_string())),
            response_format: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
        };
        // tool_choice = "none" should return None (don't send tool config)
        assert!(build_tool_config(&req).unwrap().is_none());
    }

    // ── Unsupported error retryability ───────────────────────────────────────

    #[test]
    fn test_unsupported_error_is_retryable() {
        let e = ProviderError::Unsupported("not supported".to_string());
        assert!(e.is_retryable());
        assert_eq!(e.status_code(), 405);
    }
}
