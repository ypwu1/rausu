# GitHub Copilot Provider

> **Chinese version:** [GITHUB_COPILOT_PROVIDER_CN.md](GITHUB_COPILOT_PROVIDER_CN.md)

## Overview

The `github-copilot` provider lets you route requests through your GitHub Copilot
subscription without an API key.  Rausu exchanges your GitHub OAuth device-flow
token for a short-lived Copilot API token automatically, then proxies
OpenAI-compatible chat completions to `https://api.githubcopilot.com`.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | ✅ (streaming + non-streaming) |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/messages` | ❌ Copilot does not expose the Anthropic Messages API |
| `POST /v1/responses` | ❌ Copilot does not expose the OpenAI Responses API |

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
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o

  - name: copilot-claude-sonnet
    providers:
      - provider: github-copilot
        model: claude-3.5-sonnet
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
    "model": "copilot-gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: github-copilot
      model: <upstream-copilot-model>   # Required
      credentials_path: /path/to/hosts.json  # Optional; default: ~/.config/github-copilot/hosts.json
```

### `credentials_path`

Overrides the default `~/.config/github-copilot/hosts.json` path.

## Upstream model names

Model availability depends on your Copilot subscription tier.  Confirmed working
names as of 2025-Q1:

| Model ID | Description |
|---|---|
| `gpt-4o` | OpenAI GPT-4o via Copilot |
| `gpt-4o-mini` | OpenAI GPT-4o Mini |
| `claude-3.5-sonnet` | Anthropic Claude 3.5 Sonnet via Copilot |
| `o1-mini` | OpenAI o1-mini reasoning model |
| `o3-mini` | OpenAI o3-mini (where available) |

Copilot may return `404` or `400` for models not enabled on your plan.

## Authentication internals

Token exchange is fully automatic:

```
hosts.json (ghu_...)  →  GET /copilot_internal/v2/token  →  Copilot API token (TTL ~30 min)
```

Rausu caches the Copilot API token and re-exchanges it 5 minutes before expiry.
Tokens are **never logged**.

## Known limitations

- **No Messages API passthrough** (`/v1/messages`).  Use `provider: anthropic` or
  `provider: claude-subscription` for Anthropic-native routing.
- **No Responses API passthrough** (`/v1/responses`).
- Copilot rate limits and model availability are controlled by GitHub — Rausu
  propagates the upstream HTTP status code unchanged.
- Tool/function calling support depends on the upstream Copilot model.
- The `base_url` config field is ignored for this provider; the endpoint is
  determined by the token exchange response (or defaults to
  `https://api.githubcopilot.com`).
