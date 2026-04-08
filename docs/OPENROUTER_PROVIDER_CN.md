# OpenRouter Provider

> **English version:** [OPENROUTER_PROVIDER.md](OPENROUTER_PROVIDER.md)

## 概述

`openrouter` provider 将请求路由到 [OpenRouter](https://openrouter.ai)——一个 LLM 聚合服务，通过单个 API Key 和统一的 OpenAI 兼容接口，提供对来自 OpenAI、Anthropic、Google、Meta、Mistral 等 100+ 模型的访问。

**为什么使用 OpenRouter？** 一个 OpenRouter API Key 即可访问多个上游模型，无需为每个供应商单独管理凭证。非常适合实验、成本对比，以及访问在您所在地区可能不可用的模型。

**Responses API 桥接：** 当 Codex CLI 或其他客户端向 OpenRouter 支持的模型发送 Responses API 请求（`/v1/responses`）时，Rausu 自动进行 Responses -> Chat Completions 格式桥接，与通用 `openai` provider 使用相同策略。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | 流式 + 非流式 |
| `POST /v1/responses` | Responses->ChatCompletions 桥接 |
| `GET /v1/models` | 列出已配置的模型名 |
| `POST /v1/messages` | 不支持（请使用 `provider: anthropic`） |

## 前提条件

需要一个 [OpenRouter API Key](https://openrouter.ai/keys)。免费 Key 有速率限制；付费 Key 提供更高吞吐量。

## 认证

在 `config.yaml` 中或通过环境变量设置 API Key：

```yaml
api_key: "${OPENROUTER_API_KEY}"
```

```bash
export OPENROUTER_API_KEY="sk-or-v1-..."
```

## 快速开始

### 1. 添加到 config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: openrouter-gpt-4o
    providers:
      - provider: openrouter
        model: openai/gpt-4o
        api_key: "${OPENROUTER_API_KEY}"

  - name: openrouter-claude-sonnet
    providers:
      - provider: openrouter
        model: anthropic/claude-sonnet-4
        api_key: "${OPENROUTER_API_KEY}"
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
    "model": "openrouter-gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配合 Codex CLI 使用

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu 忽略此值；真实 Key 在 config.yaml 中
codex --model openrouter-gpt-4o
```

## 使用 Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "openrouter-gpt-4o",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: openrouter
      model: <openrouter-model-id>    # 必填（如 "openai/gpt-4o"）
      api_key: <your-api-key>         # 必填
      base_url: <url>                 # 可选；默认: https://openrouter.ai/api/v1
```

### `model`

OpenRouter 模型 ID 使用 `provider/model` 格式。示例：

| 模型 ID | 说明 |
|---|---|
| `openai/gpt-4o` | OpenAI GPT-4o |
| `openai/o3` | OpenAI o3 推理模型 |
| `anthropic/claude-sonnet-4` | Anthropic Claude Sonnet 4 |
| `anthropic/claude-opus-4` | Anthropic Claude Opus 4 |
| `google/gemini-2.5-pro` | Google Gemini 2.5 Pro |
| `meta-llama/llama-4-maverick` | Meta Llama 4 Maverick |

完整列表请参见 [OpenRouter 模型目录](https://openrouter.ai/models)。

### `base_url`

覆盖默认的 `https://openrouter.ai/api/v1` 端点。用于指向自托管的 OpenRouter 兼容代理。

## 模型命名

config 中的虚拟 `name` 是客户端发送的名称。`model` 字段是上游 OpenRouter 模型 ID。您可以选择任何命名方式：

```yaml
# 方式 A：描述性名称
- name: gpt-4o
  providers:
    - provider: openrouter
      model: openai/gpt-4o

# 方式 B：带前缀的名称（避免与直接 provider 条目冲突）
- name: or-gpt-4o
  providers:
    - provider: openrouter
      model: openai/gpt-4o

# 方式 C：直接使用 OpenRouter ID
- name: openai/gpt-4o
  providers:
    - provider: openrouter
      model: openai/gpt-4o
```

## 多 Provider 故障转移

OpenRouter 模型可以与其他 provider 一起参与 Rausu 的优先级故障转移：

```yaml
- name: gpt-4o
  providers:
    - provider: openai          # 先尝试直连 OpenAI
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
    - provider: openrouter      # 失败后转到 OpenRouter
      model: openai/gpt-4o
      api_key: "${OPENROUTER_API_KEY}"
```

## 能力感知路由

OpenRouter provider 向 Rausu 路由器声明以下能力：

| 能力 | 是否声明 |
|---|---|
| `chat_completions` | 是 |
| `streaming` | 是（SSE） |
| `responses_api` | 是（Responses → Chat Completions 桥接） |
| `tools` | 是（透传给 OpenRouter） |
| `response_format` | 是（透传给 OpenRouter） |

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

在 OpenRouter 路径上，Rausu **不会**静默剥离请求中的 `tools`、`tool_choice` 或 `response_format` 字段。如果所选上游模型不支持请求的能力，上游错误会原样传播给客户端。

> **注意：** 上述能力声明反映的是 Rausu 中 OpenRouter provider 向路由器暴露的内容。实际能力支持仍取决于通过 OpenRouter 选择的具体上游模型。例如，声明了 `tools` 表示 Rausu 会将该字段转发给 OpenRouter，但不支持函数调用的模型仍会从 OpenRouter 返回错误。

## Docker 部署

```bash
docker run \
  -e OPENROUTER_API_KEY="sk-or-v1-..." \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## 故障排查

| 症状 | 原因 | 解决方法 |
|---|---|---|
| `401 Unauthorized` | 无效或缺失的 API Key | 确认 `OPENROUTER_API_KEY` 已设置且有效 |
| `402 Payment Required` | 额度不足 | 在 [openrouter.ai/credits](https://openrouter.ai/credits) 充值 |
| `429 Too Many Requests` | 超出速率限制 | 升级计划或添加其他 provider 进行故障转移 |
| `404 Not Found` | 无效的模型 ID | 检查模型 ID 格式：`provider/model` |
| Rausu 中找不到模型 | config `name` 与客户端请求不匹配 | 确保客户端发送的是 config 中准确的虚拟 `name` |

## 已知限制

- **不支持 `/v1/messages`。** OpenRouter 使用 OpenAI 兼容格式。如需 Anthropic Messages API 直传，请使用 `provider: anthropic` 或 `provider: claude-subscription`。
- **无原生 Responses API。** Rausu 自动进行 Responses -> Chat Completions 桥接。
- 速率限制和模型可用性由 OpenRouter 控制。Rausu 原样传播上游 HTTP 状态码。
- 工具/函数调用直接透传；不进行额外转换。能力取决于所选的上游模型。
