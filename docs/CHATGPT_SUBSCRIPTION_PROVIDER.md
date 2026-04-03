# ChatGPT Subscription Provider

> **中文版:** [CHATGPT_SUBSCRIPTION_PROVIDER_CN.md](CHATGPT_SUBSCRIPTION_PROVIDER_CN.md)

## Overview

The `chatgpt-subscription` provider lets you route requests through your ChatGPT Plus, Pro, or Max subscription without an API key. Rausu reads your OAuth access token from `~/.config/rausu/chatgpt-auth.json` and bridges OpenAI Chat Completions requests to the ChatGPT Responses API (`https://chatgpt.com/backend-api/codex/responses`). The Responses API is also directly accessible via `/v1/responses`.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | ✅ (streaming + non-streaming, bridged to Responses API) |
| `POST /v1/responses` | ✅ native Responses API passthrough |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/messages` | ✅ GPT models: Messages→Responses bridge; Claude models: not supported |

## Prerequisites

You need a ChatGPT Plus, Pro, or Max subscription. The Codex models (`gpt-5.3-codex`, etc.) require a plan that includes access to Codex.

The OAuth token is loaded from `~/.config/rausu/chatgpt-auth.json`, which you must create manually (see [Authentication](#authentication) below).

## Authentication

### Credentials file

Create `~/.config/rausu/chatgpt-auth.json` with your ChatGPT OAuth token:

```json
{
  "access_token": "eyJ...",
  "refresh_token": "...",
  "expires_at": 1900000000000,
  "account_id": "user-..."
}
```

Fields:

| Field | Required | Description |
|---|---|---|
| `access_token` | Yes | Bearer access token from the ChatGPT OAuth flow |
| `refresh_token` | Recommended | Used for automatic token refresh before expiry |
| `expires_at` | Recommended | Expiry time in Unix milliseconds |
| `account_id` | Optional | ChatGPT account ID; auto-extracted from JWT if omitted |

If `account_id` is absent, Rausu decodes the JWT payload and looks up `["https://api.openai.com/auth"]["chatgpt_account_id"]` automatically.

Rausu refreshes the access token automatically when it is within 5 minutes of expiry, provided a `refresh_token` is present.

### Obtaining your token

The access token can be extracted from a logged-in ChatGPT browser session (check the `Authorization` header in browser developer tools on `chatgpt.com`), or from the Codex CLI authentication flow at `~/.codex/auth.json`. Copy the relevant fields into `~/.config/rausu/chatgpt-auth.json`.

### Environment variables

Alternatively, provide the token via environment variables (no credentials file needed):

```bash
export CHATGPT_ACCESS_TOKEN="eyJ..."
export CHATGPT_REFRESH_TOKEN="..."          # Optional: enables automatic refresh
export CHATGPT_ACCOUNT_ID="user-..."        # Optional: skips JWT decode
```

Then set `token_source: env` in your Rausu config.

### Token source resolution for `auto`

1. `CHATGPT_ACCESS_TOKEN` environment variable (plus optional `CHATGPT_REFRESH_TOKEN` / `CHATGPT_ACCOUNT_ID`)
2. `~/.config/rausu/chatgpt-auth.json`

## Quick start

### 1. Create `~/.config/rausu/chatgpt-auth.json`

See [Authentication](#authentication) above.

### 2. Add to config.yaml

```yaml
models:
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto

  - name: gpt-5-codex
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.3-codex
        token_source: auto
```

### 3. Start Rausu

```bash
rausu --config config.yaml
```

### 4. Send a request

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gpt-5",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

The `chatgpt-subscription` provider is designed to work with [Codex CLI](https://github.com/openai/codex), which uses the OpenAI-compatible API.

### Step-by-step

**1. Create `config.yaml`**

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty

models:
  - name: gpt-5.3-codex
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.3-codex
        token_source: auto

  - name: gpt-5.4
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto
```

