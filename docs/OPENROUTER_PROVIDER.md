# OpenRouter Provider

> **中文版:** [OPENROUTER_PROVIDER_CN.md](OPENROUTER_PROVIDER_CN.md)

## Overview

The `openrouter` provider routes requests to [OpenRouter](https://openrouter.ai), an LLM aggregator that provides access to 100+ models from OpenAI, Anthropic, Google, Meta, Mistral, and others through a single API key and a unified OpenAI-compatible interface.

**Why OpenRouter?** A single OpenRouter API key gives access to many upstream models without managing separate credentials for each provider. This makes it ideal for experimentation, cost comparison, and accessing models that may not be available in your region.

**Responses API bridge:** When Codex CLI or other clients send Responses API requests (`/v1/responses`) to an OpenRouter-backed model, Rausu automatically bridges Responses -> Chat Completions format, the same strategy used by the generic `openai` provider.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | Streaming + non-streaming |
| `POST /v1/responses` | Responses->ChatCompletions bridge |
| `GET /v1/models` | Lists configured model names |
| `POST /v1/messages` | Not supported (use `provider: anthropic`) |

## Prerequisites

An [OpenRouter API key](https://openrouter.ai/keys). Free-tier keys have rate limits; paid keys provide higher throughput.

## Authentication

Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${OPENROUTER_API_KEY}"
```

```bash
export OPENROUTER_API_KEY="sk-or-v1-..."
```

## Quick start

### 1. Add to config.yaml

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

### 2. Start Rausu

```bash
rausu --config config.yaml
```

### 3. Send a request

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "openrouter-gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
codex --model openrouter-gpt-4o
```

## Using the Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "openrouter-gpt-4o",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: openrouter
      model: <openrouter-model-id>    # Required (e.g. "openai/gpt-4o")
      api_key: <your-api-key>         # Required
      base_url: <url>                 # Optional; default: https://openrouter.ai/api/v1
```

### `model`

OpenRouter model IDs use the `provider/model` format. Examples:

| Model ID | Description |
|---|---|
| `openai/gpt-4o` | OpenAI GPT-4o |
| `openai/o3` | OpenAI o3 reasoning model |
| `anthropic/claude-sonnet-4` | Anthropic Claude Sonnet 4 |
| `anthropic/claude-opus-4` | Anthropic Claude Opus 4 |
| `google/gemini-2.5-pro` | Google Gemini 2.5 Pro |
| `meta-llama/llama-4-maverick` | Meta Llama 4 Maverick |

See the [OpenRouter model catalogue](https://openrouter.ai/models) for the full list.

### `base_url`

Overrides the default `https://openrouter.ai/api/v1` endpoint. Use this to point at a self-hosted OpenRouter-compatible proxy.

## Model naming

The virtual `name` in your config is what clients send. The `model` field is the upstream OpenRouter model ID. You can choose any naming convention you prefer:

```yaml
# Option A: descriptive names
- name: gpt-4o
  providers:
    - provider: openrouter
      model: openai/gpt-4o

# Option B: prefixed names (avoids collision with direct provider entries)
- name: or-gpt-4o
  providers:
    - provider: openrouter
      model: openai/gpt-4o

# Option C: use the OpenRouter ID directly
- name: openai/gpt-4o
  providers:
    - provider: openrouter
      model: openai/gpt-4o
```

## Multi-provider failover

OpenRouter models can participate in Rausu's priority-based failover alongside other providers:

```yaml
- name: gpt-4o
  providers:
    - provider: openai          # Try direct OpenAI first
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
    - provider: openrouter      # Fall back to OpenRouter
      model: openai/gpt-4o
      api_key: "${OPENROUTER_API_KEY}"
```

## Capability-aware routing

The OpenRouter provider declares the following capabilities to Rausu's router:

| Capability | Declared |
|---|---|
| `chat_completions` | Yes |
| `streaming` | Yes (SSE) |
| `responses_api` | Yes (Responses → Chat Completions bridge) |
| `tools` | Yes (passed through to OpenRouter) |
| `response_format` | Yes (passed through to OpenRouter) |

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

Rausu does **not** silently strip `tools`, `tool_choice`, or `response_format` fields from requests on the OpenRouter path. If the selected upstream model does not support a requested capability, the upstream error is propagated to the client unchanged.

> **Note:** The capability declarations above reflect what the OpenRouter provider in Rausu exposes to the router. Actual capability support can still depend on the specific upstream model selected through OpenRouter. For example, `tools` being declared means Rausu will forward the field to OpenRouter, but a model that does not support function calling will return an error from OpenRouter itself.

## Docker deployment

```bash
docker run \
  -e OPENROUTER_API_KEY="sk-or-v1-..." \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `401 Unauthorized` | Invalid or missing API key | Verify `OPENROUTER_API_KEY` is set and valid |
| `402 Payment Required` | Insufficient credits | Add credits at [openrouter.ai/credits](https://openrouter.ai/credits) |
| `429 Too Many Requests` | Rate limit exceeded | Upgrade plan or add another provider for failover |
| `404 Not Found` | Invalid model ID | Check model ID format: `provider/model` |
| Model not found in Rausu | Typo in config `name` vs. client request | Ensure client sends the exact virtual `name` from config |

## Known limitations

- **No `/v1/messages` support.** OpenRouter uses the OpenAI-compatible format. For Anthropic Messages API passthrough, use `provider: anthropic` or `provider: claude-subscription`.
- **No native Responses API.** Rausu bridges Responses -> Chat Completions automatically.
- Rate limits and model availability are controlled by OpenRouter. Rausu propagates upstream HTTP status codes unchanged.
- Tool/function calling is passed through as-is; no additional translation is performed. Capability depends on the upstream model selected.
