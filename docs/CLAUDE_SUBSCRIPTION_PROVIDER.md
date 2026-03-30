# Claude Subscription Provider

> **中文版:** [CLAUDE_SUBSCRIPTION_PROVIDER_CN.md](CLAUDE_SUBSCRIPTION_PROVIDER_CN.md)

## Overview

The `claude-subscription` provider lets you route requests through your Claude Pro or Max subscription without an API key. Rausu reads your OAuth token from `~/.claude/.credentials.json` (written by the Claude CLI) and forwards requests to `https://api.anthropic.com/v1/messages` using the Claude Code identity headers required for subscription-based access.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/messages` | ✅ (streaming + non-streaming) |
| `POST /v1/chat/completions` | ❌ Use `provider: anthropic` for API-key-based chat completions |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/responses` | ❌ Not supported by the Anthropic Messages API |

## Prerequisites

You need a Claude Pro or Max subscription and the Claude CLI installed and authenticated.

The OAuth token is loaded from `~/.claude/.credentials.json`, which is written when you sign in with `claude` (the Claude CLI).

```json
{
  "claudeAiOauth": {
    "accessToken": "...",
    "refreshToken": "...",
    "expiresAt": 1743000000000
  }
}
```

Rausu refreshes the access token automatically using the refresh token when it is within 5 minutes of expiry.

## Authentication

### Option A: Credentials file (recommended)

Sign in once with the Claude CLI:

```bash
claude
# Follow the browser-based OAuth flow to authenticate
```

This writes `~/.claude/.credentials.json`. Set `token_source: auto` or `token_source: credentials_file` in your Rausu config.

### Option B: Environment variable

Set a static access token (no automatic refresh):

```bash
export CLAUDE_OAUTH_TOKEN="your-oauth-access-token"
```

Then set `token_source: env` in your Rausu config. This is useful for CI environments where you manage token rotation externally.

### Token source resolution for `auto`

1. `CLAUDE_OAUTH_TOKEN` environment variable (no refresh)
2. `~/.claude/.credentials.json` (with automatic refresh)

## Quick start

### 1. Authenticate with Claude CLI (if not already)

```bash
claude
```

### 2. Add to config.yaml

```yaml
models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto
```

> **Tip:** Use the exact upstream model ID as the virtual model name so that Claude Code picks it up without any additional configuration.

### 3. Start Rausu

```bash
rausu --config config.yaml
```

### 4. Test with curl

```bash
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Claude Code CLI

This is the primary use case: point Claude Code at Rausu to use your Claude subscription instead of a direct API key.

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
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto

  - name: claude-opus-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-opus-4-20250514
        token_source: auto
```

**2. Start Rausu**

```bash
./rausu --config config.yaml
```

**3. Point Claude Code at Rausu**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="fake-key"   # Rausu ignores this, but Claude Code requires it
claude -p "Hello via subscription"
```

Claude Code sends requests to `/v1/messages`, which is exactly what this provider handles.

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: claude-subscription
      model: <upstream-claude-model-id>        # Required
      token_source: auto                        # Required: auto | env | credentials_file
      credentials_path: /path/to/credentials.json  # Optional; default: ~/.claude/.credentials.json
```

### `token_source`

| Value | Behavior |
|---|---|
| `auto` | Try `CLAUDE_OAUTH_TOKEN` env var first, then `~/.claude/.credentials.json` |
| `env` | Use `CLAUDE_OAUTH_TOKEN` only (no refresh) |
| `credentials_file` | Use credentials file only (with refresh) |

### `credentials_path`

Overrides the default `~/.claude/.credentials.json` path. Useful when running Rausu as a different user or in a container.

## Upstream model names

Availability depends on your subscription tier (Pro vs Max). Confirmed model IDs as of 2026-Q1:

| Model ID | Description |
|---|---|
| `claude-opus-4-6` | Claude Opus 4.6 (most capable) |
| `claude-sonnet-4-6` | Claude Sonnet 4.6 |
| `claude-opus-4-20250514` | Claude Opus 4 |
| `claude-sonnet-4-20250514` | Claude Sonnet 4 |
| `claude-haiku-4-20250514` | Claude Haiku 4 (fastest) |
| `claude-sonnet-4-5-20251001` | Claude Sonnet 4.5 |
| `claude-haiku-3-20240307` | Claude 3 Haiku (legacy) |

Check the [Anthropic model documentation](https://docs.anthropic.com/en/docs/about-claude/models) for the latest IDs. Rausu passes whatever model name you configure directly to the API.

## Authentication internals

Rausu uses the Claude Code OAuth identity to access the subscription endpoint:

- **Beta headers:** `claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14`
- **User-Agent:** `claude-cli/2.1.75`
- **System prompt prefix:** `You are Claude Code, Anthropic's official CLI for Claude.` is prepended to the system prompt automatically.

Token refresh uses the OAuth metadata discovery endpoint at `https://claude.ai/oauth/claude-code-client-metadata` to locate the token endpoint dynamically. Tokens are **never logged**.

## Docker deployment

```bash
docker run \
  -v ~/.claude/.credentials.json:/app/credentials.json \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

In `config.yaml`, add `credentials_path: /app/credentials.json` or use the `CLAUDE_OAUTH_TOKEN` environment variable approach.

## Known limitations

- **No `/v1/chat/completions` support.** Use `provider: anthropic` with an API key for OpenAI-compatible format.
- **No `/v1/responses` support.**
- Subscription rate limits and model availability are controlled by Anthropic — Rausu propagates the upstream HTTP status code unchanged.
- Token refresh requires a valid refresh token in the credentials file. If only `CLAUDE_OAUTH_TOKEN` is set (env source), no refresh occurs and you must rotate the token manually.
- The system prompt prefix (`You are Claude Code...`) is always injected. This is required for the subscription endpoint to accept the request.
