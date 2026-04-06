# 工具兼容性与 Provider 能力检查 — 设计草案

> [English](TOOL_COMPATIBILITY_DRAFT.md)

> **状态：草案** — 本文档记录已达成共识的设计方向。尚无实现。细节可能随工作推进而调整。

## 背景

Rausu 是一个本地优先的 LLM 网关。Claude Code 和 Codex CLI 等客户端越来越依赖工具调用 — 发送 `tools` 数组、接收 `tool_use` / `function_call` 块、管理多轮工具循环。不同的 provider 和端点支持这些能力的不同子集。

目前，Rausu 的优先级故障转移路由基于可用性（可重试错误如 429、5xx、传输故障）来选择 provider，**不考虑** provider 是否实际支持请求所需的功能。带有 `tools` 的请求可能被路由到静默忽略 tools 数组的 provider，或者流式请求可能命中只支持同步响应的端点。

本草案定义了一个**最小工具兼容层** — 不是工具执行运行时，而是正确路由请求并在能力缺失时显式失败所需的感知能力。

## 设计原则

1. **工具感知透传，而非工具执行。** Rausu 在客户端和 provider 之间转发工具定义和工具调用结果。它不执行工具、不托管 MCP 服务器、不审批 shell 命令、不运行 agent 循环。
2. **显式失败优于静默降级。** 如果 provider 无法满足请求需求，Rausu 必须返回明确错误或故障转移到另一个 provider。绝不能静默剥离 `tools`、`tool_choice`、`parallel_tool_calls` 或相关字段。
3. **在路由时检查能力。** 在转发请求之前，Rausu 评估所选 provider 是否能处理该请求。这与现有的优先级故障转移循环集成。
4. **按需翻译，而非随意翻译。** 协议翻译（例如 `function_call` ↔ `tool_use`）发生在协议边界。如果客户端和 provider 使用相同协议，优先透传。

## 工具感知透传的含义

工具感知透传意味着 Rausu 对工具相关字段的**结构**有足够的理解，能够：

- **检测**请求是否需要工具调用能力（存在 `tools`、`tool_choice`、`tool_results` 等）
- **评估**目标 provider 是否支持这些能力
- **路由**请求到有能力的 provider，或显式失败
- **翻译**请求跨越协议边界时的工具相关字段（例如 OpenAI `function_call` ↔ Anthropic `tool_use`）
- **保留**不需要翻译时所有工具相关字段的完整性

这**不**意味着：

- Rausu 不代替客户端调用工具
- Rausu 不验证工具参数 schema
- Rausu 不在请求之间维护工具执行状态
- Rausu 不实现 MCP 宿主能力

## 能力模型

### 能力维度

每个 provider-端点组合声明一组能力。最小维度包括：

| 维度 | 描述 | 示例值 |
|------|------|--------|
| `messages_endpoint` | 支持 Anthropic Messages API（`/v1/messages`） | `true` / `false` |
| `responses_endpoint` | 支持 OpenAI Responses API（`/v1/responses`） | `true` / `false` |
| `chat_completions_endpoint` | 支持 OpenAI Chat Completions API（`/v1/chat/completions`） | `true` / `false` |
| `streaming` | 支持 SSE 流式响应 | `true` / `false` |
| `tools` | 支持工具定义和工具调用响应 | `true` / `false` |
| `parallel_tools` | 支持 `parallel_tool_calls`（单轮多工具调用） | `true` / `false` |
| `bridge_support` | 可通过协议翻译到达（例如 Messages→Responses） | `true` / `false` |

这些维度按 provider 实现**静态声明**，不在运行时发现。每个 provider 模块了解自身的能力。

### 请求需求

当请求到达时，Rausu 提取其隐含的**需求**：

| 请求信号 | 隐含需求 |
|----------|----------|
| `tools` 数组存在且非空 | `tools = true` |
| `parallel_tool_calls: true` | `parallel_tools = true` |
| `stream: true` | `streaming = true` |
| 到达 `/v1/messages` | `messages_endpoint = true`（或目标的 `bridge_support = true`） |
| 到达 `/v1/responses` | `responses_endpoint = true`（或目标的 `bridge_support = true`） |
| 到达 `/v1/chat/completions` | `chat_completions_endpoint = true`（或目标的 `bridge_support = true`） |

### 评估结果

将请求需求与 provider 能力对比时，结果为以下之一：

| 结果 | 含义 | 操作 |
|------|------|------|
| **通过（Pass）** | Provider 满足所有需求 | 转发请求 |
| **软失败（Soft fail）** | 当前 provider 无法满足此请求，但故障转移列表中的其他 provider 可能可以 | 跳过此 provider，继续故障转移循环 |
| **硬失败（Hard fail）** | 模型的 provider 列表中没有任何 provider 能满足此请求 | 返回 `422 Unprocessable Entity`，附带明确错误：`{"error": {"type": "unsupported_capability", "message": "...", "missing_capabilities": [...]}}` |

### 禁止静默降级规则

这是一条硬性规则，不是指导方针：

> **Rausu 绝不能静默剥离或修改工具相关字段来使请求适应 provider 的能力。**

此规则禁止的行为示例：

