# MiniMax Provider

> **中文版:** [MINIMAX_PROVIDER_CN.md](MINIMAX_PROVIDER_CN.md)

## Overview

The `minimax` provider routes requests to [MiniMax](https://www.minimax.io), a Chinese AI company that exposes both an **Anthropic-compatible** and an **OpenAI-compatible** API under `api.minimax.io`. Rausu maps these two protocols internally behind a single `provider: minimax` entry in your config — no separate `minimax-openai` or `minimax-anthropic` providers needed.

**Internal protocol selection:**

| Downstream request | MiniMax upstream endpoint |
|---|---|
| `POST /v1/chat/completions` | `https://api.minimax.io/v1/chat/completions` (OpenAI-compatible) |
| `POST /v1/messages` | `https://api.minimax.io/anthropic/v1/messages` (Anthropic-compatible) |
| `POST /v1/responses` | Bridged: Responses → Chat Completions → Responses |

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | ✅ Streaming + non-streaming |
| `POST /v1/messages` | ✅ Text and tool calls (streaming + non-streaming) |
| `POST /v1/responses` | ✅ Bridged via Chat Completions transform |
| `GET /v1/models` | ✅ Lists configured model names |
| Image / document inputs | ❌ Not supported (explicit error returned) |

## Prerequisites

A MiniMax API key. Sign up at [minimax.io](https://www.minimax.io) and generate a key in the console.

## Authentication

Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${MINIMAX_API_KEY}"
```

```bash
export MINIMAX_API_KEY="eyJ..."
```

Rausu sends the key as `Authorization: Bearer <key>` on both the OpenAI-compatible and Anthropic-compatible paths.

## Quick start

### 1. Add to config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: minimax-text-01
    providers:
      - provider: minimax
        model: minimax-text-01
        api_key: "${MINIMAX_API_KEY}"

  - name: abab6.5s-chat
    providers:
      - provider: minimax
        model: abab6.5s-chat
        api_key: "${MINIMAX_API_KEY}"
```

### 2. Start Rausu

```bash
rausu --config config.yaml
```

### 3. Send a request

**Chat completions:**

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-text-01",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

**Messages API:**

```bash
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "minimax-text-01",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: minimax
      model: <minimax-model-id>       # Required (e.g. "minimax-text-01")
      api_key: <your-api-key>         # Required
      base_url: <url>                 # Optional; default: https://api.minimax.io
```

### `model`

Use the MiniMax model ID. Examples:

| Model ID | Description |
|---|---|
| `minimax-text-01` | MiniMax Text-01 flagship model |
| `abab6.5s-chat` | MiniMax ABAB 6.5S chat model |
| `abab6.5g-chat` | MiniMax ABAB 6.5G chat model |
| `abab5.5s-chat` | MiniMax ABAB 5.5S chat model |

Consult the [MiniMax model catalogue](https://www.minimax.io/platform/document/model-list) for the authoritative list.

### `base_url`

Overrides the default root `https://api.minimax.io`. Rausu appends `/v1` for OpenAI-compatible requests and `/anthropic/v1` for Anthropic-compatible requests.

```yaml
base_url: "https://api.minimax.io"   # default
```

This setting is useful for routing through a local proxy or alternative MiniMax endpoint.

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
codex --model minimax-text-01
```

## Using with Claude Code

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
claude --model minimax-text-01
```

## Tool calling

Both the Messages API and Chat Completions paths support tool calling.

**Chat completions (OpenAI tools format):**

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-text-01",
    "messages": [{"role": "user", "content": "What is the weather in Tokyo?"}],
    "tools": [{
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "Get current weather for a city",
        "parameters": {
          "type": "object",
          "properties": {
            "city": {"type": "string"}
          },
          "required": ["city"]
        }
      }
    }]
  }'
```

**Messages API (Anthropic tools format):**

```bash
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "minimax-text-01",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "What is the weather in Tokyo?"}],
    "tools": [{
      "name": "get_weather",
      "description": "Get current weather for a city",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": {"type": "string"}
        },
        "required": ["city"]
      }
    }]
  }'
```

## Using the Responses API

Rausu bridges the Responses API through MiniMax's OpenAI-compatible endpoint automatically:

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-text-01",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Multi-provider failover

MiniMax models can participate in Rausu's priority-based failover:

```yaml
- name: my-llm
  providers:
    - provider: anthropic          # Try direct Anthropic first
      model: claude-sonnet-4-5
      api_key: "${ANTHROPIC_API_KEY}"
    - provider: minimax            # Fall back to MiniMax
      model: minimax-text-01
      api_key: "${MINIMAX_API_KEY}"
```

## Capability-aware routing

The MiniMax provider declares the following capabilities to Rausu's router:

| Capability | Declared |
|---|---|
| `chat_completions` | Yes |
| `streaming` | Yes (SSE on both paths) |
| `responses_api` | Yes (Responses → Chat Completions bridge) |
| `tools` | Yes (both paths) |
| `messages_api` | Yes (Anthropic-compatible passthrough) |
| `response_format` | No |

### `unsupported_capability` error

When all providers for a model are skipped due to missing capabilities, Rausu returns:

- **HTTP status:** `422 Unprocessable Entity`
- **`error.type`:** `unsupported_capability`
- **`error.code`:** `unsupported_capability`

Example:

```json
{
  "error": {
    "message": "No provider for model 'minimax-text-01' supports the required capabilities: response_format",
    "type": "unsupported_capability",
    "code": "unsupported_capability"
  }
}
```

## Known limitations

- **No image or document inputs.** MiniMax's Anthropic-compatible endpoint does not support `image` or `document` content blocks. Requests containing these blocks are rejected with `405 Unsupported` before reaching MiniMax, consistent with Rausu's no-silent-downgrade policy.
- **No native Responses API.** Rausu bridges Responses → Chat Completions automatically.
- **No `response_format` support declared.** If you need structured output, use a provider that declares `response_format`.
- Rate limits and model availability are controlled by MiniMax. Rausu propagates upstream HTTP status codes unchanged.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `401 Unauthorized` | Invalid or missing API key | Verify `MINIMAX_API_KEY` is set and valid |
| `429 Too Many Requests` | Rate limit exceeded | Reduce request rate or add a fallback provider |
| `404 Not Found` | Invalid model ID | Check the model ID against the MiniMax model catalogue |
| `405` on Messages API | Request contains image/document blocks | Remove unsupported content block types |
| Model not found in Rausu | Typo in config `name` vs. client request | Ensure client sends the exact virtual `name` from config |