> **Tip:** Use the exact upstream model ID as the virtual name so Codex CLI picks it up without `--model` overrides.

**2. Start Rausu**

```bash
./rausu --config config.yaml
```

**3. Point Codex CLI at Rausu**

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this, but Codex requires a non-empty key
codex --model gpt-5.3-codex
```

### Using the Responses API directly

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gpt-5.3-codex",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: chatgpt-subscription
      model: <upstream-chatgpt-model-id>       # Required
      token_source: auto                        # Required: auto | env | credentials_file
      credentials_path: /path/to/chatgpt-auth.json  # Optional; default: ~/.config/rausu/chatgpt-auth.json
```

### `token_source`

| Value | Behavior |
|---|---|
| `auto` | Try env vars first (`CHATGPT_ACCESS_TOKEN`), then credentials file |
| `env` | Use `CHATGPT_ACCESS_TOKEN` env var only |
| `credentials_file` | Use credentials file only |

### `credentials_path`

Overrides the default `~/.config/rausu/chatgpt-auth.json` path.

## Upstream model names

Model availability depends on your subscription plan. Confirmed working model IDs as of 2026-Q1:

| Model ID | Description |
|---|---|
| `gpt-5.4` | GPT-5.4 (flagship) |
| `gpt-5.4-pro` | GPT-5.4 Pro |
| `gpt-5.3-codex` | GPT-5.3 Codex (for Codex CLI) |
| `gpt-5.3-codex-spark` | GPT-5.3 Codex Spark (lightweight) |
| `gpt-5.3-instant` | GPT-5.3 Instant |
| `gpt-5.3-chat-latest` | GPT-5.3 Chat (latest) |

ChatGPT may return an error for models not available on your plan.

## Request bridging internals

Chat Completions requests are translated to the ChatGPT Responses API format:

| Chat Completions field | Responses API field |
|---|---|
| `messages[role=system]` | `instructions` |
| `messages` (user/assistant) | `input` array |
| `model` | `model` |
| `stream` | always streamed internally, aggregated for non-streaming callers |

Rausu always streams the upstream request internally and aggregates the chunks for non-streaming callers. The following headers are sent to the ChatGPT endpoint:

- `Authorization: Bearer <access_token>`
- `chatgpt-account-id: <account_id>` (when available)
- `OpenAI-Beta: responses=experimental`
- `originator: pi`

Tokens are **never logged**.

## Docker deployment

```bash
docker run \
  -v ~/.config/rausu/chatgpt-auth.json:/app/chatgpt-auth.json \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

In `config.yaml`, add `credentials_path: /app/chatgpt-auth.json`, or use the `CHATGPT_ACCESS_TOKEN` environment variable approach.

## Using with Claude Code (GPT models via protocol bridge)

Claude Code sends requests to `/v1/messages`.  When the configured model is a GPT model,
Rausu automatically bridges Messages API → Responses API and forwards to the ChatGPT
Responses endpoint.

```yaml
models:
  # Claude Code can use this GPT model via /v1/messages
  - name: gpt-5.4
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto
```

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="local-proxy"
# In Claude Code settings, select gpt-5.4 as the model
```

Rausu converts the Messages API request to Responses format, proxies to ChatGPT, then
converts the response back to Messages format — including SSE streaming with zero
buffering.  Full tool calling (`tool_use` ↔ `function_call`) is supported.

## Known limitations

- **Messages API: GPT models only.** Claude models are not available via `chatgpt-subscription`; use `provider: anthropic`, `provider: claude-subscription`, or `provider: github-copilot` for Claude.
- **Subscription rate limits** and model availability are controlled by OpenAI — Rausu propagates the upstream HTTP status code unchanged.
- **Token acquisition is manual.** Unlike GitHub Copilot, there is no automated device-flow login. You must obtain and place the token in the credentials file yourself.
- The `base_url` config field is not applicable for this provider; the endpoint is always `https://chatgpt.com/backend-api/codex/responses`.
