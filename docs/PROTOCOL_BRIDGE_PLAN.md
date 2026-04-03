# Rausu — Protocol Bridge Plan (Responses ↔ Messages)

## 目标

任意客户端 × 任意模型，Rausu 自动协议转换。

## 完整矩阵

| 客户端 | 发的协议 | 想用的模型 | Rausu 需要做的 | 现状 |
|--------|---------|-----------|---------------|------|
| Claude Code | `/v1/messages` (Anthropic) | Claude (Copilot) | 透传 | ✅ 已通 |
| Claude Code | `/v1/messages` (Anthropic) | Claude (Anthropic) | 透传 | ✅ 已通 |
| Claude Code | `/v1/messages` (Anthropic) | GPT (ChatGPT sub) | Messages→Responses 转换 | ❌ Phase 2 |
| Claude Code | `/v1/messages` (Anthropic) | GPT (Copilot) | Messages→Responses 转换 | ❌ Phase 2 |
| Codex CLI | `/v1/responses` (OpenAI) | GPT (ChatGPT sub) | 透传 | ✅ 已通 |
| Codex CLI | `/v1/responses` (OpenAI) | GPT (Copilot) | 透传 | ✅ 已通 |
| Codex CLI | `/v1/responses` (OpenAI) | Claude (Copilot) | Responses→Messages 转换 | ❌ Phase 1 |
| Codex CLI | `/v1/responses` (OpenAI) | Claude (Anthropic) | Responses→Messages 转换 | ❌ Phase 1 |

## 协议差异概览

### Responses API (OpenAI) vs Messages API (Anthropic)

| 概念 | Responses API | Messages API |
|------|---------------|--------------|
| 入口 | `POST /v1/responses` | `POST /v1/messages` |
| 用户输入 | `input` (string 或 array of items) | `messages` (array of {role, content}) |
| 系统指令 | `instructions` | `system` |
| 输出 | `output` (array of items) | `content` (array of blocks) |
| 文本输出 | `{type: "output_text", text}` | `{type: "text", text}` |
| 工具调用 | `{type: "function_call", name, arguments, call_id}` | `{type: "tool_use", name, input, id}` |
| 工具结果 | `{type: "function_call_output", call_id, output}` | `{type: "tool_result", tool_use_id, content}` |
| 最大 token | `max_output_tokens` | `max_tokens` |
| 停止原因 | `status: "completed"` | `stop_reason: "end_turn"` |
| 思考链 | `{type: "reasoning", summary}` | `{type: "thinking", thinking}` |
| 流式格式 | Named events (response.created → output_text.delta → response.completed) | Named events (message_start → content_block_delta → message_stop) |

## 实现方案

### 架构

```
src/
  transform/
    mod.rs                      — 模块入口
    responses_to_messages.rs    — Responses 请求 → Messages 请求
    messages_to_responses.rs    — Messages 响应 → Responses 响应
    messages_to_responses_stream.rs — Messages SSE → Responses SSE
    responses_to_messages_request.rs — (反向：Messages 请求 → Responses 请求, Phase 2)
    responses_to_messages_response.rs — (反向：Responses 响应 → Messages 响应, Phase 2)
```

### Phase 1: Codex CLI 用 Claude 模型（Responses→Messages）

**触发条件：** Copilot/Anthropic provider 的 `proxy_responses()` 检测到 Claude 模型

**请求转换 (Responses → Messages):**
```
input (string)         → messages: [{role: "user", content: input}]
input (array of items) → 遍历提取:
  - message items      → messages (按 role 分组)
  - function_call_output → tool_result block
instructions           → system
model                  → model
stream                 → stream
max_output_tokens      → max_tokens (默认 8192)
temperature            → temperature
tools (function type)  → tools (Anthropic 格式: name, description, input_schema)
tool_choice            → tool_choice (映射: required→any, auto→auto)
reasoning.effort       → thinking (映射 effort → budget_tokens)
```

**响应转换 (Messages → Responses):**
```
id: "msg_xxx"          → id: "resp_xxx"
content blocks:
  - {type: "text"}     → output: [{type: "message", content: [{type: "output_text"}]}]
  - {type: "tool_use"} → output: [{type: "function_call", call_id, name, arguments}]
  - {type: "thinking"} → output: [{type: "reasoning", summary: [{type: "summary_text"}]}]
stop_reason            → status (end_turn→completed, max_tokens→incomplete, tool_use→completed)
usage                  → usage (+ total_tokens)
```

**流式转换 (Messages SSE → Responses SSE):**
```
message_start          → response.created + response.in_progress
content_block_start    → response.output_item.added + response.content_part.added
content_block_delta:
  - text_delta         → response.output_text.delta
  - input_json_delta   → response.function_call_arguments.delta
content_block_stop     → response.content_part.done + response.output_item.done
message_delta          → response.completed / response.incomplete
message_stop           → (已在 message_delta 处理)
```

### Phase 2: Claude Code 用 GPT 模型（Messages→Responses）

Phase 1 的反向。触发条件：ChatGPT/Copilot provider 的 `proxy_messages()` 检测到 GPT 模型。

转换逻辑与 Phase 1 互为反函数。

## 参考代码

cc-switch 项目 (`github.com/farion1231/cc-switch`) 有完整实现：
- `src-tauri/src/proxy/providers/transform_responses.rs` (~1000行) — 非流式转换
- `src-tauri/src/proxy/providers/streaming_responses.rs` (~1000行) — 流式转换
- 本地参考副本: `~/.openclaw/workspace-supercoder/tmp/cc-switch/`

注意 cc-switch 的转换方向与我们 Phase 1 需要的**相反**（cc-switch 是 Messages→Responses），需要反向实现。

## 开发顺序

1. Phase 1A: 非流式 Responses→Messages 转换 + 测试
2. Phase 1B: 流式 Messages SSE → Responses SSE 转换 + 测试
3. Phase 1C: Copilot provider `proxy_responses()` 集成
4. Phase 2A: 非流式 Messages→Responses 转换 + 测试
5. Phase 2B: 流式 Responses SSE → Messages SSE 转换 + 测试
6. Phase 2C: ChatGPT provider `proxy_messages()` 集成

## 工具调用映射

Codex CLI 的 tool calling 对 agent 功能至关重要，不能省略。

| Responses (OpenAI) | Messages (Anthropic) |
|---|---|
| `{type: "function_call", call_id, name, arguments: "json_string"}` | `{type: "tool_use", id, name, input: {json_object}}` |
| `{type: "function_call_output", call_id, output: "string"}` | `{type: "tool_result", tool_use_id, content: "string"}` |
| `tools: [{type: "function", name, description, parameters}]` | `tools: [{name, description, input_schema}]` |

注意：Responses 的 `arguments` 是 JSON 字符串，Messages 的 `input` 是 JSON 对象。

## 状态映射

| Responses status | Messages stop_reason | 条件 |
|---|---|---|
| `completed` | `end_turn` | 正常完成 |
| `completed` | `tool_use` | 有工具调用 |
| `incomplete` | `max_tokens` | 超出 token 上限 |
| `cancelled` | - | 用户取消 |

## 日期
- 计划确认：2026-04-03
- 参考项目分析完成：2026-04-03
