//! Protocol bridge: Responses API ↔ Messages API conversion.
//!
//! Phase 1 converts Codex CLI's Responses API requests into Anthropic Messages
//! API format, and converts the Messages API responses back into Responses API
//! format — enabling Codex CLI to use Claude models through Copilot.

use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;
use serde_json::{json, Value};
use tracing::warn;
use uuid::Uuid;

// ── Request conversion (Responses → Messages) ───────────────────────────────

/// Convert an OpenAI Responses API request body to an Anthropic Messages API request.
///
/// Field mapping:
/// - `input` (string) → `messages: [{role: "user", content: input}]`
/// - `input` (array)  → extract message items + function_call_output → tool_result
/// - `instructions`   → `system`
/// - `max_output_tokens` → `max_tokens` (default 8192)
/// - `temperature`    → `temperature`
/// - `stream`         → `stream`
/// - `tools`          → `tools` (Anthropic format)
/// - `tool_choice`    → `tool_choice`
/// - `model`          → `model`
pub fn responses_to_messages_request(body: &Value) -> Value {
    let mut req = json!({});

    // model
    if let Some(model) = body.get("model") {
        req["model"] = model.clone();
    }

    // instructions → system
    if let Some(instructions) = body.get("instructions").and_then(|v| v.as_str()) {
        if !instructions.is_empty() {
            req["system"] = json!(instructions);
        }
    }

    // input → messages
    let messages = convert_input_to_messages(body.get("input"));
    if !messages.is_empty() {
        req["messages"] = json!(messages);
    }

    // max_output_tokens → max_tokens
    let max_tokens = body
        .get("max_output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(8192);
    req["max_tokens"] = json!(max_tokens);

    // stream
    if let Some(stream) = body.get("stream") {
        req["stream"] = stream.clone();
    }

    // temperature
    if let Some(temp) = body.get("temperature") {
        req["temperature"] = temp.clone();
    }

    // top_p
    if let Some(top_p) = body.get("top_p") {
        req["top_p"] = top_p.clone();
    }

    // tools
    if let Some(Value::Array(tools)) = body.get("tools") {
        let anthropic_tools: Vec<Value> =
            tools.iter().filter_map(convert_tool_definition).collect();
        if !anthropic_tools.is_empty() {
            req["tools"] = json!(anthropic_tools);
        }
    }

    // tool_choice
    if let Some(tc) = body.get("tool_choice") {
        req["tool_choice"] = convert_tool_choice(tc);
    }

    req
}

/// Convert the `input` field to Anthropic `messages` array.
///
/// Handles both string input (simple user message) and array input
/// (sequence of message items, function_call, function_call_output).
fn convert_input_to_messages(input: Option<&Value>) -> Vec<Value> {
    let input = match input {
        Some(v) => v,
        None => return vec![],
    };

    // Simple string input → single user message
    if let Some(text) = input.as_str() {
        return vec![json!({"role": "user", "content": text})];
    }

    // Array input → extract messages, tool calls, and tool results
    let items = match input.as_array() {
        Some(arr) => arr,
        None => return vec![],
    };

    let mut messages: Vec<Value> = Vec::new();
    // Accumulate content blocks for the current message role.
    let mut current_role: Option<String> = None;
    let mut current_content: Vec<Value> = Vec::new();

    for item in items {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match item_type {
            // Message items carry role + content
            "message" => {
                let role = item
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();

                // Flush accumulated content if role changes.
                if current_role.as_deref() != Some(&role) {
                    flush_message(&mut messages, &mut current_role, &mut current_content);
                    current_role = Some(role.clone());
                }

                // Extract content from the message item
                if let Some(content) = item.get("content") {
                    match content {
                        Value::String(s) => {
                            current_content.push(json!({"type": "text", "text": s}));
                        }
                        Value::Array(blocks) => {
                            for block in blocks {
                                let block_type =
                                    block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                match block_type {
                                    "output_text" | "input_text" => {
                                        if let Some(text) =
                                            block.get("text").and_then(|v| v.as_str())
                                        {
                                            current_content
                                                .push(json!({"type": "text", "text": text}));
                                        }
                                    }
                                    "refusal" => {
                                        if let Some(text) =
                                            block.get("refusal").and_then(|v| v.as_str())
                                        {
                                            current_content
                                                .push(json!({"type": "text", "text": text}));
                                        }
                                    }
                                    _ => {
                                        // Pass through unknown content blocks
                                        current_content.push(block.clone());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            // function_call → tool_use block in an assistant message
            "function_call" => {
                // Flush any accumulated content first.
                flush_message(&mut messages, &mut current_role, &mut current_content);

                let call_id = item
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");

                let input_val: Value =
                    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));

                messages.push(json!({
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": input_val
                    }]
                }));
            }

            // function_call_output → tool_result block in a user message
            "function_call_output" => {
                // Flush any accumulated content first.
                flush_message(&mut messages, &mut current_role, &mut current_content);

                let call_id = item
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let output = item
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": output
                    }]
                }));
            }

            // reasoning items are dropped (Anthropic handles thinking differently)
            "reasoning" => {}

            _ => {
                warn!(
                    item_type = item_type,
                    "Unknown input item type in Responses request"
                );
            }
        }
    }

    // Flush remaining accumulated content.
    flush_message(&mut messages, &mut current_role, &mut current_content);

    messages
}

/// Flush accumulated content blocks into a message and push to the messages vec.
fn flush_message(
    messages: &mut Vec<Value>,
    current_role: &mut Option<String>,
    current_content: &mut Vec<Value>,
) {
    if let Some(role) = current_role.take() {
        if !current_content.is_empty() {
            messages.push(json!({
                "role": role,
                "content": std::mem::take(current_content)
            }));
        }
    }
    current_content.clear();
}

/// Convert a Responses API tool definition to Anthropic format.
fn convert_tool_definition(tool: &Value) -> Option<Value> {
    let tool_type = tool.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if tool_type != "function" {
        return None;
    }

    let name = tool.get("name").and_then(|v| v.as_str())?;
    let description = tool
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let parameters = tool
        .get("parameters")
        .cloned()
        .unwrap_or(json!({"type": "object"}));

    Some(json!({
        "name": name,
        "description": description,
        "input_schema": parameters
    }))
}

/// Convert Responses API tool_choice to Anthropic format.
fn convert_tool_choice(tc: &Value) -> Value {
    match tc.as_str() {
        Some("required") => json!({"type": "any"}),
        Some("auto") => json!({"type": "auto"}),
        Some("none") => json!({"type": "auto"}),
        Some(other) => json!(other),
        None => {
            // Object form: {"type": "function", "name": "X"} → {"type": "tool", "name": "X"}
            if let Some(name) = tc.get("name").and_then(|v| v.as_str()) {
                json!({"type": "tool", "name": name})
            } else {
                tc.clone()
            }
        }
    }
}

// ── Response conversion (Messages → Responses) ──────────────────────────────

/// Convert an Anthropic Messages API response to Responses API format.
///
/// Field mapping:
/// - `id: "msg_xxx"` → `id: "resp_xxx"`
/// - `content` blocks → `output` array
/// - `stop_reason` → `status`
/// - `usage` → `usage` (with total_tokens)
pub fn messages_to_responses_response(body: &Value) -> Value {
    // id: replace msg_ prefix with resp_
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| {
            if let Some(stripped) = id.strip_prefix("msg_") {
                format!("resp_{stripped}")
            } else {
                format!("resp_{id}")
            }
        })
        .unwrap_or_else(|| format!("resp_{}", Uuid::new_v4().simple()));

    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Convert content blocks → output items
    let mut output: Vec<Value> = Vec::new();
    let mut message_content: Vec<Value> = Vec::new();

    if let Some(Value::Array(content)) = body.get("content") {
        for block in content {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    message_content.push(json!({
                        "type": "output_text",
                        "text": text,
                        "annotations": []
                    }));
                }
                "tool_use" => {
                    // Flush accumulated text into a message output item first.
                    if !message_content.is_empty() {
                        output.push(json!({
                            "type": "message",
                            "id": format!("msg_{}", Uuid::new_v4().simple()),
                            "role": "assistant",
                            "status": "completed",
                            "content": std::mem::take(&mut message_content)
                        }));
                    }

                    let call_id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let input_val = block.get("input").cloned().unwrap_or(json!({}));
                    let arguments = serde_json::to_string(&input_val).unwrap_or_default();

                    output.push(json!({
                        "type": "function_call",
                        "id": format!("fc_{}", Uuid::new_v4().simple()),
                        "call_id": call_id,
                        "name": name,
                        "arguments": arguments,
                        "status": "completed"
                    }));
                }
                "thinking" => {
                    let thinking = block.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                    output.push(json!({
                        "type": "reasoning",
                        "id": format!("rs_{}", Uuid::new_v4().simple()),
                        "summary": [{"type": "summary_text", "text": thinking}]
                    }));
                }
                _ => {}
            }
        }
    }

    // Flush remaining text content as a message output item.
    if !message_content.is_empty() {
        output.push(json!({
            "type": "message",
            "id": format!("msg_{}", Uuid::new_v4().simple()),
            "role": "assistant",
            "status": "completed",
            "content": message_content
        }));
    }

    // stop_reason → status
    let stop_reason = body
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("end_turn");
    let status = match stop_reason {
        "max_tokens" => "incomplete",
        _ => "completed", // end_turn, tool_use → completed
    };

    // usage
    let input_tokens = body
        .pointer("/usage/input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = body
        .pointer("/usage/output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read = body
        .pointer("/usage/cache_read_input_tokens")
        .and_then(|v| v.as_u64());
    let cache_creation = body
        .pointer("/usage/cache_creation_input_tokens")
        .and_then(|v| v.as_u64());

    let mut usage = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens
    });
    if let Some(v) = cache_read {
        usage["input_tokens_details"] = json!({"cached_tokens": v});
    }
    if let Some(v) = cache_creation {
        usage["cache_creation_input_tokens"] = json!(v);
    }

    json!({
        "id": id,
        "object": "response",
        "model": model,
        "status": status,
        "output": output,
        "usage": usage
    })
}

// ── SSE streaming conversion (Messages SSE → Responses SSE) ─────────────────

/// Convert a single Messages SSE event to one or more Responses SSE events.
///
/// Returns a vec of (event_name, data) pairs. Some Messages events expand
/// into multiple Responses events (e.g. `content_block_start` → `output_item.added`
/// + `content_part.added`).
pub fn messages_sse_to_responses_sse(event_name: &str, data: &Value) -> Vec<(String, Value)> {
    match event_name {
        "message_start" => convert_message_start(data),
        "content_block_start" => convert_content_block_start(data),
        "content_block_delta" => convert_content_block_delta(data),
        "content_block_stop" => convert_content_block_stop(data),
        "message_delta" => convert_message_delta(data),
        "message_stop" => vec![], // handled by message_delta
        "ping" => vec![],
        _ => {
            warn!(event = event_name, "Unknown Messages SSE event type");
            vec![]
        }
    }
}

/// message_start → response.created + response.in_progress
fn convert_message_start(data: &Value) -> Vec<(String, Value)> {
    let message = data.get("message").unwrap_or(data);

    let id = message
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| {
            if let Some(stripped) = id.strip_prefix("msg_") {
                format!("resp_{stripped}")
            } else {
                format!("resp_{id}")
            }
        })
        .unwrap_or_else(|| format!("resp_{}", Uuid::new_v4().simple()));

    let model = message
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let input_tokens = message
        .pointer("/usage/input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let response_obj = json!({
        "id": id,
        "object": "response",
        "model": model,
        "status": "in_progress",
        "output": [],
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": 0,
            "total_tokens": input_tokens
        }
    });

    vec![
        (
            "response.created".to_string(),
            json!({"type": "response.created", "response": response_obj.clone()}),
        ),
        (
            "response.in_progress".to_string(),
            json!({"type": "response.in_progress", "response": response_obj}),
        ),
    ]
}

