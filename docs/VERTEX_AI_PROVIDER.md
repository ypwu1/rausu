# Google Vertex AI Provider

> **中文版:** [VERTEX_AI_PROVIDER_CN.md](VERTEX_AI_PROVIDER_CN.md)

## Overview

The `vertex-ai` provider routes OpenAI-compatible chat completions through Google's Vertex AI Gemini models. Rausu translates between the OpenAI Chat Completions format and Gemini's `generateContent` / `streamGenerateContent` API automatically.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | ✅ (streaming + non-streaming) — Gemini models |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/messages` | ✅ (streaming + non-streaming) — Claude models only |
| `POST /v1/responses` | ❌ Use `openai` or `chatgpt-subscription` |

## Prerequisites

1. A GCP project with the **Vertex AI API** enabled
2. Gemini and/or Claude models enabled in [Model Garden](https://console.cloud.google.com/vertex-ai/model-garden)
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

For **Claude models on Vertex** (native Anthropic Messages API, no format translation):

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty

models:
  # Claude on Vertex — /v1/messages requests are proxied transparently
  - name: claude-sonnet-4-6
    providers:
      - provider: vertex-ai
        model: claude-sonnet-4-6
        project_id: "your-gcp-project-id"
        location: "us-east5"
```

For **Gemini models on Vertex** (OpenAI Chat Completions, with format translation):

```yaml
models:
  - name: claude-sonnet-4-20250514   # alias Claude Code recognises
    providers:
      - provider: vertex-ai
        model: gemini-2.5-pro-preview-05-06
        project_id: "your-gcp-project-id"
        location: "us-central1"
```

> **Tip:** Use the exact Claude model ID (e.g. `claude-sonnet-4-6`) as the virtual name so Claude Code works without any extra configuration.

**3. Start Rausu**

```bash
./rausu --config config.yaml
```

**4. Point Claude Code at Rausu**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="fake-key"   # Rausu ignores this, but Claude Code requires it
claude -p "Hello from Claude on Vertex AI via Rausu"
```

Claude Code sends requests to `/v1/messages`. When the model name starts with `claude-`, the vertex-ai provider proxies the request transparently to the Anthropic publisher endpoint on Vertex AI, injecting GCP OAuth auth.

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
      model: <model-id>                  # Required (Claude or Gemini model ID)
      project_id: <gcp-project-id>       # Required
      location: <gcp-region>             # Required (default: us-central1)
      credentials_path: <path>           # Optional (falls back to env/ADC)
```

The provider auto-detects the model type by name:
- Names starting with `claude-` → Anthropic publisher, `/v1/messages` endpoint
- All other names → Google publisher, `/v1/chat/completions` endpoint

### Location values

| Value | Description |
|---|---|
| `us-central1` | US Central (default, recommended for Gemini) |
| `us-east5` | US East (recommended for Claude on Vertex) |
| `europe-west1` | Belgium |
| `europe-west4` | Netherlands |
| `asia-southeast1` | Singapore |
| `global` | Global endpoint (lower latency in some regions) |

See [Vertex AI locations](https://cloud.google.com/vertex-ai/generative-ai/docs/learn/locations) for the full list.

## Supported Models Reference

> **Key concepts:**
> - `name` — what the client sends (must match exactly, e.g., Claude Code sends `claude-sonnet-4-6`)
> - `aliases` — additional names that also route to this config entry
> - `model` — the actual model ID sent to Vertex AI
> - Pinned versions on Vertex use `@` (e.g., `claude-haiku-4-5@20251001`), but clients typically send `-` (e.g., `claude-haiku-4-5-20251001`) — use `aliases` to bridge this
> - Claude models use the `/v1/messages` endpoint (Anthropic Messages API, no format translation)
> - Gemini models use the `/v1/chat/completions` endpoint (OpenAI-compatible, with format translation)
> - Region availability varies by model; check [Model Garden](https://console.cloud.google.com/vertex-ai/model-garden) for supported regions

### Claude models (Anthropic publisher, `/v1/messages`)

| Config `name` | Config `aliases` | Config `model` (sent to Vertex) | Vertex Publisher | Notes |
|---|---|---|---|---|
| `claude-sonnet-4-6` | | `claude-sonnet-4-6` | anthropic | Latest Sonnet |
| `claude-opus-4-6` | | `claude-opus-4-6` | anthropic | Latest Opus |
| `claude-haiku-4-5` | `claude-haiku-4-5-20251001` | `claude-haiku-4-5@20251001` | anthropic | Fastest, cheapest |
| `claude-sonnet-4-5` | `claude-sonnet-4-5-20250929` | `claude-sonnet-4-5@20250929` | anthropic | Legacy Sonnet |
| `claude-opus-4-5` | `claude-opus-4-5-20251101` | `claude-opus-4-5@20251101` | anthropic | Legacy Opus |
| `claude-opus-4-1` | `claude-opus-4-1-20250805` | `claude-opus-4-1@20250805` | anthropic | Legacy |
| `claude-sonnet-4` | `claude-sonnet-4-20250514` | `claude-sonnet-4@20250514` | anthropic | Legacy |
| `claude-opus-4` | `claude-opus-4-20250514` | `claude-opus-4@20250514` | anthropic | Legacy |
| `claude-3-5-haiku` | `claude-3-5-haiku-20241022` | `claude-3-5-haiku@20241022` | anthropic | Deprecated |

Recommended regions for Claude on Vertex: `us-east5`, `europe-west1`, `asia-southeast1`.

### Gemini models (Google publisher, `/v1/chat/completions`)

| Config `name` | Config `model` (sent to Vertex) | Vertex Publisher | Notes |
|---|---|---|---|
| `gemini-2.5-pro` | `gemini-2.5-pro` | google | Latest Pro |
| `gemini-2.5-flash` | `gemini-2.5-flash` | google | Latest Flash |
| `gemini-2.0-flash` | `gemini-2.0-flash` | google | Previous gen |

Recommended region for Gemini: `us-central1`.

### Complete config example

```yaml
models:
  # Claude models (via /v1/messages)
  - name: claude-sonnet-4-6
    providers:
      - provider: vertex-ai
        model: claude-sonnet-4-6
        project_id: "my-project"
        location: "us-east5"
        credentials_path: /path/to/credentials.json

  - name: claude-opus-4-6
    providers:
      - provider: vertex-ai
        model: claude-opus-4-6
        project_id: "my-project"
        location: "us-east5"
        credentials_path: /path/to/credentials.json

  - name: claude-haiku-4-5
    aliases:
      - claude-haiku-4-5-20251001
    providers:
      - provider: vertex-ai
        model: "claude-haiku-4-5@20251001"
        project_id: "my-project"
        location: "us-east5"
        credentials_path: /path/to/credentials.json

  # Gemini models (via /v1/chat/completions)
  - name: gemini-2.5-pro
    providers:
      - provider: vertex-ai
        model: gemini-2.5-pro
        project_id: "my-project"
        location: "us-central1"
        credentials_path: /path/to/credentials.json
```

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

- **No tool/function calling translation for Gemini** — Gemini's function calling format differs from OpenAI's; left for a future phase.
- **Text content only for Gemini** — image/audio parts in messages are silently skipped when using the Gemini path.
- **No embeddings, images, or audio endpoints** — only chat completions and messages.
