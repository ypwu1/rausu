# OpenAI Provider

> **English version:** [OPENAI_PROVIDER.md](OPENAI_PROVIDER.md)

## 概述

`openai` provider 使用 API Key 将请求路由到 OpenAI API 或任意 **OpenAI 兼容**端点。它支持 Chat Completions 和 Responses API，并与任何实现 OpenAI Chat Completions 格式的服务兼容——DeepSeek、Qwen（阿里云 DashScope）、Ollama、GLM、Moonshot、百川、Yi、MiniMax 等均支持。

**Phase 3 协议桥接：** 当 Codex CLI 向通用 OpenAI 兼容服务发送 Responses API 请求（`/v1/responses`）时，Rausu 自动进行 Responses → Chat Completions 格式桥接。这意味着 Codex CLI 可以直接使用所有通过 `provider: openai` + `base_url` 接入的服务，无需任何客户端侧配置。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | ✅（流式 + 非流式） |
| `POST /v1/responses` | ✅ 原生直传（OpenAI）；Responses→ChatCompletions 桥接（通用服务） |
| `GET /v1/models` | ✅ 列出已配置的模型名 |
| `POST /v1/messages` | ❌ Anthropic Messages API 请使用 `provider: anthropic` |

## 前提条件

需要一个可以访问所需模型的 [OpenAI API Key](https://platform.openai.com/api-keys)。

## 认证

在 `config.yaml` 中或通过环境变量设置 API Key：

```yaml
api_key: "${OPENAI_API_KEY}"
```

```bash
export OPENAI_API_KEY="sk-..."
```

## 快速开始

### 1. 添加到 config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  - name: o3
    providers:
      - provider: openai
        model: o3
        api_key: "${OPENAI_API_KEY}"
```

### 2. 启动 Rausu

```bash
rausu --config config.yaml
```

### 3. 发送请求

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 与 Codex CLI 配合使用

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu 会忽略此值；真实 Key 在 config.yaml 中
codex --model o3
```

## 使用 Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gpt-4o",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: openai
      model: <openai-模型-id>     # 必填
      api_key: <你的-api-key>     # 必填
      base_url: <url>             # 可选；默认：https://api.openai.com/v1
```

### `base_url`

覆盖默认的 `https://api.openai.com/v1` 端点。可用于指向 OpenAI 兼容 API（Azure OpenAI、本地代理等）。

```yaml
base_url: "https://your-azure-endpoint.openai.azure.com/openai/deployments/gpt-4o"
```

## OpenAI 兼容 Provider（Phase 3）

任意具有 OpenAI 兼容 Chat Completions 端点的服务，通过设置 `base_url` 即可接入。当 Codex CLI 通过 Responses API 访问这些服务时，Rausu 自动进行 Responses → Chat Completions 桥接。

### DeepSeek

```yaml
models:
  - name: deepseek-chat
    providers:
      - provider: openai
        model: deepseek-chat
        base_url: https://api.deepseek.com/v1
        api_key: sk-xxx
  - name: deepseek-reasoner
    providers:
      - provider: openai
        model: deepseek-reasoner
        base_url: https://api.deepseek.com/v1
        api_key: sk-xxx
```

```bash
export OPENAI_BASE_URL=http://localhost:4000
codex --model deepseek-chat
```

### Qwen（阿里云 DashScope）

```yaml
models:
  - name: qwen-max
    providers:
      - provider: openai
        model: qwen-max
        base_url: https://dashscope.aliyuncs.com/compatible-mode/v1
        api_key: sk-xxx
  - name: qwen-plus
    providers:
      - provider: openai
        model: qwen-plus
        base_url: https://dashscope.aliyuncs.com/compatible-mode/v1
        api_key: sk-xxx
```

### Ollama（本地）

```yaml
models:
  - name: llama3
    providers:
      - provider: openai
        model: llama3
        base_url: http://localhost:11434/v1
        api_key: ollama   # Ollama 忽略此值；任意非空字符串均可
  - name: qwen2.5-coder
    providers:
      - provider: openai
        model: qwen2.5-coder:7b
        base_url: http://localhost:11434/v1
        api_key: ollama
```

```bash
export OPENAI_BASE_URL=http://localhost:4000
codex --model llama3
```

### 其他兼容服务

相同的配置模式适用于任意 OpenAI 兼容端点：

| Provider | `base_url` |
|---|---|
| Moonshot（Kimi） | `https://api.moonshot.cn/v1` |
| GLM（智谱 AI） | `https://open.bigmodel.cn/api/paas/v4` |
| Yi（零一万物） | `https://api.lingyiwanwu.com/v1` |
| MiniMax | `https://api.minimax.chat/v1` |
| 百川 | `https://api.baichuan-ai.com/v1` |
| Groq | `https://api.groq.com/openai/v1` |
| Together AI | `https://api.together.xyz/v1` |

## 上游模型名

你的 OpenAI 账号可访问的任何模型均可使用。常见示例：

| 模型 ID | 说明 |
|---|---|
| `gpt-4o` | GPT-4o（多模态） |
| `gpt-4o-mini` | GPT-4o Mini（快速、高性价比） |
| `o3` | o3 推理模型 |
| `o4-mini` | o4-mini 推理模型 |
| `gpt-4-turbo` | GPT-4 Turbo |

完整模型 ID 列表请参阅 [OpenAI 模型文档](https://platform.openai.com/docs/models)。

## Docker 部署

```bash
docker run \
  -e OPENAI_API_KEY="sk-..." \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## 已知限制

- **不支持 `/v1/messages`。** Anthropic 原生路由请使用 `provider: anthropic`。
- 速率限制和模型可用性由上游服务控制 — Rausu 原样传递上游 HTTP 状态码。
- 工具/函数调用以 Chat Completions 格式原样透传，不做额外转换。
- 通过 `base_url` 接入的通用服务必须支持 OpenAI Chat Completions API 格式，非标准格式的服务不受支持。