/// content_block_start → output_item.added [+ content_part.added for text]
fn convert_content_block_start(data: &Value) -> Vec<(String, Value)> {
    let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
    let block = data.get("content_block").unwrap_or(data);
    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match block_type {
        "text" => {
            let item_id = format!("item_{}", Uuid::new_v4().simple());
            vec![
                (
                    "response.output_item.added".to_string(),
                    json!({
                        "type": "response.output_item.added",
                        "output_index": index,
                        "item": {
                            "type": "message",
                            "id": item_id,
                            "role": "assistant",
                            "status": "in_progress",
                            "content": []
                        }
                    }),
                ),
                (
                    "response.content_part.added".to_string(),
                    json!({
                        "type": "response.content_part.added",
                        "item_id": item_id,
                        "output_index": index,
                        "content_index": 0,
                        "part": {
                            "type": "output_text",
                            "text": "",
                            "annotations": []
                        }
                    }),
                ),
            ]
        }
        "tool_use" => {
            let tool_id = block
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = block
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            vec![(
                "response.output_item.added".to_string(),
                json!({
                    "type": "response.output_item.added",
                    "output_index": index,
                    "item": {
                        "type": "function_call",
                        "id": format!("fc_{}", Uuid::new_v4().simple()),
                        "call_id": tool_id,
                        "name": name,
                        "arguments": "",
                        "status": "in_progress"
                    }
                }),
            )]
        }
        "thinking" => {
            vec![(
                "response.output_item.added".to_string(),
                json!({
                    "type": "response.output_item.added",
                    "output_index": index,
                    "item": {
                        "type": "reasoning",
                        "id": format!("rs_{}", Uuid::new_v4().simple()),
                        "summary": []
                    }
                }),
            )]
        }
        _ => vec![],
    }
}

/// content_block_delta → output_text.delta / function_call_arguments.delta / reasoning.delta
fn convert_content_block_delta(data: &Value) -> Vec<(String, Value)> {
    let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
    let delta = match data.get("delta") {
        Some(d) => d,
        None => return vec![],
    };
    let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match delta_type {
        "text_delta" => {
            let text = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
            vec![(
                "response.output_text.delta".to_string(),
                json!({
                    "type": "response.output_text.delta",
                    "output_index": index,
                    "content_index": 0,
                    "delta": text
                }),
            )]
        }
        "input_json_delta" => {
            let partial = delta
                .get("partial_json")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            vec![(
                "response.function_call_arguments.delta".to_string(),
                json!({
                    "type": "response.function_call_arguments.delta",
                    "output_index": index,
                    "delta": partial
                }),
            )]
        }
        "thinking_delta" => {
            let thinking = delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
            vec![(
                "response.reasoning.delta".to_string(),
                json!({
                    "type": "response.reasoning.delta",
                    "output_index": index,
                    "delta": thinking
                }),
            )]
        }
        _ => vec![],
    }
}

/// content_block_stop → content_part.done + output_item.done
fn convert_content_block_stop(data: &Value) -> Vec<(String, Value)> {
    let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);

    vec![
        (
            "response.content_part.done".to_string(),
            json!({
                "type": "response.content_part.done",
                "output_index": index,
                "content_index": 0
            }),
        ),
        (
            "response.output_item.done".to_string(),
            json!({
                "type": "response.output_item.done",
                "output_index": index
            }),
        ),
    ]
}

/// message_delta → response.completed / response.incomplete
fn convert_message_delta(data: &Value) -> Vec<(String, Value)> {
    let delta = data.get("delta").unwrap_or(data);

    let stop_reason = delta
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("end_turn");

    let status = match stop_reason {
        "max_tokens" => "incomplete",
        _ => "completed",
    };

    let output_tokens = data
        .pointer("/usage/output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let event_type = if status == "completed" {
        "response.completed"
    } else {
        "response.incomplete"
    };

    vec![(
        event_type.to_string(),
        json!({
            "type": event_type,
            "response": {
                "status": status,
                "usage": {
                    "output_tokens": output_tokens
                }
            }
        }),
    )]
}

/// Format a sequence of Responses SSE events into an SSE text stream.
///
/// Used by the streaming proxy to convert buffered Messages SSE into
/// a complete Responses SSE text.
#[allow(dead_code)]
pub fn format_responses_sse_events(
    events: &[(String, Value)],
) -> Result<String, serde_json::Error> {
    let mut output = String::new();
    for (event_name, data) in events {
        output.push_str("event: ");
        output.push_str(event_name);
        output.push_str("\ndata: ");
        output.push_str(&serde_json::to_string(data)?);
        output.push_str("\n\n");
    }
    Ok(output)
}

/// Convert an entire buffered Messages SSE stream to a Responses SSE stream.
///
/// Parses the Messages SSE text, converts each event, and returns the
/// formatted Responses SSE text.
#[allow(dead_code)]
pub fn convert_messages_sse_stream(sse_text: &str) -> Result<String, serde_json::Error> {
    let mut all_events: Vec<(String, Value)> = Vec::new();
    let mut current_event: Option<String> = None;
    let mut current_data = String::new();

    for line in sse_text.lines() {
        if let Some(event) = line.strip_prefix("event: ") {
            current_event = Some(event.trim().to_string());
            current_data.clear();
        } else if let Some(data) = line.strip_prefix("data: ") {
            current_data.push_str(data);
        } else if line.trim().is_empty() {
            // End of event
            if let Some(ref event_name) = current_event {
                if !current_data.is_empty() {
                    if let Ok(data) = serde_json::from_str::<Value>(&current_data) {
                        let responses_events = messages_sse_to_responses_sse(event_name, &data);
                        all_events.extend(responses_events);
                    }
                }
            }
            current_event = None;
            current_data.clear();
        }
    }

    // Handle final event if stream doesn't end with empty line
    if let Some(ref event_name) = current_event {
        if !current_data.is_empty() {
            if let Ok(data) = serde_json::from_str::<Value>(&current_data) {
                let responses_events = messages_sse_to_responses_sse(event_name, &data);
                all_events.extend(responses_events);
            }
        }
    }

    format_responses_sse_events(&all_events)
}

// ── Phase 2: Request conversion (Messages → Responses) ────────────────────

/// Convert an Anthropic Messages API request body to an OpenAI Responses API request.
///
/// Field mapping:
/// - `messages` → `input` (array of Responses input items)
/// - `system`   → `instructions`
/// - `max_tokens` → `max_output_tokens`
/// - `temperature` → `temperature`
/// - `stream`   → `stream`
/// - `tools`    → `tools` (function type)
/// - `tool_choice` → `tool_choice`
/// - `model`    → `model`
pub fn messages_to_responses_request(body: &Value) -> Value {
    let mut req = json!({});

    // model
    if let Some(model) = body.get("model") {
        req["model"] = model.clone();
    }

    // system → instructions
    if let Some(system) = body.get("system") {
        match system {
            Value::String(s) => {
                if !s.is_empty() {
                    req["instructions"] = json!(s);
                }
            }
            Value::Array(parts) => {
                let text = parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    req["instructions"] = json!(text);
                }
            }
            _ => {}
        }
    }

    // messages → input
    let input = convert_messages_to_input(body.get("messages"));
    if !input.is_empty() {
        req["input"] = json!(input);
    }

    // max_tokens → max_output_tokens
    if let Some(max_tokens) = body.get("max_tokens") {
        req["max_output_tokens"] = max_tokens.clone();
    }

    // stream
    if let Some(stream) = body.get("stream") {
        req["stream"] = stream.clone();
    }

    // temperature
    if let Some(temp) = body.get("temperature") {
        req["temperature"] = temp.clone();
    }

    // top_p
    if let Some(top_p) = body.get("top_p") {
        req["top_p"] = top_p.clone();
    }

    // tools (Anthropic format → Responses format)
    if let Some(Value::Array(tools)) = body.get("tools") {
        let responses_tools: Vec<Value> = tools
            .iter()
            .filter_map(convert_anthropic_tool_to_responses)
            .collect();
        if !responses_tools.is_empty() {
            req["tools"] = json!(responses_tools);
        }
    }

    // tool_choice (Anthropic format → Responses format)
    if let Some(tc) = body.get("tool_choice") {
        req["tool_choice"] = convert_anthropic_tool_choice(tc);
    }

    req
}

/// Convert Anthropic `messages` array to Responses API `input` items.
fn convert_messages_to_input(messages: Option<&Value>) -> Vec<Value> {
    let messages = match messages.and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return vec![],
    };

    let mut input: Vec<Value> = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let content = msg.get("content");

        match role {
            "user" => convert_user_message(&mut input, content),
            "assistant" => convert_assistant_message(&mut input, content),
            _ => {}
        }
    }

    input
}

/// Convert a user message's content to Responses input items.
fn convert_user_message(input: &mut Vec<Value>, content: Option<&Value>) {
    let content = match content {
        Some(c) => c,
        None => return,
    };

    // Simple string content
    if let Some(text) = content.as_str() {
        input.push(json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": text}]
        }));
        return;
    }

    // Array content — may contain text or tool_result blocks
    if let Some(blocks) = content.as_array() {
        let mut text_parts: Vec<Value> = Vec::new();

        for block in blocks {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    text_parts.push(json!({"type": "input_text", "text": text}));
                }
                "tool_result" => {
                    // Flush accumulated text parts as a message first
                    if !text_parts.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "user",
                            "content": std::mem::take(&mut text_parts)
                        }));
                    }

                    let call_id = block
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let output = extract_tool_result_content(block.get("content"));

                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output
                    }));
                }
                _ => {}
            }
        }

        // Flush remaining text parts
        if !text_parts.is_empty() {
            input.push(json!({
                "type": "message",
                "role": "user",
                "content": text_parts
            }));
        }
    }
}

/// Extract string content from a tool_result content field.
fn extract_tool_result_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    b.get("text").and_then(|v| v.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Convert an assistant message's content to Responses input items.
fn convert_assistant_message(input: &mut Vec<Value>, content: Option<&Value>) {
    let content = match content {
        Some(c) => c,
        None => return,
    };

    // Simple string content
    if let Some(text) = content.as_str() {
        input.push(json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text}]
        }));
        return;
    }

    // Array content — may contain text, tool_use, or thinking blocks
    if let Some(blocks) = content.as_array() {
        let mut text_parts: Vec<Value> = Vec::new();

        for block in blocks {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    text_parts.push(json!({"type": "output_text", "text": text}));
                }
                "tool_use" => {
                    // Flush accumulated text parts as a message first
                    if !text_parts.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": std::mem::take(&mut text_parts)
                        }));
                    }

                    let call_id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let input_val = block.get("input").cloned().unwrap_or(json!({}));
                    let arguments = serde_json::to_string(&input_val).unwrap_or_default();

                    input.push(json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": arguments
                    }));
                }
                "thinking" => {
                    // Thinking blocks are not directly representable in Responses input;
                    // skip them (the model will re-reason on its own).
                }
                _ => {}
            }
        }

        // Flush remaining text parts
        if !text_parts.is_empty() {
            input.push(json!({
                "type": "message",
                "role": "assistant",
                "content": text_parts
            }));
        }
    }
}

/// Convert an Anthropic tool definition to Responses API format.
fn convert_anthropic_tool_to_responses(tool: &Value) -> Option<Value> {
    let name = tool.get("name").and_then(|v| v.as_str())?;
    let description = tool
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let parameters = tool
        .get("input_schema")
        .cloned()
        .unwrap_or(json!({"type": "object"}));

    Some(json!({
        "type": "function",
        "name": name,
        "description": description,
        "parameters": parameters
    }))
}

