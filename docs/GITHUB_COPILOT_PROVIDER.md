# GitHub Copilot Provider

> **Chinese version:** [GITHUB_COPILOT_PROVIDER_CN.md](GITHUB_COPILOT_PROVIDER_CN.md)

## Overview

The `github-copilot` provider lets you route requests through your GitHub Copilot
subscription without an API key.  Rausu exchanges your GitHub OAuth device-flow
token for a short-lived Copilot API token automatically, then proxies requests to
`https://api.githubcopilot.com`.

Claude models are forwarded directly to Copilot's native `/v1/messages` endpoint
(Anthropic Messages API passthrough — no protocol conversion).  All other models
use the OpenAI-compatible `/chat/completions` endpoint.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | ✅ (streaming + non-streaming) |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/messages` | ✅ Claude: native passthrough; others: protocol-translated |
| `POST /v1/responses` | ❌ not supported |

## Prerequisites

You need a GitHub account with an active Copilot subscription (Individual, Business,
or Enterprise).

The token is loaded from `~/.config/github-copilot/hosts.json`, which is written
by `gh auth login` or the Copilot VS Code / JetBrains extension.

> **Note:** Environment variables like `GH_TOKEN` / `GITHUB_TOKEN` are intentionally
> **not** supported. They often contain PATs (personal access tokens) which are
> incompatible with the Copilot internal token exchange endpoint. Only device-flow
> tokens (`ghu_...`) from `hosts.json` are supported.

The `hosts.json` file looks like:

```json
{
  "github.com": {
    "user": "your-username",
    "oauth_token": "ghu_..."
  }
}
```

## Quick start

### 1. Authenticate (if not already)

```bash
# Using GitHub CLI (recommended):
gh auth login --scopes read:user
```

### 2. Add to config.yaml

```yaml
models:
  # Claude models — native /v1/messages passthrough
  - name: claude-sonnet-4-6
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.6

  # OpenAI models — /chat/completions with protocol translation
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o
```

### 3. Start Rausu

```bash
rausu --config config.yaml
```

### 4. Send a request

```bash
# Claude model via /v1/messages (native passthrough)
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: any-key" \
  -d '{
    "model": "claude-sonnet-4-6",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# OpenAI model via /v1/chat/completions
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "copilot-gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  aliases:                               # Optional; alternative names
    - <alias-1>
  providers:
    - provider: github-copilot
      model: <upstream-copilot-model>   # Required
      credentials_path: /path/to/hosts.json  # Optional; default: ~/.config/github-copilot/hosts.json
```

### `credentials_path`

Overrides the default `~/.config/github-copilot/hosts.json` path.

## Supported models reference

### Claude models (via `/v1/messages` — native Anthropic passthrough)

| Config `name` (client sends) | Config `model` (Copilot ID) | Category | Features |
|---|---|---|---|
| `claude-opus-4-6` | `claude-opus-4.6` | powerful | vision, thinking, tools, streaming |
| `claude-sonnet-4-6` | `claude-sonnet-4.6` | versatile | vision, thinking, tools, streaming |
| `claude-sonnet-4-5` | `claude-sonnet-4.5` | versatile | vision, thinking, tools, streaming |
| `claude-opus-4-5` | `claude-opus-4.5` | powerful | vision, thinking, tools, streaming |
| `claude-sonnet-4` | `claude-sonnet-4` | versatile | vision, thinking, tools, streaming |
| `claude-haiku-4-5` | `claude-haiku-4.5` | versatile | vision, thinking, tools, streaming |

### OpenAI models (via `/chat/completions`)

| Config `name` | Config `model` (Copilot ID) | Category | Features |
|---|---|---|---|
| `gpt-5.4` | `gpt-5.4` | powerful | vision, tools, streaming, reasoning |
| `gpt-5.2` | `gpt-5.2` | versatile | vision, tools, streaming, reasoning |
| `gpt-5.1` | `gpt-5.1` | versatile | vision, tools, streaming, reasoning |
| `gpt-5-mini` | `gpt-5-mini` | lightweight | vision, tools, streaming, reasoning |
| `gpt-4.1` | `gpt-4.1` | versatile | vision, tools, streaming |
| `gpt-4o` | `gpt-4o` | versatile | vision, tools, streaming |

### Google models (via `/chat/completions`)

| Config `name` | Config `model` (Copilot ID) | Category | Features |
|---|---|---|---|
| `gemini-2.5-pro` | `gemini-2.5-pro` | powerful | vision, thinking, tools, streaming |
| `gemini-3.1-pro-preview` | `gemini-3.1-pro-preview` | powerful | vision, thinking, tools, streaming (preview) |
| `gemini-3-flash-preview` | `gemini-3-flash-preview` | lightweight | vision, thinking, tools, streaming (preview) |

### xAI models (via `/chat/completions`)

| Config `name` | Config `model` (Copilot ID) | Category |
|---|---|---|
| `grok-code-fast-1` | `grok-code-fast-1` | lightweight |

### Codex models (via `/responses` only — not yet supported by Rausu)

| Copilot ID | Category | Note |
|---|---|---|
| `gpt-5.2-codex` | powerful | /responses only |
| `gpt-5.3-codex` | powerful | /responses only |
| `gpt-5.4-mini` | lightweight | /responses only |

### Notes

- **Claude models use native `/v1/messages` passthrough** — no protocol conversion,
  full feature support (thinking, tools, vision, streaming).
- **Copilot model IDs use dots** (`claude-opus-4.6`) while **Claude Code uses hyphens**
  (`claude-opus-4-6`).  Use `aliases` in your config to accept both naming conventions.
- Model availability depends on your Copilot subscription tier.  Copilot may return
  `404` or `400` for models not enabled on your plan.

### Complete config example

```yaml
models:
  # ── Claude models (native /v1/messages passthrough) ──────────────────────
  - name: claude-opus-4-6
    providers:
      - provider: github-copilot
        model: claude-opus-4.6

  - name: claude-sonnet-4-6
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.6

  - name: claude-sonnet-4-5
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.5

  - name: claude-opus-4-5
    providers:
      - provider: github-copilot
        model: claude-opus-4.5

  - name: claude-sonnet-4
    providers:
      - provider: github-copilot
        model: claude-sonnet-4

  - name: claude-haiku-4-5
    providers:
      - provider: github-copilot
        model: claude-haiku-4.5

  # ── OpenAI models (/chat/completions with protocol translation) ──────────
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o

  - name: copilot-gpt-5.4
    providers:
      - provider: github-copilot
        model: gpt-5.4

  # ── Google models (/chat/completions) ────────────────────────────────────
  - name: copilot-gemini-2.5-pro
    providers:
      - provider: github-copilot
        model: gemini-2.5-pro
```

## Authentication internals

Token exchange is fully automatic:

```
hosts.json (ghu_...)  →  GET /copilot_internal/v2/token  →  Copilot API token (TTL ~30 min)
```

Rausu caches the Copilot API token and re-exchanges it 5 minutes before expiry.
Tokens are **never logged**.

## Known limitations

- **No Responses API passthrough** (`/v1/responses`).
- Copilot rate limits and model availability are controlled by GitHub — Rausu
  propagates the upstream HTTP status code unchanged.
- Tool/function calling support depends on the upstream Copilot model.
- The `base_url` config field is ignored for this provider; the endpoint is
  determined by the token exchange response (or defaults to
  `https://api.githubcopilot.com`).
