# Google Vertex AI Provider

> **中文版:** [VERTEX_AI_PROVIDER_CN.md](VERTEX_AI_PROVIDER_CN.md)

## Overview

The `vertex-ai` provider routes OpenAI-compatible chat completions through Google's Vertex AI Gemini models. Rausu translates between the OpenAI Chat Completions format and Gemini's `generateContent` / `streamGenerateContent` API automatically.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | ✅ (streaming + non-streaming) |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/messages` | ❌ Use `claude-subscription` for Anthropic Messages |
| `POST /v1/responses` | ❌ Use `openai` or `chatgpt-subscription` |

## Prerequisites

1. A GCP project with the **Vertex AI API** enabled
2. Claude models or Gemini models enabled in [Model Garden](https://console.cloud.google.com/vertex-ai/model-garden)
3. One of these authentication methods configured:
   - **Application Default Credentials (ADC)** — via `gcloud auth application-default login`
   - **Service Account JSON** — downloaded from GCP IAM

## Authentication

### Option A: Application Default Credentials (recommended for local dev)

```bash
gcloud auth application-default login
```

This writes credentials to `~/.config/gcloud/application_default_credentials.json`. Rausu reads this automatically.

### Option B: Service Account JSON (recommended for production/Docker)

1. Create a service account in GCP IAM with the **Vertex AI User** role
2. Download the JSON key file
3. Reference it in config:

```yaml
credentials_path: "/path/to/service-account.json"
```

Or set the environment variable:
```bash
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
```

### Credential resolution order

1. `credentials_path` in config (explicit)
2. `GOOGLE_APPLICATION_CREDENTIALS` environment variable
3. `~/.config/gcloud/application_default_credentials.json` (default ADC)

## Quick start

### 1. Configure Rausu

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

### 2. Start Rausu

```bash
./rausu --config config.yaml
```

### 3. Test

```bash
curl http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gemini-2.5-pro",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Claude Code CLI

This is the primary use case: point Claude Code at Rausu, and Rausu proxies to Vertex AI Gemini models.

### Step-by-step setup

**1. Prepare GCP credentials**

```bash
# Option A: ADC (interactive login)
gcloud auth application-default login

# Option B: Service account (set env var)
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
```

**2. Create `config.yaml`**

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty

models:
  # Map a model name that Claude Code will request
  - name: claude-sonnet-4-20250514
    providers:
      - provider: vertex-ai
        model: gemini-2.5-pro-preview-05-06
        project_id: "your-gcp-project-id"
        location: "us-central1"
```

> **Tip:** Name the model something Claude Code expects (e.g. `claude-sonnet-4-20250514`) so you don't need to override the model name in Claude Code.

**3. Start Rausu**

```bash
./rausu --config config.yaml
```

**4. Point Claude Code at Rausu**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="fake-key"   # Rausu ignores this, but Claude Code requires it
claude -p "Hello from Vertex AI via Rausu"
```

> **Important:** Claude Code sends requests to `/v1/messages` (Anthropic Messages API), not `/v1/chat/completions`. The Vertex AI provider currently supports `/v1/chat/completions` only. To use Claude Code with Vertex AI through Rausu, you would need a model name that Claude Code requests via the chat completions path, or use a different client that speaks the OpenAI protocol.

### For OpenAI-compatible clients (Codex CLI, curl, SDKs)

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"

# Codex CLI
codex --model gemini-2.5-pro

# Or any OpenAI SDK
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

## Docker deployment

```bash
docker run \
  -v /path/to/application_default_credentials.json:/app/adc.json \
  -v /path/to/config.yaml:/app/config.yaml \
  -e GOOGLE_APPLICATION_CREDENTIALS=/app/adc.json \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: vertex-ai
      model: <gemini-model-id>           # Required
      project_id: <gcp-project-id>       # Required
      location: <gcp-region>             # Required (default: us-central1)
      credentials_path: <path>           # Optional (falls back to env/ADC)
```

### Upstream model names

| Model ID | Description |
|---|---|
| `gemini-2.5-pro-preview-05-06` | Gemini 2.5 Pro |
| `gemini-2.0-flash-001` | Gemini 2.0 Flash |
| `gemini-1.5-pro-002` | Gemini 1.5 Pro |
| `gemini-1.5-flash-002` | Gemini 1.5 Flash |

Check [Vertex AI Model Garden](https://console.cloud.google.com/vertex-ai/model-garden) for the latest model IDs.

### Location values

| Value | Description |
|---|---|
| `us-central1` | US Central (default, recommended) |
| `europe-west4` | Netherlands |
| `asia-southeast1` | Singapore |
| `global` | Global endpoint (lower latency in some regions) |

See [Vertex AI locations](https://cloud.google.com/vertex-ai/generative-ai/docs/learn/locations) for the full list.

## Format translation

Rausu automatically translates between OpenAI and Gemini formats:

| OpenAI field | Gemini field |
|---|---|
| `messages[role=system]` | `systemInstruction` |
| `messages[role=user]` | `contents[role=user]` |
| `messages[role=assistant]` | `contents[role=model]` |
| `temperature` | `generationConfig.temperature` |
| `max_tokens` | `generationConfig.maxOutputTokens` |
| `top_p` | `generationConfig.topP` |
| `stop` | `generationConfig.stopSequences` |

## Known limitations

- **No tool/function calling translation** — Gemini's function calling format differs from OpenAI's; left for a future phase.
- **Text content only** — image/audio parts in messages are silently skipped.
- **No Claude-on-Vertex** — only Gemini models via `/publishers/google/models/`. For Claude on Vertex, use a native Anthropic provider.
- **No embeddings, images, or audio endpoints** — only chat completions.
