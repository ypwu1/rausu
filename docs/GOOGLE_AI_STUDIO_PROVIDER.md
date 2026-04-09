# Google AI Studio Provider

> **中文版:** [GOOGLE_AI_STUDIO_PROVIDER_CN.md](GOOGLE_AI_STUDIO_PROVIDER_CN.md)

## Overview

The `google-ai-studio` provider routes requests to [Google AI Studio](https://aistudio.google.com), which exposes an OpenAI-compatible endpoint for Gemini models. This provider forwards requests to `https://generativelanguage.googleapis.com/v1beta/openai` (or a custom base URL) with API-key authentication via the `x-goog-api-key` header.

**Auth note:** Google AI Studio uses the `x-goog-api-key` header for authentication, **not** the standard `Authorization: Bearer` scheme.

**Distinction from Vertex AI:** This provider targets Google AI Studio API keys (individual developer / free-tier access). For enterprise GCP deployments with project-based IAM auth, use `provider: vertex-ai`.

**Responses API bridge:** When Codex CLI or other clients send Responses API requests (`/v1/responses`) to a Google AI Studio-backed model, Rausu automatically bridges Responses -> Chat Completions format, the same strategy used by the `openrouter`, `openai`, `moonshot`, `deepseek`, and `z-ai` providers.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | Streaming + non-streaming |
| `POST /v1/responses` | Responses->ChatCompletions bridge |
| `GET /v1/models` | Lists configured model names |
| `POST /v1/messages` | Not supported (use `provider: anthropic`) |

## Prerequisites

A [Google AI Studio](https://aistudio.google.com/apikey) API key.

## Authentication

Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${GOOGLE_AI_STUDIO_API_KEY}"
```

```bash
export GOOGLE_AI_STUDIO_API_KEY="your-api-key-here"
```

## Quick start

### 1. Add to config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: gemini-2.0-flash
    providers:
      - provider: google-ai-studio
        model: gemini-2.0-flash
        api_key: "${GOOGLE_AI_STUDIO_API_KEY}"
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
    "model": "gemini-2.0-flash",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
codex --model gemini-2.0-flash
```

## Using the Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-2.0-flash",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: google-ai-studio
      model: <gemini-model-id>           # Required (e.g. "gemini-2.0-flash")
      api_key: <your-api-key>            # Required
      base_url: <url>                    # Optional; default: https://generativelanguage.googleapis.com/v1beta/openai
```

### `model`

Use the model ID as listed in Google AI Studio's documentation. Examples:

| Model ID | Description |
|---|---|
| `gemini-2.5-pro` | Most capable Gemini model |
| `gemini-2.5-flash` | Fast, balanced Gemini model |
| `gemini-2.0-flash` | Previous generation fast model |
| `gemini-2.0-flash-lite` | Lightweight fast model |

See [Google AI Studio documentation](https://ai.google.dev/gemini-api/docs/models) for the full list of available models.

### `base_url`

Overrides the default `https://generativelanguage.googleapis.com/v1beta/openai` endpoint. Use this to point at an alternative proxy. The provider appends `/chat/completions` to whatever you provide.

## Model naming

The virtual `name` in your config is what clients send. The `model` field is the upstream Google AI Studio model ID. You can choose any naming convention you prefer:

```yaml
# Option A: use the model ID directly
- name: gemini-2.0-flash
  providers:
    - provider: google-ai-studio
      model: gemini-2.0-flash

# Option B: custom alias
- name: my-gemini
  providers:
    - provider: google-ai-studio
      model: gemini-2.5-pro
```

## Multi-provider failover

Google AI Studio models can participate in Rausu's priority-based failover alongside other providers:

```yaml
- name: my-model
  providers:
    - provider: openai          # Try direct OpenAI first
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
    - provider: google-ai-studio  # Fall back to Google AI Studio
      model: gemini-2.0-flash
      api_key: "${GOOGLE_AI_STUDIO_API_KEY}"
```

## Capability-aware routing

The Google AI Studio provider declares the following capabilities to Rausu's router:

| Capability | Declared |
|---|---|
| `chat_completions` | Yes |
| `streaming` | Yes (SSE) |
| `responses_api` | Yes (Responses -> Chat Completions bridge) |
| `tools` | Yes (passed through to Google AI Studio) |
| `response_format` | Yes (passed through to Google AI Studio) |

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

Rausu does **not** silently strip `tools`, `tool_choice`, or `response_format` fields from requests on the Google AI Studio path. If the selected upstream model does not support a requested capability, the upstream error is propagated to the client unchanged.

> **Note:** The capability declarations above reflect what the Google AI Studio provider in Rausu exposes to the router. Actual capability support can still depend on the specific upstream model selected through Google AI Studio.

## Docker deployment

```bash
docker run \
  -e GOOGLE_AI_STUDIO_API_KEY="your-key" \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `401 Unauthorized` | Invalid or missing API key | Verify `GOOGLE_AI_STUDIO_API_KEY` is set and valid |
| `429 Too Many Requests` | Rate limit exceeded | Reduce request rate or add another provider for failover |
| `404 Not Found` | Invalid model ID | Check model ID matches Google AI Studio's available models |
| Model not found in Rausu | Typo in config `name` vs. client request | Ensure client sends the exact virtual `name` from config |

## Known limitations

- **No `/v1/messages` support.** Google AI Studio uses the OpenAI-compatible format. For Anthropic Messages API passthrough, use `provider: anthropic` or `provider: claude-subscription`.
- **No native Responses API.** Rausu bridges Responses -> Chat Completions automatically.
- Rate limits and model availability are controlled by Google. Rausu propagates upstream HTTP status codes unchanged.
- Tool/function calling is passed through as-is; no additional translation is performed. Capability depends on the upstream model.
