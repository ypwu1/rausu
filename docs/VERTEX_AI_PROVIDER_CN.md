# Google Vertex AI Provider

> **English version:** [VERTEX_AI_PROVIDER.md](VERTEX_AI_PROVIDER.md)

## 概述

`vertex-ai` provider 将 OpenAI 兼容的 chat completions 请求路由到 Google Vertex AI 的 Gemini 模型。Rausu 自动在 OpenAI Chat Completions 格式和 Gemini `generateContent` / `streamGenerateContent` API 之间进行双向转换。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | ✅（流式 + 非流式） |
| `GET /v1/models` | ✅ 列出已配置的模型名 |
| `POST /v1/messages` | ❌ Anthropic Messages API 请使用 `claude-subscription` |
| `POST /v1/responses` | ❌ 请使用 `openai` 或 `chatgpt-subscription` |

## 前提条件

1. 已启用 **Vertex AI API** 的 GCP 项目
2. 在 [Model Garden](https://console.cloud.google.com/vertex-ai/model-garden) 中启用了 Gemini 模型
3. 配置了以下认证方式之一：
   - **应用默认凭据 (ADC)** — 通过 `gcloud auth application-default login`
   - **服务账号 JSON** — 从 GCP IAM 下载

## 认证

### 方式 A：应用默认凭据（推荐本地开发使用）

```bash
gcloud auth application-default login
```

凭据会写入 `~/.config/gcloud/application_default_credentials.json`，Rausu 自动读取。

### 方式 B：服务账号 JSON（推荐生产/Docker 环境）

1. 在 GCP IAM 中创建服务账号，赋予 **Vertex AI User** 角色
2. 下载 JSON 密钥文件
3. 在配置中引用：

```yaml
credentials_path: "/path/to/service-account.json"
```

或设置环境变量：
```bash
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
```

### 凭据解析顺序

1. 配置中的 `credentials_path`（显式指定）
2. `GOOGLE_APPLICATION_CREDENTIALS` 环境变量
3. `~/.config/gcloud/application_default_credentials.json`（默认 ADC）

## 快速开始

### 1. 配置 Rausu

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: gemini-2.5-pro
    providers:
      - provider: vertex-ai
        model: gemini-2.5-pro-preview-05-06
        project_id: "your-gcp-project-id"
        location: "us-central1"
```

### 2. 启动 Rausu

```bash
./rausu --config config.yaml
```

### 3. 测试

```bash
curl http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gemini-2.5-pro",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配合 Claude Code CLI 使用

### 完整操作步骤

**1. 准备 GCP 凭据**

```bash
# 方式 A：ADC（交互式登录）
gcloud auth application-default login

# 方式 B：服务账号（设置环境变量）
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
```

**2. 创建 `config.yaml`**

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty

models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: vertex-ai
        model: gemini-2.5-pro-preview-05-06
        project_id: "your-gcp-project-id"
        location: "us-central1"
```

**3. 启动 Rausu**

```bash
./rausu --config config.yaml
```

**4. 将 Claude Code 指向 Rausu**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="fake-key"   # Rausu 会忽略此值，但 Claude Code 需要它非空
claude -p "Hello from Vertex AI via Rausu"
```

> **注意：** Claude Code 使用 `/v1/messages`（Anthropic Messages API），而 Vertex AI provider 当前只支持 `/v1/chat/completions`。要通过 Rausu 将 Claude Code 连接到 Vertex AI，需要使用 OpenAI 协议的客户端。

### 适用于 OpenAI 兼容客户端（Codex CLI、curl、SDK）

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"

# Codex CLI
codex --model gemini-2.5-pro

# 或任何 OpenAI SDK
curl http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gemini-2.5-pro",
    "messages": [
      {"role": "system", "content": "You are a helpful assistant."},
      {"role": "user", "content": "What is Vertex AI?"}
    ],
    "stream": true
  }'
```

## Docker 部署

```bash
docker run \
  -v /path/to/application_default_credentials.json:/app/adc.json \
  -v /path/to/config.yaml:/app/config.yaml \
  -e GOOGLE_APPLICATION_CREDENTIALS=/app/adc.json \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: vertex-ai
      model: <gemini-模型-id>           # 必填
      project_id: <gcp-项目-id>         # 必填
      location: <gcp-区域>              # 必填（默认：us-central1）
      credentials_path: <路径>          # 可选（回退到环境变量/ADC）
```

### 上游模型名

| 模型 ID | 说明 |
|---|---|
| `gemini-2.5-pro-preview-05-06` | Gemini 2.5 Pro |
| `gemini-2.0-flash-001` | Gemini 2.0 Flash |
| `gemini-1.5-pro-002` | Gemini 1.5 Pro |
| `gemini-1.5-flash-002` | Gemini 1.5 Flash |

最新模型 ID 请查看 [Vertex AI Model Garden](https://console.cloud.google.com/vertex-ai/model-garden)。

### Location 值

| 值 | 说明 |
|---|---|
| `us-central1` | 美国中部（默认，推荐） |
| `europe-west4` | 荷兰 |
| `asia-southeast1` | 新加坡 |
| `global` | 全球端点（某些区域延迟更低） |

完整列表参见 [Vertex AI 位置文档](https://cloud.google.com/vertex-ai/generative-ai/docs/learn/locations)。

## 格式转换

Rausu 自动在 OpenAI 和 Gemini 格式之间转换：

| OpenAI 字段 | Gemini 字段 |
|---|---|
| `messages[role=system]` | `systemInstruction` |
| `messages[role=user]` | `contents[role=user]` |
| `messages[role=assistant]` | `contents[role=model]` |
| `temperature` | `generationConfig.temperature` |
| `max_tokens` | `generationConfig.maxOutputTokens` |
| `top_p` | `generationConfig.topP` |
| `stop` | `generationConfig.stopSequences` |

## 已知限制

- **不支持工具/函数调用转换** — Gemini 的函数调用格式与 OpenAI 不同，留待后续阶段。
- **仅支持文本内容** — 消息中的图片/音频部分会被静默跳过。
- **不支持 Claude-on-Vertex** — 仅支持 `/publishers/google/models/` 下的 Gemini 模型。Claude on Vertex 请使用原生 Anthropic provider。
- **无 embeddings、images 或 audio 端点** — 仅支持 chat completions。
