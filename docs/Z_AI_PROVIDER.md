# Z.AI Provider

> **中文版:** [Z_AI_PROVIDER_CN.md](Z_AI_PROVIDER_CN.md)

## Overview

The `z-ai` provider routes requests to [Z.AI](https://z.ai), which exposes an OpenAI-compatible API. This provider forwards requests to `https://api.z.ai/api/paas/v4/` (or a custom base URL) with API-key authentication.

**Responses API bridge:** When Codex CLI or other clients send Responses API requests (`/v1/responses`) to a Z.AI-backed model, Rausu automatically bridges Responses -> Chat Completions format, the same strategy used by the `openrouter` and `openai` providers.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | Streaming + non-streaming |
| `POST /v1/responses` | Responses->ChatCompletions bridge |
| `GET /v1/models` | Lists configured model names |
| `POST /v1/messages` | Not supported (use `provider: anthropic`) |

## Prerequisites

A [Z.AI](https://z.ai) API key.

## Authentication

Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${Z_AI_API_KEY}"
```

```bash
export Z_AI_API_KEY="your-api-key-here"
```

## Quick start

### 1. Add to config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: z-ai-1-preview
    providers:
      - provider: z-ai
        model: z-ai-1-preview
        api_key: "${Z_AI_API_KEY}"
```

### 2. Start Rausu

```bash
rausu --config config.yaml
```

### 3. Send a request

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "z-ai-1-preview",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
codex --model z-ai-1-preview
```

## Using the Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "z-ai-1-preview",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: z-ai
      model: <z-ai-model-id>           # Required (e.g. "z-ai-1-preview")
      api_key: <your-api-key>           # Required
      base_url: <url>                   # Optional; default: https://api.z.ai/api/paas/v4
```

### `model`

Use the model ID as listed in Z.AI's documentation. Examples:

| Model ID | Description |
|---|---|
| `z-ai-1-preview` | Z.AI preview model |

See Z.AI documentation for the full list of available models.

### `base_url`

Overrides the default `https://api.z.ai/api/paas/v4` endpoint. Use this to point at a self-hosted or alternative Z.AI-compatible proxy.

## Model naming

The virtual `name` in your config is what clients send. The `model` field is the upstream Z.AI model ID. You can choose any naming convention you prefer:

```yaml
# Option A: use the Z.AI model ID directly
- name: z-ai-1-preview
  providers:
    - provider: z-ai
      model: z-ai-1-preview

# Option B: custom alias
- name: my-zai-model
  providers:
    - provider: z-ai
      model: z-ai-1-preview
```

## Multi-provider failover

Z.AI models can participate in Rausu's priority-based failover alongside other providers:

```yaml
- name: my-model
  providers:
    - provider: openai          # Try direct OpenAI first
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
    - provider: z-ai            # Fall back to Z.AI
      model: z-ai-1-preview
      api_key: "${Z_AI_API_KEY}"
```

## Capability-aware routing

The Z.AI provider declares the following capabilities to Rausu's router:

| Capability | Declared |
|---|---|
| `chat_completions` | Yes |
| `streaming` | Yes (SSE) |
| `responses_api` | Yes (Responses -> Chat Completions bridge) |
| `tools` | Yes (passed through to Z.AI) |
| `response_format` | Yes (passed through to Z.AI) |

**How routing works:**

1. When a request arrives the router inspects it and determines which capabilities are required. A request containing `tools` requires `tools`; a request with `response_format` requires `response_format`.
2. Providers that lack any required capability are **skipped before any upstream call** is made.
3. If another configured provider for the same virtual model supports the required capabilities, failover continues there.
4. If **no** configured provider supports all required capabilities, Rausu returns a clear client-facing error instead of silently degrading or stripping fields.

### `unsupported_capability` error

When all providers for a model are skipped due to missing capabilities, Rausu returns:

- **HTTP status:** `422 Unprocessable Entity`
- **`error.type`:** `unsupported_capability`
- **`error.code`:** `unsupported_capability`
- **`error.message`:** names the missing capability or capabilities

Example response body:

```json
{
  "error": {
    "message": "No provider for model 'my-model' supports the required capabilities: tools",
    "type": "unsupported_capability",
    "code": "unsupported_capability"
  }
}
```

### No silent downgrade policy

Rausu does **not** silently strip `tools`, `tool_choice`, or `response_format` fields from requests on the Z.AI path. If the selected upstream model does not support a requested capability, the upstream error is propagated to the client unchanged.

> **Note:** The capability declarations above reflect what the Z.AI provider in Rausu exposes to the router. Actual capability support can still depend on the specific upstream model selected through Z.AI.

## Docker deployment

```bash
docker run \
  -e Z_AI_API_KEY="your-key" \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `401 Unauthorized` | Invalid or missing API key | Verify `Z_AI_API_KEY` is set and valid |
| `429 Too Many Requests` | Rate limit exceeded | Reduce request rate or add another provider for failover |
| `404 Not Found` | Invalid model ID | Check model ID matches Z.AI's available models |
| Model not found in Rausu | Typo in config `name` vs. client request | Ensure client sends the exact virtual `name` from config |

## Known limitations

- **No `/v1/messages` support.** Z.AI uses the OpenAI-compatible format. For Anthropic Messages API passthrough, use `provider: anthropic` or `provider: claude-subscription`.
- **No native Responses API.** Rausu bridges Responses -> Chat Completions automatically.
- Rate limits and model availability are controlled by Z.AI. Rausu propagates upstream HTTP status codes unchanged.
- Tool/function calling is passed through as-is; no additional translation is performed. Capability depends on the upstream model.