/// Convert Anthropic tool_choice to Responses API format.
fn convert_anthropic_tool_choice(tc: &Value) -> Value {
    match tc.get("type").and_then(|v| v.as_str()) {
        Some("any") => json!("required"),
        Some("auto") => json!("auto"),
        Some("tool") => {
            if let Some(name) = tc.get("name").and_then(|v| v.as_str()) {
                json!({"type": "function", "name": name})
            } else {
                json!("auto")
            }
        }
        _ => {
            // Could be a plain string already
            if let Some(s) = tc.as_str() {
                json!(s)
            } else {
                json!("auto")
            }
        }
    }
}

// ── Phase 2: Response conversion (Responses → Messages) ──────────────────

/// Convert an OpenAI Responses API response to Anthropic Messages API format.
///
/// Field mapping:
/// - `id: "resp_xxx"` → `id: "msg_xxx"`
/// - `output` array → `content` blocks
/// - `status` → `stop_reason`
/// - `usage` → `usage` (without total_tokens)
pub fn responses_to_messages_response(body: &Value) -> Value {
    // id: replace resp_ prefix with msg_
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| {
            if let Some(stripped) = id.strip_prefix("resp_") {
                format!("msg_{stripped}")
            } else {
                format!("msg_{id}")
            }
        })
        .unwrap_or_else(|| format!("msg_{}", Uuid::new_v4().simple()));

    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Convert output items → content blocks
    let mut content: Vec<Value> = Vec::new();
    let mut has_tool_use = false;

    if let Some(Value::Array(output)) = body.get("output") {
        for item in output {
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match item_type {
                "message" => {
                    if let Some(Value::Array(parts)) = item.get("content") {
                        for part in parts {
                            let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            if part_type == "output_text" {
                                let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                content.push(json!({"type": "text", "text": text}));
                            }
                        }
                    }
                }
                "function_call" => {
                    has_tool_use = true;
                    let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let arguments = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    let input_val: Value =
                        serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));

                    content.push(json!({
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": input_val
                    }));
                }
                "reasoning" => {
                    let thinking_text = item
                        .get("summary")
                        .and_then(|v| v.as_array())
                        .map(|summaries| {
                            summaries
                                .iter()
                                .filter_map(|s| s.get("text").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default();

                    if !thinking_text.is_empty() {
                        content.push(json!({
                            "type": "thinking",
                            "thinking": thinking_text
                        }));
                    }
                }
                _ => {}
            }
        }
    }

    // status → stop_reason
    let status = body
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("completed");
    let stop_reason = match status {
        "incomplete" => "max_tokens",
        _ => {
            if has_tool_use {
                "tool_use"
            } else {
                "end_turn"
            }
        }
    };

    // usage (without total_tokens)
    let input_tokens = body
        .pointer("/usage/input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = body
        .pointer("/usage/output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens
        }
    })
}

// ── Phase 2: SSE streaming conversion (Responses SSE → Messages SSE) ─────

/// Convert a single Responses SSE event to one or more Messages SSE events.
///
/// Returns a vec of (event_name, data) pairs. Some Responses events expand
/// into multiple Messages events.
pub fn responses_sse_to_messages_sse(event_name: &str, data: &Value) -> Vec<(String, Value)> {
    match event_name {
        "response.created" => convert_response_created(data),
        "response.output_item.added" => convert_output_item_added(data),
        "response.output_text.delta" => convert_output_text_delta(data),
        "response.function_call_arguments.delta" => convert_function_call_arguments_delta(data),
        "response.content_part.done" | "response.output_item.done" => {
            convert_content_or_item_done(data)
        }
        "response.completed" => convert_response_completed(data),
        "response.incomplete" => convert_response_incomplete(data),
        "response.in_progress" => vec![], // no Messages equivalent
        "response.content_part.added" => vec![], // handled by output_item.added
        "response.output_text.done" => vec![],
        "response.function_call_arguments.done" => vec![],
        "response.reasoning.delta" => vec![], // thinking deltas not mapped
        _ => {
            warn!(event = event_name, "Unknown Responses SSE event type");
            vec![]
        }
    }
}

/// response.created → message_start
fn convert_response_created(data: &Value) -> Vec<(String, Value)> {
    let response = data.get("response").unwrap_or(data);

    let id = response
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| {
            if let Some(stripped) = id.strip_prefix("resp_") {
                format!("msg_{stripped}")
            } else {
                format!("msg_{id}")
            }
        })
        .unwrap_or_else(|| format!("msg_{}", Uuid::new_v4().simple()));

    let model = response
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let input_tokens = response
        .pointer("/usage/input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    vec![(
        "message_start".to_string(),
        json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": 0
                }
            }
        }),
    )]
}

/// response.output_item.added → content_block_start
fn convert_output_item_added(data: &Value) -> Vec<(String, Value)> {
    let index = data
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let item = data.get("item").unwrap_or(data);
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match item_type {
        "message" => {
            vec![(
                "content_block_start".to_string(),
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "text", "text": ""}
                }),
            )]
        }
        "function_call" => {
            let tool_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");

            vec![(
                "content_block_start".to_string(),
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {
                        "type": "tool_use",
                        "id": tool_id,
                        "name": name,
                        "input": {}
                    }
                }),
            )]
        }
        _ => vec![],
    }
}

/// response.output_text.delta → content_block_delta (text_delta)
fn convert_output_text_delta(data: &Value) -> Vec<(String, Value)> {
    let index = data
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let text = data.get("delta").and_then(|v| v.as_str()).unwrap_or("");

    vec![(
        "content_block_delta".to_string(),
        json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "text_delta", "text": text}
        }),
    )]
}

/// response.function_call_arguments.delta → content_block_delta (input_json_delta)
fn convert_function_call_arguments_delta(data: &Value) -> Vec<(String, Value)> {
    let index = data
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let partial = data.get("delta").and_then(|v| v.as_str()).unwrap_or("");

    vec![(
        "content_block_delta".to_string(),
        json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "input_json_delta", "partial_json": partial}
        }),
    )]
}

/// response.content_part.done / response.output_item.done → content_block_stop
fn convert_content_or_item_done(data: &Value) -> Vec<(String, Value)> {
    let index = data
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    vec![(
        "content_block_stop".to_string(),
        json!({
            "type": "content_block_stop",
            "index": index
        }),
    )]
}

/// response.completed → message_delta + message_stop
fn convert_response_completed(data: &Value) -> Vec<(String, Value)> {
    let output_tokens = data
        .pointer("/response/usage/output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    vec![
        (
            "message_delta".to_string(),
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": output_tokens}
            }),
        ),
        ("message_stop".to_string(), json!({"type": "message_stop"})),
    ]
}

/// response.incomplete → message_delta (max_tokens) + message_stop
fn convert_response_incomplete(data: &Value) -> Vec<(String, Value)> {
    let output_tokens = data
        .pointer("/response/usage/output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    vec![
        (
            "message_delta".to_string(),
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "max_tokens"},
                "usage": {"output_tokens": output_tokens}
            }),
        ),
        ("message_stop".to_string(), json!({"type": "message_stop"})),
    ]
}

/// Format a sequence of Messages SSE events into an SSE text stream.
#[allow(dead_code)]
pub fn format_messages_sse_events(events: &[(String, Value)]) -> Result<String, serde_json::Error> {
    let mut output = String::new();
    for (event_name, data) in events {
        output.push_str("event: ");
        output.push_str(event_name);
        output.push_str("\ndata: ");
        output.push_str(&serde_json::to_string(data)?);
        output.push_str("\n\n");
    }
    Ok(output)
}

/// Convert an entire buffered Responses SSE stream to a Messages SSE stream.
///
/// Parses the Responses SSE text, converts each event, and returns the
/// formatted Messages SSE text.
#[allow(dead_code)]
pub fn convert_responses_sse_stream(sse_text: &str) -> Result<String, serde_json::Error> {
    let mut all_events: Vec<(String, Value)> = Vec::new();
    let mut current_event: Option<String> = None;
    let mut current_data = String::new();

    for line in sse_text.lines() {
        if let Some(event) = line.strip_prefix("event: ") {
            current_event = Some(event.trim().to_string());
            current_data.clear();
        } else if let Some(data) = line.strip_prefix("data: ") {
            current_data.push_str(data);
        } else if line.trim().is_empty() {
            if let Some(ref event_name) = current_event {
                if !current_data.is_empty() {
                    if let Ok(data) = serde_json::from_str::<Value>(&current_data) {
                        let messages_events = responses_sse_to_messages_sse(event_name, &data);
                        all_events.extend(messages_events);
                    }
                }
            }
            current_event = None;
            current_data.clear();
        }
    }

    // Handle final event if stream doesn't end with empty line
    if let Some(ref event_name) = current_event {
        if !current_data.is_empty() {
            if let Ok(data) = serde_json::from_str::<Value>(&current_data) {
                let messages_events = responses_sse_to_messages_sse(event_name, &data);
                all_events.extend(messages_events);
            }
        }
    }

    format_messages_sse_events(&all_events)
}

// ── Streaming SSE adapters ───────────────────────────────────────────────────

/// Parse complete SSE events from a buffer, returning remaining unparsed bytes.
///
/// Each complete event (delimited by `\n\n`) is returned as a `(event_name, data_json)` pair.
fn drain_sse_events(buffer: &mut String) -> Vec<(String, Value)> {
    let mut events = Vec::new();

    while let Some(pos) = buffer.find("\n\n") {
        let block = buffer[..pos].to_string();
        *buffer = buffer[pos + 2..].to_string();

        if block.trim().is_empty() {
            continue;
        }

        let mut event_type: Option<String> = None;
        let mut data_parts: Vec<String> = Vec::new();

        for line in block.lines() {
            if let Some(evt) = line.strip_prefix("event: ") {
                event_type = Some(evt.trim().to_string());
            } else if let Some(d) = line.strip_prefix("data: ") {
                data_parts.push(d.to_string());
            } else if let Some(evt) = line.strip_prefix("event:") {
                event_type = Some(evt.trim().to_string());
            } else if let Some(d) = line.strip_prefix("data:") {
                data_parts.push(d.to_string());
            }
        }

        if data_parts.is_empty() {
            continue;
        }

        let data_str = data_parts.join("\n");
        let event_name = event_type.unwrap_or_default();

        if let Ok(data) = serde_json::from_str::<Value>(&data_str) {
            events.push((event_name, data));
        }
    }

    events
}

/// Format a single converted SSE event pair as SSE text bytes.
fn format_sse_event(event_name: &str, data: &Value) -> Bytes {
    let mut out = String::new();
    out.push_str("event: ");
    out.push_str(event_name);
    out.push_str("\ndata: ");
    out.push_str(&serde_json::to_string(data).unwrap_or_default());
    out.push_str("\n\n");
    Bytes::from(out)
}

/// Create a true-streaming adapter: upstream Messages SSE → downstream Responses SSE.
///
/// Reads upstream SSE events one-by-one from the byte stream, converts each via
/// [`messages_sse_to_responses_sse`], and yields converted events immediately.
pub fn create_responses_sse_stream_from_messages<E: std::error::Error + Send + 'static>(
    stream: impl Stream<Item = Result<Bytes, E>> + Send + 'static,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::stream! {
        let mut buffer = String::new();
        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    buffer.push_str(&text);

                    for (event_name, data) in drain_sse_events(&mut buffer) {
                        let converted = messages_sse_to_responses_sse(&event_name, &data);
                        for (out_event, out_data) in &converted {
                            yield Ok(format_sse_event(out_event, out_data));
                        }
                    }
                }
                Err(e) => {
                    let error_event = json!({
                        "type": "error",
                        "error": {
                            "type": "stream_error",
                            "message": format!("Upstream stream error: {e}")
                        }
                    });
                    yield Ok(format_sse_event("error", &error_event));
                    break;
                }
            }
        }

        // Flush any remaining data in the buffer (stream ended without trailing \n\n).
        if !buffer.trim().is_empty() {
            buffer.push_str("\n\n");
            for (event_name, data) in drain_sse_events(&mut buffer) {
                let converted = messages_sse_to_responses_sse(&event_name, &data);
                for (out_event, out_data) in &converted {
                    yield Ok(format_sse_event(out_event, out_data));
                }
            }
        }
    }
}