- 移除 `tools` 数组以发送给不支持工具的 provider
- 当客户端发送 `parallel_tool_calls: true` 时将其设为 `false`
- 因为目标 provider 不支持 `tool_choice` 而丢弃该字段
- 静默将工具调用转换为纯文本消息

如果 provider 无法按原样处理请求（在合法的协议翻译之后），请求必须故障转移或硬失败 — 绝不能静默降级。

## 与优先级故障转移路由的集成

当前 `src/server/routes/chat.rs` 中的故障转移循环工作方式：

```
for 每个 provider（按优先级顺序）:
    尝试发送请求
    如果可重试错误 → 尝试下一个 provider
    如果不可重试错误 → 返回错误
    如果成功 → 返回响应
```

能力检查在网络调用**之前**增加一个**预检步骤**：

```
for 每个 provider（按优先级顺序）:
    检查 provider 能力是否满足请求需求
    如果软失败 → 跳过 provider，记录警告，尝试下一个
    尝试发送请求
    如果可重试错误 → 尝试下一个 provider
    如果不可重试错误 → 返回错误
    如果成功 → 返回响应

如果所有 provider 均已耗尽 → 检查是否有能力软失败
    如果有 → 返回 422，附带 unsupported_capability 错误
    如果没有 → 如今天一样返回 503
```

这意味着：

- **能力软失败是可重试的。** 如果 provider A 不支持工具但 provider B 支持，请求路由到 provider B。这与现有故障转移模型透明协作。
- **能力检查在网络调用之前发生。** 不会向无法处理请求的 provider 浪费网络往返。
- **现有错误分类不变。** HTTP 429、5xx 和传输错误仍然像以前一样触发故障转移。能力检查是额外的、更早期的门控。

### 日志

能力相关的路由决策应使用与现有故障转移相同的日志级别：

- `INFO`："Checking capabilities for provider X"
- `WARN`："Provider X does not support tools, skipping (soft fail)"
- `ERROR`："No provider supports required capabilities: [tools, parallel_tools]"

## 为什么这对 Claude Code 和 Codex CLI 很重要

Claude Code 通过 `/v1/messages` 发送带有 `tools` 数组的请求，用于文件操作、shell 命令和其他 agent 能力。如果 Rausu 将 Claude Code 的请求路由到不支持工具的 provider，agent 循环会静默中断 — 客户端期望响应中包含工具调用块，但却收到纯文本。

Codex CLI 通过 `/v1/responses` 发送带有函数定义的请求。同样的问题适用：静默剥离工具意味着客户端的 agent 循环停滞或产生错误结果。

两个客户端都假设网关对工具语义是透明的。能力检查确保 Rausu 维护这一假设 — 要么路由到有能力的 provider，要么显式失败，使客户端能向用户展示有意义的错误信息。

## 分阶段实施计划

### Phase A：文档与能力模型（本文档）

- 定义能力维度和评估结果
- 建立禁止静默降级规则
- 记录与现有路由的集成点

### Phase B：请求需求提取

- 解析传入请求以提取能力需求
- 检测 `tools`、`parallel_tool_calls`、`stream` 和端点类型
- 生成 `RequestRequirements` 结构体

### Phase C：Provider 能力声明

- 每个 provider 实现通过 `Provider` trait 上的方法声明能力（例如 `fn capabilities(&self) -> ProviderCapabilities`）
- 能力按 provider 类型静态声明，非按请求

### Phase D：路由集成

- 向故障转移循环添加能力预检
- 实现软失败 / 硬失败逻辑
- 添加不支持能力的结构化错误响应
- 添加能力相关日志行

### Phase E：协议特定的工具翻译（如需要）

- 扩展现有协议桥接以处理工具特定的翻译边缘情况
- 确保 `function_call` ↔ `tool_use` 转换保留所有语义
- 处理 provider 特定的工具调用差异（例如不同的 JSON schema 格式）

## 非目标

以下明确**不在**本设计范围内：

- **内置工具执行** — Rausu 不运行工具。它转发工具定义和结果。工具执行是客户端的责任。
- **MCP 宿主运行时** — Rausu 不托管 MCP 服务器、管理工具注册表或中介工具发现。MCP 网关功能推迟到 Phase 5。
- **Shell 审批循环** — Rausu 不提示用户批准或拒绝工具调用。这是客户端的 UX 责任。
- **Agent 运行时** — Rausu 不实现多轮 agent 循环、工具结果注入或自主执行流程。它处理单次请求-响应周期。
- **运行时能力发现** — 能力在代码中静态声明。Rausu 不在启动时探测 provider 或查询能力端点。未来可能改变，但不在初始实现范围内。
- **工具参数验证** — Rausu 不验证工具参数是否匹配其声明的 JSON schema。Provider 和客户端处理此事。

## 开放问题

- 能力声明应该在 YAML 中按模型可配置，还是纯粹从 provider 类型派生？按 provider 静态声明更简单；按模型配置可处理同一 provider 对不同模型有不同能力集的边缘情况。
- 硬失败响应是否应包含哪些 provider 能满足请求的建议，以辅助调试？
- 能力硬失败的正确 HTTP 状态码是什么？`422 Unprocessable Entity` 语义上正确；`400 Bad Request` 对某些客户端更常规。
