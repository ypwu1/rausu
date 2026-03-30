# OpenAI Provider

> **English version:** [OPENAI_PROVIDER.md](OPENAI_PROVIDER.md)

## 概述

`openai` provider 使用 API Key 将请求路由到 OpenAI API。它支持 OpenAI Chat Completions 和 Responses API，并接受你的 API Key 有权访问的任何模型。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | ✅（流式 + 非流式） |
| `POST /v1/responses` | ✅ 原生 Responses API 直传 |
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
- 速率限制和模型可用性由 OpenAI 控制 — Rausu 原样传递上游 HTTP 状态码。
- 工具/函数调用以 Chat Completions 格式原样透传，不做额外转换。
