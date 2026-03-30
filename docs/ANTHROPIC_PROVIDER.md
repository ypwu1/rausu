# Anthropic Provider

> **中文版:** [ANTHROPIC_PROVIDER_CN.md](ANTHROPIC_PROVIDER_CN.md)

## Overview

The `anthropic` provider routes requests to the Anthropic API using an API key. It accepts both OpenAI Chat Completions format (automatically translated to Anthropic Messages API format) and native Anthropic Messages API requests via `/v1/messages`.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/messages` | ✅ native Anthropic Messages API passthrough |
| `POST /v1/chat/completions` | ✅ (streaming + non-streaming, translated to Messages API) |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/responses` | ❌ Use `provider: openai` or `provider: chatgpt-subscription` |

## Prerequisites

An [Anthropic API key](https://console.anthropic.com/settings/keys).

## Authentication

Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${ANTHROPIC_API_KEY}"
```

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

## Quick start

### 1. Add to config.yaml

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

### 2. Start Rausu

```bash
rausu --config config.yaml
```

### 3. Test with curl (Messages API)

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

### 4. Test with curl (Chat Completions format)

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "claude-sonnet",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Claude Code CLI

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
claude -p "Hello via Rausu"
```

Claude Code sends requests to `/v1/messages`, which is natively supported by this provider.

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: anthropic
      model: <anthropic-model-id>   # Required
      api_key: <your-api-key>       # Required
```

> **Note:** There is no `base_url` override for this provider. Requests always go to `https://api.anthropic.com/v1/messages`.

## Format translation

When requests arrive at `/v1/chat/completions`, Rausu translates them to the Anthropic Messages API format automatically:

| OpenAI Chat Completions | Anthropic Messages API |
|---|---|
| `messages[role=system]` | top-level `system` field |
| `messages[role=user/assistant]` | `messages` array |
| `max_tokens` | `max_tokens` (default: 4096 if not specified) |
| `temperature` | `temperature` |
| `stop` | `stop_sequences` |
| `tools` / `functions` | `tools` |
| Stop reason `end_turn` | `finish_reason: stop` |
| Stop reason `max_tokens` | `finish_reason: length` |
| Stop reason `tool_use` | `finish_reason: tool_calls` |

## Upstream model names

Any model available on your Anthropic account can be used. Common examples:

| Model ID | Description |
|---|---|
| `claude-opus-4-20250514` | Claude Opus 4 (most capable) |
| `claude-sonnet-4-20250514` | Claude Sonnet 4 |
| `claude-haiku-4-20250514` | Claude Haiku 4 (fastest) |
| `claude-sonnet-4-5-20251001` | Claude Sonnet 4.5 |
| `claude-haiku-3-20240307` | Claude 3 Haiku (legacy) |

Check the [Anthropic model documentation](https://docs.anthropic.com/en/docs/about-claude/models) for the full list of available model IDs.

## Docker deployment

```bash
docker run \
  -e ANTHROPIC_API_KEY="sk-ant-..." \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Known limitations

- **No `/v1/responses` support.** Use `provider: openai` or `provider: chatgpt-subscription`.
- Rate limits and model availability are controlled by Anthropic — Rausu propagates the upstream HTTP status code unchanged.
- The `base_url` config field is not supported for this provider.
- Image/multimodal content in Chat Completions format may not translate correctly — use the native Messages API format (`/v1/messages`) for multimodal requests.
