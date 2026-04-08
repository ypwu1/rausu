# Moonshot / Kimi Provider

> **中文版:** [MOONSHOT_PROVIDER_CN.md](MOONSHOT_PROVIDER_CN.md)

## Overview

The `moonshot` provider routes requests to [Moonshot AI / Kimi](https://www.moonshot.cn), which exposes an OpenAI-compatible API. This provider forwards requests to `https://api.moonshot.ai/v1` (or a custom base URL) with API-key authentication.

**Responses API bridge:** When Codex CLI or other clients send Responses API requests (`/v1/responses`) to a Moonshot-backed model, Rausu automatically bridges Responses -> Chat Completions format, the same strategy used by the `openrouter`, `openai`, and `z-ai` providers.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | Streaming + non-streaming |
| `POST /v1/responses` | Responses->ChatCompletions bridge |
| `GET /v1/models` | Lists configured model names |
| `POST /v1/messages` | Not supported (use `provider: anthropic`) |

## Prerequisites

A [Moonshot AI](https://platform.moonshot.cn) API key.

## Authentication

Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${MOONSHOT_API_KEY}"
```

```bash
export MOONSHOT_API_KEY="your-api-key-here"
```

## Quick start

### 1. Add to config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: moonshot-v1-8k
    providers:
      - provider: moonshot
        model: moonshot-v1-8k
        api_key: "${MOONSHOT_API_KEY}"
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
    "model": "moonshot-v1-8k",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
codex --model moonshot-v1-8k
```

## Using the Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "moonshot-v1-8k",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: moonshot
      model: <moonshot-model-id>         # Required (e.g. "moonshot-v1-8k")
      api_key: <your-api-key>            # Required
      base_url: <url>                    # Optional; default: https://api.moonshot.ai/v1
```

### `model`

Use the model ID as listed in Moonshot's documentation. Examples:

| Model ID | Description |
|---|---|
| `moonshot-v1-8k` | 8K context window |
| `moonshot-v1-32k` | 32K context window |
| `moonshot-v1-128k` | 128K context window |

See [Moonshot platform documentation](https://platform.moonshot.cn) for the full list of available models.

### `base_url`

Overrides the default `https://api.moonshot.ai/v1` endpoint. Use this to point at a self-hosted or alternative Moonshot-compatible proxy.

## Model naming

The virtual `name` in your config is what clients send. The `model` field is the upstream Moonshot model ID. You can choose any naming convention you prefer:

```yaml
# Option A: use the Moonshot model ID directly
- name: moonshot-v1-8k
  providers:
    - provider: moonshot
      model: moonshot-v1-8k

# Option B: custom alias
- name: kimi-8k
  providers:
    - provider: moonshot
      model: moonshot-v1-8k
```

## Multi-provider failover

Moonshot models can participate in Rausu's priority-based failover alongside other providers:

```yaml
- name: my-model
  providers:
    - provider: openai          # Try direct OpenAI first
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
    - provider: moonshot        # Fall back to Moonshot
      model: moonshot-v1-8k
      api_key: "${MOONSHOT_API_KEY}"
```

## Capability-aware routing

The Moonshot provider declares the following capabilities to Rausu's router:

| Capability | Declared |
|---|---|
| `chat_completions` | Yes |
| `streaming` | Yes (SSE) |
| `responses_api` | Yes (Responses -> Chat Completions bridge) |
| `tools` | Yes (passed through to Moonshot) |
| `response_format` | Yes (passed through to Moonshot) |

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

Rausu does **not** silently strip `tools`, `tool_choice`, or `response_format` fields from requests on the Moonshot path. If the selected upstream model does not support a requested capability, the upstream error is propagated to the client unchanged.

> **Note:** The capability declarations above reflect what the Moonshot provider in Rausu exposes to the router. Actual capability support can still depend on the specific upstream model selected through Moonshot.

## Docker deployment

```bash
docker run \
  -e MOONSHOT_API_KEY="your-key" \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `401 Unauthorized` | Invalid or missing API key | Verify `MOONSHOT_API_KEY` is set and valid |
| `429 Too Many Requests` | Rate limit exceeded | Reduce request rate or add another provider for failover |
| `404 Not Found` | Invalid model ID | Check model ID matches Moonshot's available models |
| Model not found in Rausu | Typo in config `name` vs. client request | Ensure client sends the exact virtual `name` from config |

## Known limitations

- **No `/v1/messages` support.** Moonshot uses the OpenAI-compatible format. For Anthropic Messages API passthrough, use `provider: anthropic` or `provider: claude-subscription`.
- **No native Responses API.** Rausu bridges Responses -> Chat Completions automatically.
- Rate limits and model availability are controlled by Moonshot. Rausu propagates upstream HTTP status codes unchanged.
- Tool/function calling is passed through as-is; no additional translation is performed. Capability depends on the upstream model.
