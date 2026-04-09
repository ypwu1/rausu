# AWS Bedrock 供应商

> **English version:** [AWS_BEDROCK_PROVIDER.md](AWS_BEDROCK_PROVIDER.md)

## 概述

`bedrock` 供应商通过 **Converse API** 将请求路由到 [AWS Bedrock](https://aws.amazon.com/bedrock/)，该 API 为所有 Bedrock 托管模型（Anthropic Claude、Amazon Nova/Titan、Meta Llama、Mistral、Cohere 等）提供统一接口。

此供应商在 OpenAI Chat Completions 格式和 Bedrock Converse API 格式之间进行转换，包括：

- **消息：** OpenAI 角色 → Bedrock `User`/`Assistant` 消息，`system` 单独字段
- **工具调用：** OpenAI `tools` / `tool_choice` ↔ Bedrock `toolConfig` / `toolChoice`
- **流式传输：** AWS EventStream 二进制编码 → SSE `ChatCompletionChunk` 格式
- **推理配置：** `temperature`、`max_tokens`、`top_p`、`stop` → Bedrock `inferenceConfig`

**认证：** 通过标准 AWS SDK 凭证链的 AWS SigV4 请求签名 — 配置中无需 API key。

**Responses API 桥接：** 当客户端向 Bedrock 支持的模型发送 Responses API 请求（`/v1/responses`）时，Rausu 自动将 Responses 桥接为 Chat Completions 格式。

## 支持矩阵

| 端点 | 支持情况 |
|---|---|
| `POST /v1/chat/completions` | 流式 + 非流式（通过 Converse API） |
| `POST /v1/responses` | Responses → ChatCompletions 桥接 |
| `GET /v1/models` | 列出配置的模型名称 |
| `POST /v1/messages` | 不支持（请使用 `provider: anthropic`） |

## 前提条件

1. 拥有 AWS 账户并为所需模型[启用 Bedrock 模型访问](https://docs.aws.amazon.com/bedrock/latest/userguide/model-access.html)。
2. 通过以下方式之一提供 AWS 凭证：
   - **环境变量：** `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY`（+ 可选 `AWS_SESSION_TOKEN`）
   - **共享凭证文件：** `~/.aws/credentials`
   - **IAM 角色：** 在 EC2、ECS、Lambda 等上自动获取
   - **AWS SSO：** 通过 `aws sso login` + `AWS_PROFILE`

## 认证

无需 `api_key` 字段。AWS SDK 自动处理凭证解析。

```bash
# 方式 1：环境变量
export AWS_ACCESS_KEY_ID="AKIA..."
export AWS_SECRET_ACCESS_KEY="..."
export AWS_REGION="us-east-1"

# 方式 2：AWS CLI 配置文件
export AWS_PROFILE="my-bedrock-profile"

# 方式 3：IAM 角色（在 EC2/ECS/Lambda 上自动获取 — 无需配置）
```

## 快速开始

### 1. 添加到 config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: claude-3-5-sonnet
    providers:
      - provider: bedrock
        model: anthropic.claude-3-5-sonnet-20241022-v2:0
        region: us-east-1
```

### 2. 启动 Rausu

```bash
rausu --config config.yaml
```

### 3. 发送请求

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-3-5-sonnet",
    "messages": [{"role": "user", "content": "你好！"}]
  }'
```

## 与 Codex CLI 配合使用

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="unused"
codex --model claude-3-5-sonnet
```

## 与 Claude Code 配合使用

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="unused"
claude --model claude-3-5-sonnet
```

## 使用 Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-3-5-sonnet",
    "input": "用一句话解释 AWS Bedrock。"
  }'
```

## 配置参考

```yaml
providers:
  - provider: bedrock            # 必填：供应商类型
    model: <bedrock-model-id>    # 必填：Bedrock 模型 ID
    region: <aws-region>         # 必填：AWS 区域（如 us-east-1）
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `provider` | 是 | 必须为 `"bedrock"` |
| `model` | 是 | Bedrock 模型 ID（见下方） |
| `region` | 是 | 模型可用的 AWS 区域 |

## 模型 ID 格式

Bedrock 模型 ID 遵循 `<厂商>.<模型名>-<版本>:<修订号>` 格式：

| 厂商 | 示例模型 ID |
|---|---|
| Anthropic | `anthropic.claude-3-5-sonnet-20241022-v2:0`、`anthropic.claude-3-5-haiku-20241022-v1:0` |
| Amazon | `amazon.nova-pro-v1:0`、`amazon.nova-lite-v1:0`、`amazon.titan-text-premier-v1:0` |
| Meta | `meta.llama3-1-70b-instruct-v1:0`、`meta.llama3-1-8b-instruct-v1:0` |
| Mistral | `mistral.mistral-large-2407-v1:0` |

## 请求/响应格式转换

### 消息

| OpenAI 格式 | Bedrock Converse 格式 |
|---|---|
| `role: "system"` 消息 | 单独的 `system` 参数，使用 `SystemContentBlock::Text` |
| `role: "user"` 消息 | `Message { role: User, content: [ContentBlock::Text] }` |
| `role: "assistant"` 消息 | `Message { role: Assistant, content: [ContentBlock::Text] }` |
| `role: "assistant"` 带 `tool_calls` | `Message { role: Assistant, content: [ContentBlock::ToolUse] }` |
| `role: "tool"` 消息 | `Message { role: User, content: [ContentBlock::ToolResult] }` |

### 工具调用

| OpenAI 格式 | Bedrock 格式 |
|---|---|
| `tools[].function.{name, description, parameters}` | `toolConfig.tools[].toolSpec.{name, description, inputSchema}` |
| `tool_choice: "auto"` | `toolChoice: Auto` |
| `tool_choice: "required"` | `toolChoice: Any` |
| `tool_choice: {type: "function", function: {name}}` | `toolChoice: Tool {name}` |
| `tool_choice: "none"` | 省略工具配置 |

### 停止原因映射

| Bedrock `stopReason` | OpenAI `finish_reason` |
|---|---|
| `EndTurn` | `stop` |
| `ToolUse` | `tool_calls` |
| `MaxTokens` | `length` |
| `StopSequence` | `stop` |
| `ContentFiltered` | `content_filter` |

## 多供应商故障转移

```yaml
models:
  - name: claude-sonnet
    providers:
      - provider: bedrock
        model: anthropic.claude-3-5-sonnet-20241022-v2:0
        region: us-east-1
      - provider: anthropic
        model: claude-3-5-sonnet-20241022
        api_key: "${ANTHROPIC_API_KEY}"
```

如果 Bedrock 返回 5xx、429（限流）或连接错误，Rausu 会自动故障转移到下一个供应商。

## 能力感知路由

`bedrock` 供应商声明以下能力：

| 能力 | 是否声明 |
|---|---|
| `chat_completions` | 是 |
| `streaming` | 是 |
| `responses_api` | 是（桥接） |
| `tools` | 是 |
| `response_format` | 否 |
| `messages_api` | 否 |

请求未声明的能力（如 `response_format`）将收到 422 错误或故障转移到支持该能力的供应商。

## 故障排除

### "region is required for bedrock"
在供应商配置中添加 `region: us-east-1`（或您所需的区域）。

### "access denied" / 403
- 验证 AWS 凭证有效：`aws sts get-caller-identity`
- 确保 IAM 用户/角色具有 `bedrock:InvokeModel` 和 `bedrock:InvokeModelWithResponseStream` 权限
- 检查模型是否在 Bedrock 控制台中为您的区域启用

### "not found" / 404
- 模型 ID 可能不正确 — 请查看 [Bedrock 模型 ID](https://docs.aws.amazon.com/bedrock/latest/userguide/model-ids.html)
- 该模型可能在您的区域不可用

### "throttled" / 429
- 已达到 Bedrock 的速率限制 — 如果配置了多供应商，Rausu 将自动重试下一个供应商

## 已知限制

- `response_format`（结构化输出 / JSON 模式）不支持通过 Converse API 转换 — 如需此功能请使用原生支持的供应商
- 消息中的图片/视觉内容不会被转换（仅提取纯文本内容部分）
- Bedrock Converse API 的 token 计数与 OpenAI 不同 — 报告的使用量数据来自 Bedrock
