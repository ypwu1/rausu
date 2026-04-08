# DeepSeek Provider

> **中文版:** [DEEPSEEK_PROVIDER_CN.md](DEEPSEEK_PROVIDER_CN.md)

## Overview

The `deepseek` provider routes requests to [DeepSeek](https://www.deepseek.com), which exposes an OpenAI-compatible API. This provider forwards requests to `https://api.deepseek.com` (or a custom base URL) with API-key authentication.

**Base URL note:** DeepSeek's official documentation lists `https://api.deepseek.com` as the primary base URL. The `/v1` prefix (`https://api.deepseek.com/v1`) is also accepted for OpenAI client-library compatibility but is **not** a model-version signal.

**Responses API bridge:** When Codex CLI or other clients send Responses API requests (`/v1/responses`) to a DeepSeek-backed model, Rausu automatically bridges Responses -> Chat Completions format, the same strategy used by the `openrouter`, `openai`, `moonshot`, and `z-ai` providers.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | Streaming + non-streaming |
| `POST /v1/responses` | Responses->ChatCompletions bridge |
| `GET /v1/models` | Lists configured model names |
| `POST /v1/messages` | Not supported (use `provider: anthropic`) |

## Prerequisites

A [DeepSeek](https://platform.deepseek.com) API key.

## Authentication

Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${DEEPSEEK_API_KEY}"
```

```bash
export DEEPSEEK_API_KEY="your-api-key-here"
```

## Quick start

### 1. Add to config.yaml

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

### 2. Start Rausu

```bash
rausu --config config.yaml
```

### 3. Send a request

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-chat",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
codex --model deepseek-chat
```

## Using the Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-chat",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: deepseek
      model: <deepseek-model-id>         # Required (e.g. "deepseek-chat")
      api_key: <your-api-key>            # Required
      base_url: <url>                    # Optional; default: https://api.deepseek.com
```

### `model`

Use the model ID as listed in DeepSeek's documentation. Examples:

| Model ID | Description |
|---|---|
| `deepseek-chat` | General-purpose chat model |
| `deepseek-reasoner` | Reasoning-focused model (DeepSeek-R1) |

See [DeepSeek API documentation](https://platform.deepseek.com/api-docs) for the full list of available models.

### `base_url`

Overrides the default `https://api.deepseek.com` endpoint. Use this to point at a self-hosted or alternative DeepSeek-compatible proxy.

**Note:** DeepSeek also accepts `https://api.deepseek.com/v1` for OpenAI client-library compatibility. The `/v1` path is not a model-version signal. If you use a custom base URL, the provider appends `/chat/completions` to whatever you provide.

## Model naming

The virtual `name` in your config is what clients send. The `model` field is the upstream DeepSeek model ID. You can choose any naming convention you prefer:

```yaml
# Option A: use the DeepSeek model ID directly
- name: deepseek-chat
  providers:
    - provider: deepseek
      model: deepseek-chat

# Option B: custom alias
- name: my-reasoning-model
  providers:
    - provider: deepseek
      model: deepseek-reasoner
```

## Multi-provider failover

DeepSeek models can participate in Rausu's priority-based failover alongside other providers:

```yaml
- name: my-model
  providers:
    - provider: openai          # Try direct OpenAI first
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
    - provider: deepseek        # Fall back to DeepSeek
      model: deepseek-chat
      api_key: "${DEEPSEEK_API_KEY}"
```

## Capability-aware routing

The DeepSeek provider declares the following capabilities to Rausu's router:

| Capability | Declared |
|---|---|
| `chat_completions` | Yes |
| `streaming` | Yes (SSE) |
| `responses_api` | Yes (Responses -> Chat Completions bridge) |
| `tools` | Yes (passed through to DeepSeek) |
| `response_format` | Yes (passed through to DeepSeek) |

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

Rausu does **not** silently strip `tools`, `tool_choice`, or `response_format` fields from requests on the DeepSeek path. If the selected upstream model does not support a requested capability, the upstream error is propagated to the client unchanged.

> **Note:** The capability declarations above reflect what the DeepSeek provider in Rausu exposes to the router. Actual capability support can still depend on the specific upstream model selected through DeepSeek.

## Docker deployment

```bash
docker run \
  -e DEEPSEEK_API_KEY="your-key" \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `401 Unauthorized` | Invalid or missing API key | Verify `DEEPSEEK_API_KEY` is set and valid |
| `429 Too Many Requests` | Rate limit exceeded | Reduce request rate or add another provider for failover |
| `404 Not Found` | Invalid model ID | Check model ID matches DeepSeek's available models |
| Model not found in Rausu | Typo in config `name` vs. client request | Ensure client sends the exact virtual `name` from config |

## Known limitations

- **No `/v1/messages` support.** DeepSeek uses the OpenAI-compatible format. For Anthropic Messages API passthrough, use `provider: anthropic` or `provider: claude-subscription`.
- **No native Responses API.** Rausu bridges Responses -> Chat Completions automatically.
- Rate limits and model availability are controlled by DeepSeek. Rausu propagates upstream HTTP status codes unchanged.
- Tool/function calling is passed through as-is; no additional translation is performed. Capability depends on the upstream model.