/// Create a true-streaming adapter: upstream Responses SSE → downstream Messages SSE.
///
/// Reads upstream SSE events one-by-one from the byte stream, converts each via
/// [`responses_sse_to_messages_sse`], and yields converted events immediately.
pub fn create_messages_sse_stream_from_responses<E: std::error::Error + Send + 'static>(
    stream: impl Stream<Item = Result<Bytes, E>> + Send + 'static,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::stream! {
        let mut buffer = String::new();
        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    buffer.push_str(&text);

                    for (event_name, data) in drain_sse_events(&mut buffer) {
                        let converted = responses_sse_to_messages_sse(&event_name, &data);
                        for (out_event, out_data) in &converted {
                            yield Ok(format_sse_event(out_event, out_data));
                        }
                    }
                }
                Err(e) => {
                    let error_event = json!({
                        "type": "error",
                        "error": {
                            "type": "stream_error",
                            "message": format!("Upstream stream error: {e}")
                        }
                    });
                    yield Ok(format_sse_event("error", &error_event));
                    break;
                }
            }
        }

        // Flush remaining buffer.
        if !buffer.trim().is_empty() {
            buffer.push_str("\n\n");
            for (event_name, data) in drain_sse_events(&mut buffer) {
                let converted = responses_sse_to_messages_sse(&event_name, &data);
                for (out_event, out_data) in &converted {
                    yield Ok(format_sse_event(out_event, out_data));
                }
            }
        }
    }
}

// ── Phase 3: Request conversion (Responses → Chat Completions) ──────────────

/// Convert an OpenAI Responses API request body to a Chat Completions API request.
///
/// Field mapping:
/// - `input` (string/array) → `messages`
/// - `instructions` → prepended system message
/// - `model` → `model`
/// - `stream` → `stream`
/// - `max_output_tokens` → `max_tokens`
/// - `temperature` → `temperature`
/// - `top_p` → `top_p`
/// - `tools` → `tools` (Chat Completions format)
/// - `tool_choice` → `tool_choice`
pub fn responses_to_chat_completions_request(body: &Value) -> Value {
    let mut req = json!({});

    // model
    if let Some(model) = body.get("model") {
        req["model"] = model.clone();
    }

    // Build messages from input
    let mut messages = convert_input_to_cc_messages(body.get("input"));

    // instructions → prepend system message
    if let Some(instructions) = body.get("instructions").and_then(|v| v.as_str()) {
        if !instructions.is_empty() {
            messages.insert(0, json!({"role": "system", "content": instructions}));
        }
    }

    if !messages.is_empty() {
        req["messages"] = json!(messages);
    }

    // stream
    if let Some(stream) = body.get("stream") {
        req["stream"] = stream.clone();
    }

    // max_output_tokens → max_tokens
    if let Some(max_tokens) = body.get("max_output_tokens") {
        req["max_tokens"] = max_tokens.clone();
    }

    // temperature
    if let Some(temp) = body.get("temperature") {
        req["temperature"] = temp.clone();
    }

    // top_p
    if let Some(top_p) = body.get("top_p") {
        req["top_p"] = top_p.clone();
    }

    // tools → Chat Completions format
    if let Some(Value::Array(tools)) = body.get("tools") {
        let cc_tools: Vec<Value> = tools.iter().filter_map(convert_tool_to_cc_format).collect();
        if !cc_tools.is_empty() {
            req["tools"] = json!(cc_tools);
        }
    }

    // tool_choice: "required"/"auto"/"none" pass through directly
    if let Some(tc) = body.get("tool_choice") {
        req["tool_choice"] = convert_tool_choice_to_cc(tc);
    }

    req
}

/// Convert Responses API `input` to Chat Completions `messages` array.
fn convert_input_to_cc_messages(input: Option<&Value>) -> Vec<Value> {
    let input = match input {
        Some(v) => v,
        None => return vec![],
    };

    // Simple string input → single user message
    if let Some(text) = input.as_str() {
        return vec![json!({"role": "user", "content": text})];
    }

    let items = match input.as_array() {
        Some(arr) => arr,
        None => return vec![],
    };

    let mut messages: Vec<Value> = Vec::new();
    let mut pending_tool_calls: Vec<Value> = Vec::new();

    for item in items {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match item_type {
            "message" => {
                // Flush any pending tool_calls as an assistant message
                flush_cc_tool_calls(&mut messages, &mut pending_tool_calls);

                let role = item.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                let content = extract_cc_text_content(item);
                messages.push(json!({"role": role, "content": content}));
            }

            "function_call" => {
                let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");

                pending_tool_calls.push(json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments
                    }
                }));
            }

            "function_call_output" => {
                // Flush pending tool_calls before adding tool response
                flush_cc_tool_calls(&mut messages, &mut pending_tool_calls);

                let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                let output = item.get("output").and_then(|v| v.as_str()).unwrap_or("");

                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": output
                }));
            }

            // reasoning items are dropped
            "reasoning" => {}

            _ => {
                warn!(
                    item_type = item_type,
                    "Unknown input item type in Responses→CC conversion"
                );
            }
        }
    }

    flush_cc_tool_calls(&mut messages, &mut pending_tool_calls);

    messages
}

/// Flush accumulated tool_calls into a single assistant message.
fn flush_cc_tool_calls(messages: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if !pending.is_empty() {
        messages.push(json!({
            "role": "assistant",
            "content": null,
            "tool_calls": std::mem::take(pending)
        }));
    }
}

/// Extract text content from a Responses message item for CC format.
fn extract_cc_text_content(item: &Value) -> String {
    match item.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match block_type {
                    "input_text" | "output_text" => {
                        block.get("text").and_then(|v| v.as_str()).map(String::from)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Convert a Responses API tool definition to Chat Completions format.
fn convert_tool_to_cc_format(tool: &Value) -> Option<Value> {
    let tool_type = tool.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if tool_type != "function" {
        return None;
    }

    let name = tool.get("name").and_then(|v| v.as_str())?;
    let description = tool
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let parameters = tool
        .get("parameters")
        .cloned()
        .unwrap_or(json!({"type": "object"}));

    Some(json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    }))
}

/// Convert Responses API tool_choice to Chat Completions format.
fn convert_tool_choice_to_cc(tc: &Value) -> Value {
    match tc.as_str() {
        Some("required") => json!("required"),
        Some("auto") => json!("auto"),
        Some("none") => json!("none"),
        Some(other) => json!(other),
        None => {
            // Object form: {"type": "function", "name": "X"}
            if let Some(name) = tc.get("name").and_then(|v| v.as_str()) {
                json!({"type": "function", "function": {"name": name}})
            } else {
                tc.clone()
            }
        }
    }
}

// ── Phase 3: Response conversion (Chat Completions → Responses) ─────────────

/// Convert a Chat Completions API response to Responses API format.
///
/// Field mapping:
/// - `id` → `id` (prefixed with "resp_")
/// - `choices[0].message.content` → output message
/// - `choices[0].message.tool_calls` → output function_calls
/// - `choices[0].finish_reason` → status
/// - `usage` → usage
pub fn chat_completions_to_responses_response(body: &Value) -> Value {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| format!("resp_{id}"))
        .unwrap_or_else(|| format!("resp_{}", Uuid::new_v4().simple()));

    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let choice = body
        .pointer("/choices/0")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));
    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("stop");

    // Build output items
    let mut output: Vec<Value> = Vec::new();

    // Text content → message output item
    if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
        if !content.is_empty() {
            output.push(json!({
                "type": "message",
                "id": format!("msg_{}", Uuid::new_v4().simple()),
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": content,
                    "annotations": []
                }]
            }));
        }
    }

    // Tool calls → function_call output items
    if let Some(Value::Array(tool_calls)) = message.get("tool_calls") {
        for tc in tool_calls {
            let call_id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let function = tc.get("function").cloned().unwrap_or_else(|| json!({}));
            let name = function.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = function
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");

            output.push(json!({
                "type": "function_call",
                "id": format!("fc_{}", Uuid::new_v4().simple()),
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
                "status": "completed"
            }));
        }
    }

    // finish_reason → status
    let status = match finish_reason {
        "length" => "incomplete",
        _ => "completed", // "stop", "tool_calls" → completed
    };

    // usage
    let prompt_tokens = body
        .pointer("/usage/prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = body
        .pointer("/usage/completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    json!({
        "id": id,
        "object": "response",
        "model": model,
        "status": status,
        "output": output,
        "usage": {
            "input_tokens": prompt_tokens,
            "output_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens
        }
    })
}

// ── Phase 3: SSE streaming conversion (Chat Completions SSE → Responses SSE) ──

/// State tracker for Chat Completions SSE → Responses SSE conversion.
///
/// Chat Completions uses delta chunks that need to be accumulated and mapped
/// to the Responses SSE event model. This state tracks what has been emitted.
pub struct ChatCompletionsStreamState {
    /// Whether response.created + response.in_progress have been emitted.
    created: bool,
    /// Whether the text content output_item has been added.
    content_item_added: bool,
    /// Current output index for the next item.
    output_index: u64,
    /// Set of tool_call indices that have already had output_item.added emitted.
    tool_calls_added: std::collections::HashSet<u64>,
    /// The output_index assigned to the text message item (if any).
    text_output_index: Option<u64>,
}

impl ChatCompletionsStreamState {
    pub fn new() -> Self {
        Self {
            created: false,
            content_item_added: false,
            output_index: 0,
            tool_calls_added: std::collections::HashSet::new(),
            text_output_index: None,
        }
    }
}

impl Default for ChatCompletionsStreamState {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a single Chat Completions SSE chunk to Responses SSE events.
///
/// Chat Completions SSE uses unnamed events with `data: {...}` lines.
/// This function takes the parsed JSON data and the mutable state, returning
/// zero or more Responses SSE events.
pub fn chat_completions_sse_to_responses_sse(
    data: &Value,
    state: &mut ChatCompletionsStreamState,
) -> Vec<(String, Value)> {
    let mut events: Vec<(String, Value)> = Vec::new();

    let id = data
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| format!("resp_{id}"))
        .unwrap_or_default();

