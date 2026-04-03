//! Protocol bridge: Responses API ↔ Messages API conversion.
//!
//! Phase 1 converts Codex CLI's Responses API requests into Anthropic Messages
//! API format, and converts the Messages API responses back into Responses API
//! format — enabling Codex CLI to use Claude models through Copilot.

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
        let anthropic_tools: Vec<Value> = tools
            .iter()
            .filter_map(convert_tool_definition)
            .collect();
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
                warn!(item_type = item_type, "Unknown input item type in Responses request");
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
                "content": current_content.drain(..).collect::<Vec<_>>()
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
    let parameters = tool.get("parameters").cloned().unwrap_or(json!({"type": "object"}));

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
                            "content": message_content.drain(..).collect::<Vec<_>>()
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
                    let thinking = block
                        .get("thinking")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
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
    let cache_read = body.pointer("/usage/cache_read_input_tokens").and_then(|v| v.as_u64());
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
        ("response.created".to_string(), json!({"type": "response.created", "response": response_obj.clone()})),
        ("response.in_progress".to_string(), json!({"type": "response.in_progress", "response": response_obj})),
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
            let thinking = delta
                .get("thinking")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
pub fn format_responses_sse_events(events: &[(String, Value)]) -> Result<String, serde_json::Error> {
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
        assert_eq!(output[0]["summary"][0]["text"], "Let me think about this...");

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
}
