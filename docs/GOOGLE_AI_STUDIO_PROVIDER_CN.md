# Google AI Studio Provider

> **English version:** [GOOGLE_AI_STUDIO_PROVIDER.md](GOOGLE_AI_STUDIO_PROVIDER.md)

## 概述

`google-ai-studio` provider 将请求路由到 [Google AI Studio](https://aistudio.google.com)，该服务为 Gemini 模型提供 OpenAI 兼容的端点。此 provider 通过 `x-goog-api-key` 头认证将请求转发到 `https://generativelanguage.googleapis.com/v1beta/openai`（或自定义 base URL）。

**认证说明：** Google AI Studio 使用 `x-goog-api-key` 头进行认证，**不是**标准的 `Authorization: Bearer` 方式。

**与 Vertex AI 的区别：** 此 provider 面向 Google AI Studio API Key（个人开发者/免费层访问）。企业级 GCP 部署（使用项目级 IAM 认证），请使用 `provider: vertex-ai`。

**Responses API 桥接：** 当 Codex CLI 或其他客户端向 Google AI Studio 支持的模型发送 Responses API 请求（`/v1/responses`）时，Rausu 自动进行 Responses -> Chat Completions 格式桥接，与 `openrouter`、`openai`、`moonshot`、`deepseek` 和 `z-ai` provider 使用相同策略。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | 流式 + 非流式 |
| `POST /v1/responses` | Responses->ChatCompletions 桥接 |
| `GET /v1/models` | 列出已配置的模型名 |
| `POST /v1/messages` | 不支持（请使用 `provider: anthropic`） |

## 前提条件

需要一个 [Google AI Studio](https://aistudio.google.com/apikey) API Key。

## 认证

在 `config.yaml` 中或通过环境变量设置 API Key：

```yaml
api_key: "${GOOGLE_AI_STUDIO_API_KEY}"
```

```bash
export GOOGLE_AI_STUDIO_API_KEY="your-api-key-here"
```

## 快速开始

### 1. 添加到 config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: gemini-2.0-flash
    providers:
      - provider: google-ai-studio
        model: gemini-2.0-flash
        api_key: "${GOOGLE_AI_STUDIO_API_KEY}"
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
    "model": "gemini-2.0-flash",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配合 Codex CLI 使用

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu 忽略此值；真实密钥在 config.yaml 中
codex --model gemini-2.0-flash
```

## 使用 Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-2.0-flash",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: google-ai-studio
      model: <gemini模型ID>              # 必填（如 "gemini-2.0-flash"）
      api_key: <你的API Key>             # 必填
      base_url: <url>                    # 可选；默认: https://generativelanguage.googleapis.com/v1beta/openai
```

### `model`

使用 Google AI Studio 文档中列出的模型 ID。示例：

| 模型 ID | 描述 |
|---|---|
| `gemini-2.5-pro` | 最强大的 Gemini 模型 |
| `gemini-2.5-flash` | 快速、均衡的 Gemini 模型 |
| `gemini-2.0-flash` | 上一代快速模型 |
| `gemini-2.0-flash-lite` | 轻量级快速模型 |

完整模型列表请参见 [Google AI Studio 文档](https://ai.google.dev/gemini-api/docs/models)。

### `base_url`

覆盖默认的 `https://generativelanguage.googleapis.com/v1beta/openai` 端点。用于指向替代代理。provider 会在你提供的 URL 后追加 `/chat/completions`。

## 模型命名

配置中的虚拟 `name` 是客户端发送的名称。`model` 字段是上游 Google AI Studio 模型 ID。你可以选择任何命名约定：

```yaml
# 方案 A：直接使用模型 ID
- name: gemini-2.0-flash
  providers:
    - provider: google-ai-studio
      model: gemini-2.0-flash

# 方案 B：自定义别名
- name: my-gemini
  providers:
    - provider: google-ai-studio
      model: gemini-2.5-pro
```

## 多 Provider 故障转移

Google AI Studio 模型可以参与 Rausu 基于优先级的故障转移，与其他 provider 协同工作：

```yaml
- name: my-model
  providers:
    - provider: openai          # 先尝试 OpenAI
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
    - provider: google-ai-studio  # 回退到 Google AI Studio
      model: gemini-2.0-flash
      api_key: "${GOOGLE_AI_STUDIO_API_KEY}"
```

## 能力感知路由

Google AI Studio provider 向 Rausu 路由器声明以下能力：

| 能力 | 已声明 |
|---|---|
| `chat_completions` | 是 |
| `streaming` | 是 (SSE) |
| `responses_api` | 是 (Responses -> Chat Completions 桥接) |
| `tools` | 是（透传给 Google AI Studio） |
| `response_format` | 是（透传给 Google AI Studio） |

**路由工作原理：**

1. 当请求到达时，路由器检查请求并确定所需的能力。包含 `tools` 的请求需要 `tools` 能力；包含 `response_format` 的请求需要 `response_format` 能力。
2. 缺少任何所需能力的 provider 在**发起上游调用之前**即被跳过。
3. 如果同一虚拟模型的其他已配置 provider 支持所需能力，故障转移将继续到那里。
4. 如果**没有**已配置的 provider 支持所有所需能力，Rausu 将返回明确的客户端错误，而不是静默降级或删除字段。

### `unsupported_capability` 错误

当所有 provider 因缺少能力而被跳过时，Rausu 返回：

- **HTTP 状态码：** `422 Unprocessable Entity`
- **`error.type`：** `unsupported_capability`
- **`error.code`：** `unsupported_capability`
- **`error.message`：** 指明缺少的能力

响应体示例：

```json
{
  "error": {
    "message": "No provider for model 'my-model' supports the required capabilities: tools",
    "type": "unsupported_capability",
    "code": "unsupported_capability"
  }
}
```

### 不静默降级策略

Rausu **不会**在 Google AI Studio 路径上静默删除请求中的 `tools`、`tool_choice` 或 `response_format` 字段。如果选定的上游模型不支持请求的能力，上游错误将原样传递给客户端。

> **注意：** 上述能力声明反映的是 Rausu 中 Google AI Studio provider 向路由器暴露的能力。实际能力支持仍取决于通过 Google AI Studio 选择的特定上游模型。

## Docker 部署

```bash
docker run \
  -e GOOGLE_AI_STUDIO_API_KEY="your-key" \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## 故障排除

| 症状 | 原因 | 修复 |
|---|---|---|
| `401 Unauthorized` | API Key 无效或缺失 | 验证 `GOOGLE_AI_STUDIO_API_KEY` 已设置且有效 |
| `429 Too Many Requests` | 超出速率限制 | 降低请求频率或添加其他 provider 进行故障转移 |
| `404 Not Found` | 模型 ID 无效 | 检查模型 ID 是否与 Google AI Studio 可用模型匹配 |
| Rausu 中未找到模型 | 配置 `name` 与客户端请求不匹配 | 确保客户端发送的是配置中的确切虚拟 `name` |

## 已知限制

- **不支持 `/v1/messages`。** Google AI Studio 使用 OpenAI 兼容格式。如需 Anthropic Messages API 透传，请使用 `provider: anthropic` 或 `provider: claude-subscription`。
- **没有原生 Responses API。** Rausu 自动进行 Responses -> Chat Completions 桥接。
- 速率限制和模型可用性由 Google 控制。Rausu 原样传递上游 HTTP 状态码。
- 工具/函数调用原样透传；不进行额外转换。能力取决于上游模型。