    let model = data
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Emit response.created + response.in_progress on the first chunk
    if !state.created {
        state.created = true;
        let response_obj = json!({
            "id": id,
            "object": "response",
            "model": model,
            "status": "in_progress",
            "output": [],
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "total_tokens": 0
            }
        });
        events.push((
            "response.created".to_string(),
            json!({"type": "response.created", "response": response_obj.clone()}),
        ));
        events.push((
            "response.in_progress".to_string(),
            json!({"type": "response.in_progress", "response": response_obj}),
        ));
    }

    let choice = match data.pointer("/choices/0") {
        Some(c) => c,
        None => return events,
    };

    let delta = choice.get("delta").cloned().unwrap_or_else(|| json!({}));
    let finish_reason = choice.get("finish_reason").and_then(|v| v.as_str());

    // Handle text content delta
    if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
        // Emit output_item.added + content_part.added on first content chunk
        if !state.content_item_added {
            state.content_item_added = true;
            let text_idx = state.output_index;
            state.text_output_index = Some(text_idx);
            state.output_index += 1;

            let item_id = format!("item_{}", Uuid::new_v4().simple());
            events.push((
                "response.output_item.added".to_string(),
                json!({
                    "type": "response.output_item.added",
                    "output_index": text_idx,
                    "item": {
                        "type": "message",
                        "id": item_id,
                        "role": "assistant",
                        "status": "in_progress",
                        "content": []
                    }
                }),
            ));
            events.push((
                "response.content_part.added".to_string(),
                json!({
                    "type": "response.content_part.added",
                    "item_id": item_id,
                    "output_index": text_idx,
                    "content_index": 0,
                    "part": {
                        "type": "output_text",
                        "text": "",
                        "annotations": []
                    }
                }),
            ));
        }

        if !content.is_empty() {
            let text_idx = state.text_output_index.unwrap_or(0);
            events.push((
                "response.output_text.delta".to_string(),
                json!({
                    "type": "response.output_text.delta",
                    "output_index": text_idx,
                    "content_index": 0,
                    "delta": content
                }),
            ));
        }
    }

    // Handle tool_calls delta
    if let Some(Value::Array(tool_calls)) = delta.get("tool_calls") {
        for tc in tool_calls {
            let tc_index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0);

            // Emit output_item.added on first occurrence of this tool_call index
            if !state.tool_calls_added.contains(&tc_index) {
                state.tool_calls_added.insert(tc_index);

                let call_id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let name = tc
                    .pointer("/function/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let out_idx = state.output_index;
                state.output_index += 1;

                events.push((
                    "response.output_item.added".to_string(),
                    json!({
                        "type": "response.output_item.added",
                        "output_index": out_idx,
                        "item": {
                            "type": "function_call",
                            "id": format!("fc_{}", Uuid::new_v4().simple()),
                            "call_id": call_id,
                            "name": name,
                            "arguments": "",
                            "status": "in_progress"
                        }
                    }),
                ));
            }

            // Emit arguments delta if present
            if let Some(args) = tc.pointer("/function/arguments").and_then(|v| v.as_str()) {
                if !args.is_empty() {
                    // Compute the output_index for this tool_call:
                    // it's base offset (text item count) + tc_index
                    let base = if state.content_item_added { 1 } else { 0 };
                    let out_idx = base + tc_index;

                    events.push((
                        "response.function_call_arguments.delta".to_string(),
                        json!({
                            "type": "response.function_call_arguments.delta",
                            "output_index": out_idx,
                            "delta": args
                        }),
                    ));
                }
            }
        }
    }

    // Handle finish_reason
    if let Some(reason) = finish_reason {
        match reason {
            "stop" => {
                // Close the text content item
                if let Some(text_idx) = state.text_output_index {
                    events.push((
                        "response.content_part.done".to_string(),
                        json!({
                            "type": "response.content_part.done",
                            "output_index": text_idx,
                            "content_index": 0
                        }),
                    ));
                    events.push((
                        "response.output_item.done".to_string(),
                        json!({
                            "type": "response.output_item.done",
                            "output_index": text_idx
                        }),
                    ));
                }
                let usage = extract_cc_usage(data);
                events.push((
                    "response.completed".to_string(),
                    json!({
                        "type": "response.completed",
                        "response": {
                            "id": id,
                            "object": "response",
                            "model": model,
                            "status": "completed",
                            "usage": usage
                        }
                    }),
                ));
            }
            "tool_calls" => {
                // Close each tool_call item
                let base = if state.content_item_added { 1 } else { 0 };
                for &tc_idx in &state.tool_calls_added {
                    let out_idx = base + tc_idx;
                    events.push((
                        "response.output_item.done".to_string(),
                        json!({
                            "type": "response.output_item.done",
                            "output_index": out_idx
                        }),
                    ));
                }
                let usage = extract_cc_usage(data);
                events.push((
                    "response.completed".to_string(),
                    json!({
                        "type": "response.completed",
                        "response": {
                            "id": id,
                            "object": "response",
                            "model": model,
                            "status": "completed",
                            "usage": usage
                        }
                    }),
                ));
            }
            "length" => {
                // Close any open items
                if let Some(text_idx) = state.text_output_index {
                    events.push((
                        "response.content_part.done".to_string(),
                        json!({
                            "type": "response.content_part.done",
                            "output_index": text_idx,
                            "content_index": 0
                        }),
                    ));
                    events.push((
                        "response.output_item.done".to_string(),
                        json!({
                            "type": "response.output_item.done",
                            "output_index": text_idx
                        }),
                    ));
                }
                let usage = extract_cc_usage(data);
                events.push((
                    "response.incomplete".to_string(),
                    json!({
                        "type": "response.incomplete",
                        "response": {
                            "id": id,
                            "object": "response",
                            "model": model,
                            "status": "incomplete",
                            "usage": usage
                        }
                    }),
                ));
            }
            _ => {}
        }
    }

    events
}

/// Extract usage from a Chat Completions chunk (if present).
fn extract_cc_usage(data: &Value) -> Value {
    let prompt_tokens = data
        .pointer("/usage/prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = data
        .pointer("/usage/completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    json!({
        "input_tokens": prompt_tokens,
        "output_tokens": completion_tokens,
        "total_tokens": prompt_tokens + completion_tokens
    })
}

// ── Phase 3: Streaming SSE adapter ──────────────────────────────────────────

/// Parse Chat Completions SSE data lines from a buffer.
///
/// Chat Completions SSE uses only `data:` lines (no `event:` lines), delimited
/// by `\n\n`. Returns parsed JSON values, skipping `[DONE]`.
fn drain_cc_sse_events(buffer: &mut String) -> Vec<Value> {
    let mut events = Vec::new();

    while let Some(pos) = buffer.find("\n\n") {
        let block = buffer[..pos].to_string();
        *buffer = buffer[pos + 2..].to_string();

        if block.trim().is_empty() {
            continue;
        }

        for line in block.lines() {
            let data = if let Some(d) = line.strip_prefix("data: ") {
                d
            } else if let Some(d) = line.strip_prefix("data:") {
                d.trim_start()
            } else {
                continue;
            };

            if data == "[DONE]" {
                continue;
            }

            if let Ok(value) = serde_json::from_str::<Value>(data) {
                events.push(value);
            }
        }
    }

    events
}

