# DeepSeek Provider

> **English version:** [DEEPSEEK_PROVIDER.md](DEEPSEEK_PROVIDER.md)

## 概述

`deepseek` provider 将请求路由到 [DeepSeek](https://www.deepseek.com)，该服务提供 OpenAI 兼容的 API。此 provider 通过 API Key 认证将请求转发到 `https://api.deepseek.com`（或自定义 base URL）。

**Base URL 说明：** DeepSeek 官方文档将 `https://api.deepseek.com` 列为主 base URL。`/v1` 前缀（`https://api.deepseek.com/v1`）也被接受，用于 OpenAI 客户端库兼容性，但 `/v1` **不是**模型版本信号。

**Responses API 桥接：** 当 Codex CLI 或其他客户端向 DeepSeek 支持的模型发送 Responses API 请求（`/v1/responses`）时，Rausu 自动进行 Responses -> Chat Completions 格式桥接，与 `openrouter`、`openai`、`moonshot` 和 `z-ai` provider 使用相同策略。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | 流式 + 非流式 |
| `POST /v1/responses` | Responses->ChatCompletions 桥接 |
| `GET /v1/models` | 列出已配置的模型名 |
| `POST /v1/messages` | 不支持（请使用 `provider: anthropic`） |

## 前提条件

需要一个 [DeepSeek](https://platform.deepseek.com) API Key。

## 认证

在 `config.yaml` 中或通过环境变量设置 API Key：

```yaml
api_key: "${DEEPSEEK_API_KEY}"
```

```bash
export DEEPSEEK_API_KEY="your-api-key-here"
```

## 快速开始

### 1. 添加到 config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: deepseek-chat
    providers:
      - provider: deepseek
        model: deepseek-chat
        api_key: "${DEEPSEEK_API_KEY}"
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
    "model": "deepseek-chat",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配合 Codex CLI 使用

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu 忽略此值；真实 Key 在 config.yaml 中
codex --model deepseek-chat
```

## 使用 Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-chat",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: deepseek
      model: <deepseek-model-id>         # 必填（如 "deepseek-chat"）
      api_key: <your-api-key>            # 必填
      base_url: <url>                    # 可选；默认: https://api.deepseek.com
```

### `model`

使用 DeepSeek 文档中列出的模型 ID。示例：

| 模型 ID | 说明 |
|---|---|
| `deepseek-chat` | 通用聊天模型 |
| `deepseek-reasoner` | 推理专注模型 (DeepSeek-R1) |

完整列表请参见 [DeepSeek API 文档](https://platform.deepseek.com/api-docs)。

### `base_url`

覆盖默认的 `https://api.deepseek.com` 端点。用于指向自托管的或替代的 DeepSeek 兼容代理。

**注意：** DeepSeek 也接受 `https://api.deepseek.com/v1` 用于 OpenAI 客户端库兼容性。`/v1` 路径不是模型版本信号。如果您使用自定义 base URL，provider 会在您提供的 URL 后追加 `/chat/completions`。

## 模型命名

config 中的虚拟 `name` 是客户端发送的名称。`model` 字段是上游 DeepSeek 模型 ID。您可以选择任何命名方式：

```yaml
# 方式 A：直接使用 DeepSeek 模型 ID
- name: deepseek-chat
  providers:
    - provider: deepseek
      model: deepseek-chat

# 方式 B：自定义别名
- name: my-reasoning-model
  providers:
    - provider: deepseek
      model: deepseek-reasoner
```

## 多 Provider 故障转移

DeepSeek 模型可以与其他 provider 一起参与 Rausu 的优先级故障转移：

```yaml
- name: my-model
  providers:
    - provider: openai          # 先尝试直连 OpenAI
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
    - provider: deepseek        # 失败后转到 DeepSeek
      model: deepseek-chat
      api_key: "${DEEPSEEK_API_KEY}"
```

## 能力感知路由

DeepSeek provider 向 Rausu 路由器声明以下能力：

| 能力 | 是否声明 |
|---|---|
| `chat_completions` | 是 |
| `streaming` | 是（SSE） |
| `responses_api` | 是（Responses → Chat Completions 桥接） |
| `tools` | 是（透传给 DeepSeek） |
| `response_format` | 是（透传给 DeepSeek） |

**路由工作方式：**

1. 请求到达时，路由器检查请求内容并确定所需能力。包含 `tools` 的请求需要 `tools` 能力；包含 `response_format` 的请求需要 `response_format` 能力。
2. 缺少任何所需能力的 provider 在**发起上游调用之前**就会被跳过。
3. 如果同一虚拟模型下还有其他已配置的 provider 支持所需能力，故障转移会继续到该 provider。
4. 如果**没有任何**已配置的 provider 支持所有所需能力，Rausu 会向客户端返回明确的错误，而不是静默降级或剥离字段。

### `unsupported_capability` 错误

当某个模型的所有 provider 因缺少所需能力而被跳过时，Rausu 返回：

- **HTTP 状态码：** `422 Unprocessable Entity`
- **`error.type`：** `unsupported_capability`
- **`error.code`：** `unsupported_capability`
- **`error.message`：** 说明缺少哪些能力

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

### 无静默降级策略

在 DeepSeek 路径上，Rausu **不会**静默剥离请求中的 `tools`、`tool_choice` 或 `response_format` 字段。如果所选上游模型不支持请求的能力，上游错误会原样传播给客户端。

> **注意：** 上述能力声明反映的是 Rausu 中 DeepSeek provider 向路由器暴露的内容。实际能力支持仍取决于通过 DeepSeek 选择的具体上游模型。

## Docker 部署

```bash
docker run \
  -e DEEPSEEK_API_KEY="your-key" \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## 故障排查

| 症状 | 原因 | 解决方法 |
|---|---|---|
| `401 Unauthorized` | 无效或缺失的 API Key | 确认 `DEEPSEEK_API_KEY` 已设置且有效 |
| `429 Too Many Requests` | 超出速率限制 | 降低请求频率或添加其他 provider 进行故障转移 |
| `404 Not Found` | 无效的模型 ID | 检查模型 ID 是否匹配 DeepSeek 可用模型 |
| Rausu 中找不到模型 | config `name` 与客户端请求不匹配 | 确保客户端发送的是 config 中准确的虚拟 `name` |

## 已知限制

- **不支持 `/v1/messages`。** DeepSeek 使用 OpenAI 兼容格式。如需 Anthropic Messages API 直传，请使用 `provider: anthropic` 或 `provider: claude-subscription`。
- **无原生 Responses API。** Rausu 自动进行 Responses -> Chat Completions 桥接。
- 速率限制和模型可用性由 DeepSeek 控制。Rausu 原样传播上游 HTTP 状态码。
- 工具/函数调用直接透传；不进行额外转换。能力取决于所选的上游模型。
