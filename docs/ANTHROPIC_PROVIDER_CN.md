# Anthropic Provider

> **English version:** [ANTHROPIC_PROVIDER.md](ANTHROPIC_PROVIDER.md)

## 概述

`anthropic` provider 使用 API Key 将请求路由到 Anthropic API。它同时接受 OpenAI Chat Completions 格式（自动转换为 Anthropic Messages API 格式）以及通过 `/v1/messages` 提交的原生 Anthropic Messages API 请求。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/messages` | ✅ 原生 Anthropic Messages API 直传 |
| `POST /v1/chat/completions` | ✅（流式 + 非流式，转换为 Messages API） |
| `GET /v1/models` | ✅ 列出已配置的模型名 |
| `POST /v1/responses` | ❌ 请使用 `provider: openai` 或 `provider: chatgpt-subscription` |

## 前提条件

需要一个 [Anthropic API Key](https://console.anthropic.com/settings/keys)。

## 认证

在 `config.yaml` 中或通过环境变量设置 API Key：

```yaml
api_key: "${ANTHROPIC_API_KEY}"
```

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

## 快速开始

### 1. 添加到 config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: claude-sonnet
    providers:
      - provider: anthropic
        model: claude-sonnet-4-20250514
        api_key: "${ANTHROPIC_API_KEY}"
```

### 2. 启动 Rausu

```bash
rausu --config config.yaml
```

### 3. 使用 curl 测试（Messages API）

```bash
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet",
    "max_tokens": 256,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

### 4. 使用 curl 测试（Chat Completions 格式）

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "claude-sonnet",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 与 Claude Code CLI 配合使用

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="fake-key"   # Rausu 会忽略此值；真实 Key 在 config.yaml 中
claude -p "Hello via Rausu"
```

Claude Code 发送请求到 `/v1/messages`，此 provider 原生支持该端点。

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: anthropic
      model: <anthropic-模型-id>   # 必填
      api_key: <你的-api-key>      # 必填
```

> **注意：** 此 provider 不支持 `base_url` 覆盖。请求始终发送到 `https://api.anthropic.com/v1/messages`。

## 格式转换

当请求到达 `/v1/chat/completions` 时，Rausu 会自动将其转换为 Anthropic Messages API 格式：

| OpenAI Chat Completions | Anthropic Messages API |
|---|---|
| `messages[role=system]` | 顶层 `system` 字段 |
| `messages[role=user/assistant]` | `messages` 数组 |
| `max_tokens` | `max_tokens`（未指定时默认为 4096） |
| `temperature` | `temperature` |
| `stop` | `stop_sequences` |
| `tools` / `functions` | `tools` |
| 停止原因 `end_turn` | `finish_reason: stop` |
| 停止原因 `max_tokens` | `finish_reason: length` |
| 停止原因 `tool_use` | `finish_reason: tool_calls` |

## 上游模型名

你的 Anthropic 账号可访问的任何模型均可使用。常见示例：

| 模型 ID | 说明 |
|---|---|
| `claude-opus-4-20250514` | Claude Opus 4（能力最强） |
| `claude-sonnet-4-20250514` | Claude Sonnet 4 |
| `claude-haiku-4-20250514` | Claude Haiku 4（最快） |
| `claude-sonnet-4-5-20251001` | Claude Sonnet 4.5 |
| `claude-haiku-3-20240307` | Claude 3 Haiku（旧版） |

完整模型 ID 列表请参阅 [Anthropic 模型文档](https://docs.anthropic.com/en/docs/about-claude/models)。

## Docker 部署

```bash
docker run \
  -e ANTHROPIC_API_KEY="sk-ant-..." \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## 已知限制

- **不支持 `/v1/responses`。** 请使用 `provider: openai` 或 `provider: chatgpt-subscription`。
- 速率限制和模型可用性由 Anthropic 控制 — Rausu 原样传递上游 HTTP 状态码。
- 此 provider 不支持 `base_url` 配置字段。
- Chat Completions 格式中的图片/多模态内容可能无法正确转换 — 多模态请求请使用原生 Messages API 格式（`/v1/messages`）。