/// Create a true-streaming adapter: upstream Chat Completions SSE → downstream Responses SSE.
///
/// Reads upstream Chat Completions SSE chunks, converts each via
/// [`chat_completions_sse_to_responses_sse`], and yields Responses SSE events immediately.
pub fn create_responses_sse_stream_from_chat_completions<E: std::error::Error + Send + 'static>(
    stream: impl Stream<Item = Result<Bytes, E>> + Send + 'static,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut state = ChatCompletionsStreamState::new();
        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    buffer.push_str(&text);

                    for data in drain_cc_sse_events(&mut buffer) {
                        let converted = chat_completions_sse_to_responses_sse(&data, &mut state);
                        for (out_event, out_data) in &converted {
                            yield Ok(format_sse_event(out_event, out_data));
                        }
                    }
                }
                Err(e) => {
                    let error_event = json!({
                        "type": "error",
                        "error": {
                            "type": "stream_error",
                            "message": format!("Upstream stream error: {e}")
                        }
                    });
                    yield Ok(format_sse_event("error", &error_event));
                    break;
                }
            }
        }

        // Flush remaining buffer.
        if !buffer.trim().is_empty() {
            buffer.push_str("\n\n");
            for data in drain_cc_sse_events(&mut buffer) {
                let converted = chat_completions_sse_to_responses_sse(&data, &mut state);
                for (out_event, out_data) in &converted {
                    yield Ok(format_sse_event(out_event, out_data));
                }
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Request conversion tests ─────────────────────────────────────────

    #[test]
    fn test_simple_text_input() {
        let body = json!({
            "model": "claude-sonnet-4-20250514",
            "input": "What is the capital of France?",
            "max_output_tokens": 1024,
            "stream": false
        });

        let result = responses_to_messages_request(&body);

        assert_eq!(result["model"], "claude-sonnet-4-20250514");
        assert_eq!(result["max_tokens"], 1024);
        assert_eq!(result["stream"], false);

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "What is the capital of France?");
    }

    #[test]
    fn test_with_instructions() {
        let body = json!({
            "model": "claude-sonnet-4-20250514",
            "input": "Hello",
            "instructions": "You are a helpful assistant."
        });

        let result = responses_to_messages_request(&body);

        assert_eq!(result["system"], "You are a helpful assistant.");
    }

    #[test]
    fn test_default_max_tokens() {
        let body = json!({
            "model": "claude-sonnet-4-20250514",
            "input": "Hello"
        });

        let result = responses_to_messages_request(&body);
        assert_eq!(result["max_tokens"], 8192);
    }

    #[test]
    fn test_with_tools() {
        let body = json!({
            "model": "claude-sonnet-4-20250514",
            "input": "What's the weather?",
            "tools": [{
                "type": "function",
                "name": "get_weather",
                "description": "Get weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    },
                    "required": ["city"]
                }
            }],
            "tool_choice": "auto"
        });

        let result = responses_to_messages_request(&body);

        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "get_weather");
        assert_eq!(tools[0]["description"], "Get weather for a city");
        assert!(tools[0].get("input_schema").is_some());
        assert!(tools[0].get("type").is_none()); // Anthropic tools don't have type field

        assert_eq!(result["tool_choice"]["type"], "auto");
    }

    #[test]
    fn test_tool_choice_required() {
        let body = json!({
            "model": "claude-sonnet-4-20250514",
            "input": "Hello",
            "tool_choice": "required"
        });

        let result = responses_to_messages_request(&body);
        assert_eq!(result["tool_choice"]["type"], "any");
    }

    #[test]
    fn test_with_function_call_output() {
        let body = json!({
            "model": "claude-sonnet-4-20250514",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "What's the weather in Tokyo?"
                },
                {
                    "type": "function_call",
                    "call_id": "call_123",
                    "name": "get_weather",
                    "arguments": "{\"city\":\"Tokyo\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_123",
                    "output": "Sunny, 25°C"
                }
            ]
        });

        let result = responses_to_messages_request(&body);

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);

        // First: user message
        assert_eq!(messages[0]["role"], "user");

        // Second: assistant with tool_use
        assert_eq!(messages[1]["role"], "assistant");
        let tool_use = &messages[1]["content"][0];
        assert_eq!(tool_use["type"], "tool_use");
        assert_eq!(tool_use["id"], "call_123");
        assert_eq!(tool_use["name"], "get_weather");
        assert_eq!(tool_use["input"]["city"], "Tokyo");

        // Third: user with tool_result
        assert_eq!(messages[2]["role"], "user");
        let tool_result = &messages[2]["content"][0];
        assert_eq!(tool_result["type"], "tool_result");
        assert_eq!(tool_result["tool_use_id"], "call_123");
        assert_eq!(tool_result["content"], "Sunny, 25°C");
    }

    #[test]
    fn test_array_input_with_content_blocks() {
        let body = json!({
            "model": "claude-sonnet-4-20250514",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": "Hello there"}
                    ]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Hi! How can I help?"}
                    ]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": "Follow-up question"
                }
            ]
        });

        let result = responses_to_messages_request(&body);
        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["text"], "Hello there");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["text"], "Hi! How can I help?");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["text"], "Follow-up question");
    }

    // ── Response conversion tests ────────────────────────────────────────

    #[test]
    fn test_simple_text_response() {
        let body = json!({
            "id": "msg_abc123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {"type": "text", "text": "The capital of France is Paris."}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20
            }
        });

        let result = messages_to_responses_response(&body);

        assert_eq!(result["id"], "resp_abc123");
        assert_eq!(result["object"], "response");
        assert_eq!(result["model"], "claude-sonnet-4-20250514");
        assert_eq!(result["status"], "completed");

        let output = result["output"].as_array().unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[0]["content"][0]["type"], "output_text");
        assert_eq!(
            output[0]["content"][0]["text"],
            "The capital of France is Paris."
        );

        assert_eq!(result["usage"]["input_tokens"], 10);
        assert_eq!(result["usage"]["output_tokens"], 20);
        assert_eq!(result["usage"]["total_tokens"], 30);
    }

    #[test]
    fn test_tool_use_response() {
        let body = json!({
            "id": "msg_def456",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "get_weather",
                    "input": {"city": "Tokyo"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 15, "output_tokens": 30}
        });

        let result = messages_to_responses_response(&body);

        assert_eq!(result["status"], "completed");

        let output = result["output"].as_array().unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["type"], "function_call");
        assert_eq!(output[0]["call_id"], "toolu_123");
        assert_eq!(output[0]["name"], "get_weather");
        assert_eq!(output[0]["arguments"], "{\"city\":\"Tokyo\"}");
    }

    #[test]
    fn test_thinking_response() {
        let body = json!({
            "id": "msg_think789",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {"type": "thinking", "thinking": "Let me think about this..."},
                {"type": "text", "text": "Here is my answer."}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 50}
        });

        let result = messages_to_responses_response(&body);

        let output = result["output"].as_array().unwrap();
        assert_eq!(output.len(), 2);

        // First: reasoning item
        assert_eq!(output[0]["type"], "reasoning");
        assert_eq!(output[0]["summary"][0]["type"], "summary_text");
        assert_eq!(
            output[0]["summary"][0]["text"],
            "Let me think about this..."
        );

        // Second: message with text
        assert_eq!(output[1]["type"], "message");
        assert_eq!(output[1]["content"][0]["type"], "output_text");
        assert_eq!(output[1]["content"][0]["text"], "Here is my answer.");
    }

    #[test]
    fn test_max_tokens_status() {
        let body = json!({
            "id": "msg_max",
            "type": "message",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "Truncated..."}],
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 10, "output_tokens": 4096}
        });

        let result = messages_to_responses_response(&body);
        assert_eq!(result["status"], "incomplete");
    }

    #[test]
    fn test_mixed_content_response() {
        let body = json!({
            "id": "msg_mixed",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {"type": "text", "text": "I'll check the weather."},
                {"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {"city": "NYC"}},
                {"type": "tool_use", "id": "toolu_2", "name": "get_time", "input": {"tz": "EST"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "output_tokens": 40}
        });

        let result = messages_to_responses_response(&body);
        let output = result["output"].as_array().unwrap();

        // Should be: message (text), function_call, function_call
        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[0]["content"][0]["text"], "I'll check the weather.");
        assert_eq!(output[1]["type"], "function_call");
        assert_eq!(output[1]["name"], "get_weather");
        assert_eq!(output[2]["type"], "function_call");
        assert_eq!(output[2]["name"], "get_time");
    }

    // ── SSE conversion tests ─────────────────────────────────────────────

    #[test]
    fn test_sse_message_start() {
        let data = json!({
            "type": "message_start",
            "message": {
                "id": "msg_sse1",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-20250514",
                "content": [],
                "usage": {"input_tokens": 42, "output_tokens": 0}
            }
        });

        let events = messages_sse_to_responses_sse("message_start", &data);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "response.created");
        assert_eq!(events[1].0, "response.in_progress");

        let resp = &events[0].1["response"];
        assert_eq!(resp["id"], "resp_sse1");
        assert_eq!(resp["model"], "claude-sonnet-4-20250514");
        assert_eq!(resp["usage"]["input_tokens"], 42);
    }

    #[test]
    fn test_sse_text_content_block() {
        // content_block_start (text)
        let start_data = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        });
        let start_events = messages_sse_to_responses_sse("content_block_start", &start_data);

        assert_eq!(start_events.len(), 2);
        assert_eq!(start_events[0].0, "response.output_item.added");
        assert_eq!(start_events[1].0, "response.content_part.added");

        // content_block_delta (text)
        let delta_data = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hello world"}
        });
        let delta_events = messages_sse_to_responses_sse("content_block_delta", &delta_data);

        assert_eq!(delta_events.len(), 1);
        assert_eq!(delta_events[0].0, "response.output_text.delta");
        assert_eq!(delta_events[0].1["delta"], "Hello world");

        // content_block_stop
        let stop_data = json!({"type": "content_block_stop", "index": 0});
        let stop_events = messages_sse_to_responses_sse("content_block_stop", &stop_data);

        assert_eq!(stop_events.len(), 2);
        assert_eq!(stop_events[0].0, "response.content_part.done");
        assert_eq!(stop_events[1].0, "response.output_item.done");
    }

    #[test]
    fn test_sse_tool_use_block() {
        let start_data = json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_abc",
                "name": "get_weather"
            }
        });
        let events = messages_sse_to_responses_sse("content_block_start", &start_data);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "response.output_item.added");
        assert_eq!(events[0].1["item"]["type"], "function_call");
        assert_eq!(events[0].1["item"]["call_id"], "toolu_abc");
        assert_eq!(events[0].1["item"]["name"], "get_weather");

        // arguments delta
        let delta_data = json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"city\":"}
        });
        let delta_events = messages_sse_to_responses_sse("content_block_delta", &delta_data);

        assert_eq!(delta_events.len(), 1);
        assert_eq!(delta_events[0].0, "response.function_call_arguments.delta");
        assert_eq!(delta_events[0].1["delta"], "{\"city\":");
    }

    #[test]
    fn test_sse_thinking_block() {
        let start_data = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "thinking"}
        });
        let events = messages_sse_to_responses_sse("content_block_start", &start_data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1["item"]["type"], "reasoning");

        let delta_data = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "Let me reason..."}
        });
        let delta_events = messages_sse_to_responses_sse("content_block_delta", &delta_data);
        assert_eq!(delta_events.len(), 1);
        assert_eq!(delta_events[0].0, "response.reasoning.delta");
        assert_eq!(delta_events[0].1["delta"], "Let me reason...");
    }

    #[test]
    fn test_sse_message_delta_completed() {
        let data = json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 100}
        });
        let events = messages_sse_to_responses_sse("message_delta", &data);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "response.completed");
        assert_eq!(events[0].1["response"]["status"], "completed");
        assert_eq!(events[0].1["response"]["usage"]["output_tokens"], 100);
    }

    #[test]
    fn test_sse_message_delta_incomplete() {
        let data = json!({
            "type": "message_delta",
            "delta": {"stop_reason": "max_tokens"},
            "usage": {"output_tokens": 4096}
        });
        let events = messages_sse_to_responses_sse("message_delta", &data);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "response.incomplete");
        assert_eq!(events[0].1["response"]["status"], "incomplete");
    }

    #[test]
    fn test_sse_message_stop_is_noop() {
        let events = messages_sse_to_responses_sse("message_stop", &json!({}));
        assert!(events.is_empty());
    }

    #[test]
    fn test_convert_full_sse_stream() {
        let sse_text = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-20250514\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";

        let result = convert_messages_sse_stream(sse_text).unwrap();

        // Should contain all the key Responses events
        assert!(result.contains("response.created"));
        assert!(result.contains("response.in_progress"));
        assert!(result.contains("response.output_item.added"));
        assert!(result.contains("response.content_part.added"));
        assert!(result.contains("response.output_text.delta"));
        assert!(result.contains("response.content_part.done"));
        assert!(result.contains("response.output_item.done"));
        assert!(result.contains("response.completed"));
        // message_stop should not produce events
        assert!(!result.contains("response.stopped"));
    }

    // ── Phase 2: Messages → Responses request conversion tests ──────────

    #[test]
    fn test_p2_simple_text_request() {
        let body = json!({
            "model": "gpt-5.4",
            "messages": [
                {"role": "user", "content": "What is the capital of France?"}
            ],
            "max_tokens": 1024,
            "stream": false
        });

        let result = messages_to_responses_request(&body);

        assert_eq!(result["model"], "gpt-5.4");
        assert_eq!(result["max_output_tokens"], 1024);
        assert_eq!(result["stream"], false);

        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(
            input[0]["content"][0]["text"],
            "What is the capital of France?"
        );
    }

    #[test]
    fn test_p2_with_system() {
        let body = json!({
            "model": "gpt-5.4",
            "system": "You are a helpful assistant.",
            "messages": [{"role": "user", "content": "Hello"}],
            "max_tokens": 1024
        });

        let result = messages_to_responses_request(&body);

        assert_eq!(result["instructions"], "You are a helpful assistant.");
    }

    #[test]
    fn test_p2_system_array() {
        let body = json!({
            "model": "gpt-5.4",
            "system": [
                {"type": "text", "text": "You are helpful."},
                {"type": "text", "text": "Be concise."}
            ],
            "messages": [{"role": "user", "content": "Hello"}],
            "max_tokens": 1024
        });

        let result = messages_to_responses_request(&body);
        assert_eq!(result["instructions"], "You are helpful.\nBe concise.");
    }

    #[test]
    fn test_p2_with_tools() {
        let body = json!({
            "model": "gpt-5.4",
            "messages": [{"role": "user", "content": "What's the weather?"}],
            "max_tokens": 1024,
            "tools": [{
                "name": "get_weather",
                "description": "Get weather for a city",
                "input_schema": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"]
                }
            }],
            "tool_choice": {"type": "auto"}
        });

        let result = messages_to_responses_request(&body);

        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "get_weather");
        assert_eq!(tools[0]["description"], "Get weather for a city");
        assert!(tools[0].get("parameters").is_some());

        assert_eq!(result["tool_choice"], "auto");
    }

    #[test]
    fn test_p2_tool_choice_any_to_required() {
        let body = json!({
            "model": "gpt-5.4",
            "messages": [{"role": "user", "content": "Hello"}],
            "max_tokens": 1024,
            "tool_choice": {"type": "any"}
        });

        let result = messages_to_responses_request(&body);
        assert_eq!(result["tool_choice"], "required");
    }

    #[test]
    fn test_p2_with_tool_use_and_result() {
        let body = json!({
            "model": "gpt-5.4",
            "messages": [
                {"role": "user", "content": "What's the weather in Tokyo?"},
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_123",
                        "name": "get_weather",
                        "input": {"city": "Tokyo"}
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_123",
                        "content": "Sunny, 25°C"
                    }]
                }
            ],
            "max_tokens": 1024
        });

        let result = messages_to_responses_request(&body);

        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);

        // First: user message
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");

        // Second: function_call
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "toolu_123");
        assert_eq!(input[1]["name"], "get_weather");
        assert_eq!(input[1]["arguments"], "{\"city\":\"Tokyo\"}");

        // Third: function_call_output
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "toolu_123");
        assert_eq!(input[2]["output"], "Sunny, 25°C");
    }

    #[test]
    fn test_p2_assistant_text_message() {
        let body = json!({
            "model": "gpt-5.4",
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": [{"type": "text", "text": "Hi there!"}]},
                {"role": "user", "content": "Follow up"}
            ],
            "max_tokens": 1024
        });

        let result = messages_to_responses_request(&body);
        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[1]["content"][0]["text"], "Hi there!");
    }

    // ── Phase 2: Responses → Messages response conversion tests ─────────

    #[test]
    fn test_p2_simple_text_response() {
        let body = json!({
            "id": "resp_abc123",
            "object": "response",
            "model": "gpt-5.4",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Paris is the capital."}]
            }],
            "usage": {"input_tokens": 10, "output_tokens": 8}
        });

        let result = responses_to_messages_response(&body);

        assert_eq!(result["id"], "msg_abc123");
        assert_eq!(result["type"], "message");
        assert_eq!(result["role"], "assistant");
        assert_eq!(result["model"], "gpt-5.4");
        assert_eq!(result["stop_reason"], "end_turn");

        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Paris is the capital.");

        assert_eq!(result["usage"]["input_tokens"], 10);
        assert_eq!(result["usage"]["output_tokens"], 8);
        // No total_tokens in Messages format
        assert!(
            result["usage"].get("total_tokens").is_none()
                || result["usage"]["total_tokens"].is_null()
        );
    }

    #[test]
    fn test_p2_function_call_response() {
        let body = json!({
            "id": "resp_def456",
            "object": "response",
            "model": "gpt-5.4",
            "status": "completed",
            "output": [{
                "type": "function_call",
                "call_id": "call_789",
                "name": "get_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            }],
            "usage": {"input_tokens": 15, "output_tokens": 20}
        });

        let result = responses_to_messages_response(&body);

        assert_eq!(result["stop_reason"], "tool_use");

        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "call_789");
        assert_eq!(content[0]["name"], "get_weather");
        assert_eq!(content[0]["input"]["city"], "Tokyo");
    }

    #[test]
    fn test_p2_reasoning_response() {
        let body = json!({
            "id": "resp_reason",
            "object": "response",
            "model": "gpt-5.4",
            "status": "completed",
            "output": [
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "Thinking about this..."}]
                },
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "The answer is 42."}]
                }
            ],
            "usage": {"input_tokens": 5, "output_tokens": 30}
        });

        let result = responses_to_messages_response(&body);

        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Thinking about this...");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "The answer is 42.");
    }

    #[test]
    fn test_p2_incomplete_response() {
        let body = json!({
            "id": "resp_inc",
            "object": "response",
            "model": "gpt-5.4",
            "status": "incomplete",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "Truncated..."}]
            }],
            "usage": {"input_tokens": 10, "output_tokens": 4096}
        });

        let result = responses_to_messages_response(&body);
        assert_eq!(result["stop_reason"], "max_tokens");
    }

    // ── Phase 2: Responses SSE → Messages SSE conversion tests ──────────

    #[test]
    fn test_p2_sse_response_created() {
        let data = json!({
            "type": "response.created",
            "response": {
                "id": "resp_sse1",
                "model": "gpt-5.4",
                "status": "in_progress",
                "output": [],
                "usage": {"input_tokens": 42, "output_tokens": 0}
            }
        });

        let events = responses_sse_to_messages_sse("response.created", &data);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "message_start");

        let msg = &events[0].1["message"];
        assert_eq!(msg["id"], "msg_sse1");
        assert_eq!(msg["model"], "gpt-5.4");
        assert_eq!(msg["usage"]["input_tokens"], 42);
    }

    #[test]
    fn test_p2_sse_output_text_delta() {
        let data = json!({
            "type": "response.output_text.delta",
            "output_index": 0,
            "content_index": 0,
            "delta": "Hello world"
        });

        let events = responses_sse_to_messages_sse("response.output_text.delta", &data);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "content_block_delta");
        assert_eq!(events[0].1["delta"]["type"], "text_delta");
        assert_eq!(events[0].1["delta"]["text"], "Hello world");
    }

    #[test]
    fn test_p2_sse_function_call_arguments_delta() {
        let data = json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 1,
            "delta": "{\"city\":"
        });

        let events = responses_sse_to_messages_sse("response.function_call_arguments.delta", &data);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "content_block_delta");
        assert_eq!(events[0].1["delta"]["type"], "input_json_delta");
        assert_eq!(events[0].1["delta"]["partial_json"], "{\"city\":");
    }

    #[test]
    fn test_p2_sse_output_item_added_message() {
        let data = json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "message",
                "role": "assistant",
                "content": []
            }
        });

        let events = responses_sse_to_messages_sse("response.output_item.added", &data);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "content_block_start");
        assert_eq!(events[0].1["content_block"]["type"], "text");
    }

    #[test]
    fn test_p2_sse_output_item_added_function_call() {
        let data = json!({
            "type": "response.output_item.added",
            "output_index": 1,
            "item": {
                "type": "function_call",
                "call_id": "call_abc",
                "name": "get_weather",
                "arguments": ""
            }
        });

        let events = responses_sse_to_messages_sse("response.output_item.added", &data);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "content_block_start");
        assert_eq!(events[0].1["content_block"]["type"], "tool_use");
        assert_eq!(events[0].1["content_block"]["id"], "call_abc");
        assert_eq!(events[0].1["content_block"]["name"], "get_weather");
    }

    #[test]
    fn test_p2_sse_response_completed() {
        let data = json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "usage": {"output_tokens": 100}
            }
        });

        let events = responses_sse_to_messages_sse("response.completed", &data);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "message_delta");
        assert_eq!(events[0].1["delta"]["stop_reason"], "end_turn");
        assert_eq!(events[0].1["usage"]["output_tokens"], 100);
        assert_eq!(events[1].0, "message_stop");
    }

    #[test]
    fn test_p2_sse_response_incomplete() {
        let data = json!({
            "type": "response.incomplete",
            "response": {
                "status": "incomplete",
                "usage": {"output_tokens": 4096}
            }
        });

        let events = responses_sse_to_messages_sse("response.incomplete", &data);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "message_delta");
        assert_eq!(events[0].1["delta"]["stop_reason"], "max_tokens");
        assert_eq!(events[1].0, "message_stop");
    }

    #[test]
    fn test_p2_convert_full_responses_sse_stream() {
        let sse_text = "\
event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\
\n\
event: response.output_item.added\n\
data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\
\n\
event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\
\n\
event: response.output_item.done\n\
data: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\
\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"output_tokens\":5}}}\n\
\n";

        let result = convert_responses_sse_stream(sse_text).unwrap();

        assert!(result.contains("message_start"));
        assert!(result.contains("content_block_start"));
        assert!(result.contains("content_block_delta"));
        assert!(result.contains("text_delta"));
        assert!(result.contains("content_block_stop"));
        assert!(result.contains("message_delta"));
        assert!(result.contains("message_stop"));
        assert!(result.contains("end_turn"));
    }

    // ── Streaming adapter tests ─────────────────────────────────────────

    #[tokio::test]
    async fn test_streaming_messages_to_responses_single_chunk() {
        use bytes::Bytes;
        use futures::StreamExt;

        let sse_text = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-20250514\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";

        let input_stream =
            futures::stream::once(
                async move { Ok::<Bytes, std::io::Error>(Bytes::from(sse_text)) },
            );

        let output_stream = create_responses_sse_stream_from_messages(input_stream);
        tokio::pin!(output_stream);

        let mut collected = String::new();
        while let Some(Ok(bytes)) = output_stream.next().await {
            collected.push_str(&String::from_utf8_lossy(&bytes));
        }

        assert!(collected.contains("response.created"));
        assert!(collected.contains("response.in_progress"));
        assert!(collected.contains("response.output_item.added"));
        assert!(collected.contains("response.output_text.delta"));
        assert!(collected.contains("response.completed"));
    }

    #[tokio::test]
    async fn test_streaming_messages_to_responses_multi_chunk() {
        use bytes::Bytes;
        use futures::StreamExt;

        // Split the SSE stream across multiple chunks, including mid-event splits
        let chunk1 = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_t\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";
        let chunk2 = "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n";
        let chunk3 = "\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n";
        let chunk4 = "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

        let input_stream = futures::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(chunk1)),
            Ok(Bytes::from(chunk2)),
            Ok(Bytes::from(chunk3)),
            Ok(Bytes::from(chunk4)),
        ]);

        let output_stream = create_responses_sse_stream_from_messages(input_stream);
        tokio::pin!(output_stream);

        let mut collected = String::new();
        while let Some(Ok(bytes)) = output_stream.next().await {
            collected.push_str(&String::from_utf8_lossy(&bytes));
        }

        assert!(collected.contains("response.created"));
        assert!(collected.contains("response.output_text.delta"));
        assert!(collected.contains("response.completed"));
    }

    #[tokio::test]
    async fn test_streaming_responses_to_messages_single_chunk() {
        use bytes::Bytes;
        use futures::StreamExt;

        let sse_text = "\
event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"object\":\"response\",\"model\":\"gpt-4o\",\"status\":\"in_progress\",\"output\":[],\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\
\n\
event: response.output_item.added\n\
data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"item_0\",\"role\":\"assistant\",\"content\":[]}}\n\
\n\
event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"item_id\":\"item_0\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\
\n\
event: response.output_item.done\n\
data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"item_0\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}}\n\
\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"object\":\"response\",\"model\":\"gpt-4o\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"item_0\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}],\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\
\n";

        let input_stream =
            futures::stream::once(
                async move { Ok::<Bytes, std::io::Error>(Bytes::from(sse_text)) },
            );

        let output_stream = create_messages_sse_stream_from_responses(input_stream);
        tokio::pin!(output_stream);

        let mut collected = String::new();
        while let Some(Ok(bytes)) = output_stream.next().await {
            collected.push_str(&String::from_utf8_lossy(&bytes));
        }

        assert!(collected.contains("message_start"));
        assert!(collected.contains("content_block_start"));
        assert!(collected.contains("content_block_delta"));
        assert!(collected.contains("content_block_stop"));
        assert!(collected.contains("message_delta"));
        assert!(collected.contains("message_stop"));
    }

    #[tokio::test]
    async fn test_streaming_responses_to_messages_multi_chunk() {
        use bytes::Bytes;
        use futures::StreamExt;

        let chunk1 = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_t\",\"object\":\"response\",\"model\":\"gpt-4o\",\"status\":\"in_progress\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";
        let chunk2 = "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"item_0\",\"role\":\"assistant\",\"content\":[]}}\n";
        let chunk3 = "\nevent: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"item_0\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hi\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_t\",\"object\":\"response\",\"model\":\"gpt-4o\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n";

        let input_stream = futures::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(chunk1)),
            Ok(Bytes::from(chunk2)),
            Ok(Bytes::from(chunk3)),
        ]);

        let output_stream = create_messages_sse_stream_from_responses(input_stream);
        tokio::pin!(output_stream);

        let mut collected = String::new();
        while let Some(Ok(bytes)) = output_stream.next().await {
            collected.push_str(&String::from_utf8_lossy(&bytes));
        }

        assert!(collected.contains("message_start"));
        assert!(collected.contains("content_block_delta"));
        assert!(collected.contains("message_stop"));
    }

    #[tokio::test]
    async fn test_streaming_error_forwarded() {
        use bytes::Bytes;
        use futures::StreamExt;

        let input_stream = futures::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_e\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            )),
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection lost")),
        ]);

        let output_stream = create_responses_sse_stream_from_messages(input_stream);
        tokio::pin!(output_stream);

        let mut collected = String::new();
        while let Some(Ok(bytes)) = output_stream.next().await {
            collected.push_str(&String::from_utf8_lossy(&bytes));
        }

        // Should have the converted first event plus an error event
        assert!(collected.contains("response.created"));
        assert!(collected.contains("event: error"));
        assert!(collected.contains("stream_error"));
        assert!(collected.contains("connection lost"));
    }

    #[tokio::test]
    async fn test_streaming_no_trailing_newline() {
        use bytes::Bytes;
        use futures::StreamExt;

        // Stream ends without trailing \n\n — flush logic should handle it
        let sse_text = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_f\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}";

        let input_stream =
            futures::stream::once(
                async move { Ok::<Bytes, std::io::Error>(Bytes::from(sse_text)) },
            );

        let output_stream = create_responses_sse_stream_from_messages(input_stream);
        tokio::pin!(output_stream);

        let mut collected = String::new();
        while let Some(Ok(bytes)) = output_stream.next().await {
            collected.push_str(&String::from_utf8_lossy(&bytes));
        }

        assert!(collected.contains("response.created"));
    }

    // ── Phase 3: Responses → Chat Completions request tests ─────────────

    #[test]
    fn test_p3_simple_text_request() {
        let body = json!({
            "model": "deepseek-chat",
            "input": "What is the capital of France?",
            "max_output_tokens": 1024,
            "stream": false
        });

        let result = responses_to_chat_completions_request(&body);

        assert_eq!(result["model"], "deepseek-chat");
        assert_eq!(result["max_tokens"], 1024);
        assert_eq!(result["stream"], false);

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "What is the capital of France?");
    }

    #[test]
    fn test_p3_with_instructions() {
        let body = json!({
            "model": "deepseek-chat",
            "input": "Hello",
            "instructions": "You are a helpful assistant."
        });

        let result = responses_to_chat_completions_request(&body);

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a helpful assistant.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello");
    }

    #[test]
    fn test_p3_with_tools() {
        let body = json!({
            "model": "deepseek-chat",
            "input": "What's the weather?",
            "tools": [{
                "type": "function",
                "name": "get_weather",
                "description": "Get weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"]
                }
            }],
            "tool_choice": "auto"
        });

        let result = responses_to_chat_completions_request(&body);

        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "get_weather");
        assert_eq!(
            tools[0]["function"]["description"],
            "Get weather for a city"
        );
        assert!(tools[0]["function"].get("parameters").is_some());

        assert_eq!(result["tool_choice"], "auto");
    }

    #[test]
    fn test_p3_with_function_call_and_output() {
        let body = json!({
            "model": "deepseek-chat",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "What's the weather in Tokyo?"
                },
                {
                    "type": "function_call",
                    "call_id": "call_123",
                    "name": "get_weather",
                    "arguments": "{\"city\":\"Tokyo\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_123",
                    "output": "Sunny, 25°C"
                }
            ]
        });

        let result = responses_to_chat_completions_request(&body);

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);

        // User message
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "What's the weather in Tokyo?");

        // Assistant with tool_calls
        assert_eq!(messages[1]["role"], "assistant");
        let tool_calls = messages[1]["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_123");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
        assert_eq!(
            tool_calls[0]["function"]["arguments"],
            "{\"city\":\"Tokyo\"}"
        );

        // Tool response
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_123");
        assert_eq!(messages[2]["content"], "Sunny, 25°C");
    }

    #[test]
    fn test_p3_consecutive_function_calls_grouped() {
        let body = json!({
            "model": "deepseek-chat",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "Compare weather in Tokyo and London"
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "get_weather",
                    "arguments": "{\"city\":\"Tokyo\"}"
                },
                {
                    "type": "function_call",
                    "call_id": "call_2",
                    "name": "get_weather",
                    "arguments": "{\"city\":\"London\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "Sunny, 25°C"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_2",
                    "output": "Rainy, 12°C"
                }
            ]
        });

        let result = responses_to_chat_completions_request(&body);

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);

        // User message
        assert_eq!(messages[0]["role"], "user");

        // Single assistant message with both tool_calls grouped together
        assert_eq!(messages[1]["role"], "assistant");
        let tool_calls = messages[1]["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0]["id"], "call_1");
        assert_eq!(tool_calls[1]["id"], "call_2");

        // Two tool responses
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_1");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call_2");
    }

    #[test]
    fn test_p3_array_input_with_content_blocks() {
        let body = json!({
            "model": "deepseek-chat",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Hello there"}]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Hi!"}]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": "Follow-up"
                }
            ]
        });

        let result = responses_to_chat_completions_request(&body);
        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello there");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "Hi!");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "Follow-up");
    }

    #[test]
    fn test_p3_tool_choice_required() {
        let body = json!({
            "model": "deepseek-chat",
            "input": "Hello",
            "tool_choice": "required"
        });

        let result = responses_to_chat_completions_request(&body);
        assert_eq!(result["tool_choice"], "required");
    }

    #[test]
    fn test_p3_tool_choice_none() {
        let body = json!({
            "model": "deepseek-chat",
            "input": "Hello",
            "tool_choice": "none"
        });

        let result = responses_to_chat_completions_request(&body);
        assert_eq!(result["tool_choice"], "none");
    }

    // ── Phase 3: Chat Completions → Responses response tests ────────────

    #[test]
    fn test_p3_simple_text_response() {
        let body = json!({
            "id": "chatcmpl-abc123",
            "object": "chat.completion",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "The capital of France is Paris."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        });

        let result = chat_completions_to_responses_response(&body);

        assert_eq!(result["id"], "resp_chatcmpl-abc123");
        assert_eq!(result["object"], "response");
        assert_eq!(result["model"], "deepseek-chat");
        assert_eq!(result["status"], "completed");

        let output = result["output"].as_array().unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[0]["content"][0]["type"], "output_text");
        assert_eq!(
            output[0]["content"][0]["text"],
            "The capital of France is Paris."
        );

        assert_eq!(result["usage"]["input_tokens"], 10);
        assert_eq!(result["usage"]["output_tokens"], 20);
        assert_eq!(result["usage"]["total_tokens"], 30);
    }

    #[test]
    fn test_p3_tool_calls_response() {
        let body = json!({
            "id": "chatcmpl-def456",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 15, "completion_tokens": 30, "total_tokens": 45}
        });

        let result = chat_completions_to_responses_response(&body);

        assert_eq!(result["status"], "completed");

        let output = result["output"].as_array().unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["type"], "function_call");
        assert_eq!(output[0]["call_id"], "call_123");
        assert_eq!(output[0]["name"], "get_weather");
        assert_eq!(output[0]["arguments"], "{\"city\":\"Tokyo\"}");
    }

    #[test]
    fn test_p3_length_finish_reason() {
        let body = json!({
            "id": "chatcmpl-ghi789",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Partial output..."},
                "finish_reason": "length"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 100, "total_tokens": 110}
        });

        let result = chat_completions_to_responses_response(&body);
        assert_eq!(result["status"], "incomplete");
    }

    // ── Phase 3: Chat Completions SSE → Responses SSE tests ─────────────

    #[test]
    fn test_p3_sse_first_chunk_emits_created() {
        let mut state = ChatCompletionsStreamState::new();

        let chunk = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": ""},
                "finish_reason": null
            }]
        });

        let events = chat_completions_sse_to_responses_sse(&chunk, &mut state);

        // Should emit response.created, response.in_progress, output_item.added, content_part.added
        let event_types: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
        assert!(event_types.contains(&"response.created"));
        assert!(event_types.contains(&"response.in_progress"));
        assert!(event_types.contains(&"response.output_item.added"));
        assert!(event_types.contains(&"response.content_part.added"));
        assert!(state.created);
    }

    #[test]
    fn test_p3_sse_text_delta() {
        let mut state = ChatCompletionsStreamState::new();

        // First chunk to initialize
        let first = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}]
        });
        chat_completions_sse_to_responses_sse(&first, &mut state);

        // Content delta
        let chunk = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{"index": 0, "delta": {"content": "Hello"}, "finish_reason": null}]
        });
        let events = chat_completions_sse_to_responses_sse(&chunk, &mut state);

        let has_text_delta = events
            .iter()
            .any(|(e, _)| e == "response.output_text.delta");
        assert!(has_text_delta);

        let delta_event = events
            .iter()
            .find(|(e, _)| e == "response.output_text.delta")
            .unwrap();
        assert_eq!(delta_event.1["delta"], "Hello");
    }

    #[test]
    fn test_p3_sse_tool_calls() {
        let mut state = ChatCompletionsStreamState::new();

        // First chunk with tool_calls
        let chunk1 = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": ""}
                    }]
                },
                "finish_reason": null
            }]
        });
        let events1 = chat_completions_sse_to_responses_sse(&chunk1, &mut state);

        let has_item_added = events1
            .iter()
            .any(|(e, _)| e == "response.output_item.added");
        assert!(has_item_added);

        // Check that the function_call item was added
        let added_event = events1
            .iter()
            .find(|(e, d)| {
                e == "response.output_item.added" && d["item"]["type"] == "function_call"
            })
            .unwrap();
        assert_eq!(added_event.1["item"]["call_id"], "call_abc");
        assert_eq!(added_event.1["item"]["name"], "get_weather");

        // Arguments delta
        let chunk2 = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": "{\"city\":"}
                    }]
                },
                "finish_reason": null
            }]
        });
        let events2 = chat_completions_sse_to_responses_sse(&chunk2, &mut state);

        let has_args_delta = events2
            .iter()
            .any(|(e, _)| e == "response.function_call_arguments.delta");
        assert!(has_args_delta);
    }

    #[test]
    fn test_p3_sse_finish_stop() {
        let mut state = ChatCompletionsStreamState::new();

        // Initialize with content
        let init = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Hi"}, "finish_reason": null}]
        });
        chat_completions_sse_to_responses_sse(&init, &mut state);

        // Finish with stop
        let finish = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        });
        let events = chat_completions_sse_to_responses_sse(&finish, &mut state);

        let event_types: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
        assert!(event_types.contains(&"response.content_part.done"));
        assert!(event_types.contains(&"response.output_item.done"));
        assert!(event_types.contains(&"response.completed"));

        let completed = events
            .iter()
            .find(|(e, _)| e == "response.completed")
            .unwrap();
        assert_eq!(completed.1["response"]["status"], "completed");
    }

    #[test]
    fn test_p3_sse_finish_tool_calls() {
        let mut state = ChatCompletionsStreamState::new();

        // Initialize with tool call
        let init = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "foo", "arguments": ""}}]
                },
                "finish_reason": null
            }]
        });
        chat_completions_sse_to_responses_sse(&init, &mut state);

        // Finish with tool_calls
        let finish = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
        });
        let events = chat_completions_sse_to_responses_sse(&finish, &mut state);

        let event_types: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
        assert!(event_types.contains(&"response.output_item.done"));
        assert!(event_types.contains(&"response.completed"));
    }

    #[test]
    fn test_p3_sse_finish_length() {
        let mut state = ChatCompletionsStreamState::new();

        let init = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Partial"}, "finish_reason": null}]
        });
        chat_completions_sse_to_responses_sse(&init, &mut state);

        let finish = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-chat",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "length"}]
        });
        let events = chat_completions_sse_to_responses_sse(&finish, &mut state);

        let event_types: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
        assert!(event_types.contains(&"response.incomplete"));

        let incomplete = events
            .iter()
            .find(|(e, _)| e == "response.incomplete")
            .unwrap();
        assert_eq!(incomplete.1["response"]["status"], "incomplete");
    }

    // ── Phase 3: Stream adapter test ────────────────────────────────────

    #[tokio::test]
    async fn test_p3_cc_stream_adapter() {
        use bytes::Bytes;
        use futures::stream;

        let sse_text = "\
data: {\"id\":\"chatcmpl-123\",\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"chatcmpl-123\",\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"chatcmpl-123\",\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: [DONE]\n\n";

        let input_stream = stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(sse_text))]);

        let output_stream = create_responses_sse_stream_from_chat_completions(input_stream);
        tokio::pin!(output_stream);

        let mut collected = String::new();
        while let Some(Ok(bytes)) = output_stream.next().await {
            collected.push_str(&String::from_utf8_lossy(&bytes));
        }

        assert!(collected.contains("response.created"));
        assert!(collected.contains("response.in_progress"));
        assert!(collected.contains("response.output_item.added"));
        assert!(collected.contains("response.output_text.delta"));
        assert!(collected.contains("response.completed"));
    }

    #[test]
    fn test_p3_drain_cc_sse_events() {
        let mut buffer = String::from(
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n\
             data: [DONE]\n\n",
        );

        let events = drain_cc_sse_events(&mut buffer);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["id"], "chatcmpl-1");
        assert!(buffer.is_empty());
    }
}
