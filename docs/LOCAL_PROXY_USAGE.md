# Local Proxy Usage Guide

> [中文版](LOCAL_PROXY_USAGE_CN.md)

This guide explains how to run Rausu as a **local proxy** for AI coding tools such as Codex CLI and Claude Code. It covers configuration examples, fake-key behavior, supported endpoints, and current limitations.

---

## Overview

Rausu's **local runtime** is a single-user HTTP proxy that runs on your machine. Its primary purpose is:

- Inject real upstream credentials (OAuth tokens, API keys) so local clients don't need to hold them directly.
- Expose a unified OpenAI-compatible API surface that multiple tools can share.
- Provide transparent passthrough for native Responses API and Messages API traffic.
- Bridge protocols between clients and providers — Codex CLI can use Claude models, Claude Code can use GPT models.

```
  Codex CLI / Claude Code / any OpenAI client
         │  fake or placeholder API key
         ▼
  http://localhost:4000
         │  Rausu injects real upstream auth
         │  + protocol bridge when needed
         ▼
  OpenAI / Anthropic / Claude subscription / ChatGPT subscription / GitHub Copilot
```

---

## Starting Rausu Locally

```bash
# Build (first time only)
cargo build --release

# Run with your config
./target/release/rausu --config config.yaml
```

Or with `cargo run` during development:

```bash
cargo run -- --config config.yaml
```

Rausu listens on `http://localhost:4000` by default (configurable via `server.host` / `server.port`).

---

## Configuration Examples

Copy `config.example.yaml` as a starting point:

```bash
cp config.example.yaml config.yaml
```

### OpenAI API Key

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty   # pretty for local dev; json for production

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  - name: o3
    providers:
      - provider: openai
        model: o3
        api_key: "${OPENAI_API_KEY}"
```

Set the environment variable before starting:

```bash
export OPENAI_API_KEY="sk-..."
./target/release/rausu --config config.yaml
```

### ChatGPT Subscription (Plus / Pro / Max)

No API key required — uses your ChatGPT OAuth session.

```yaml
models:
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto   # tries env, then ~/.config/rausu/chatgpt-auth.json

  - name: gpt-5-codex
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.3-codex
        token_source: auto
```

**Token sources** (checked in order):

1. `env` — set `CHATGPT_ACCESS_TOKEN=<token>` (optionally also `CHATGPT_REFRESH_TOKEN`, `CHATGPT_ACCOUNT_ID`)
2. `credentials_file` — reads `~/.config/rausu/chatgpt-auth.json`; supports automatic token refresh
3. `auto` (default) — tries `env` first, then `credentials_file`

Credentials file format (`~/.config/rausu/chatgpt-auth.json`):

```json
{
  "access_token": "eyJ...",
  "refresh_token": "...",
  "expires_at": 1900000000000,
  "account_id": "acc_..."
}
```

Supported models: `gpt-5.4`, `gpt-5.4-pro`, `gpt-5.3-codex`, `gpt-5.3-codex-spark`, `gpt-5.3-instant`, `gpt-5.3-chat-latest`

### Claude Subscription (Pro / Max)

No API key required — uses your Claude OAuth session managed by the Claude CLI.

```yaml
models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto   # tries env, then ~/.claude/.credentials.json

  - name: claude-opus-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-opus-4-20250514
        token_source: credentials_file   # explicit: read from Claude CLI credentials
```

**Token sources** (checked in order):

1. `env` — set `CLAUDE_OAUTH_TOKEN=<access_token>` (static, no auto-refresh)
2. `credentials_file` — reads `~/.claude/.credentials.json` written by Claude CLI; supports automatic token refresh
3. `auto` (default) — tries `env` first, then `credentials_file`

The `credentials_file` source works automatically if you are logged in via Claude Code or the Claude CLI — no extra setup needed.

### Mixed-Model Config (All Providers)

A single Rausu config can expose multiple virtual model names backed by different providers:

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty

models:
  # OpenAI — via API key
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  # ChatGPT subscription — no API key
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto

  # Anthropic — via API key
  - name: claude-sonnet
    providers:
      - provider: anthropic
        model: claude-sonnet-4-20250514
        api_key: "${ANTHROPIC_API_KEY}"

  # Claude subscription — matches exact model name for Claude Code compatibility
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

> **Tip:** For Claude Code, naming the virtual model to match the real model ID (e.g., `claude-sonnet-4-20250514`) lets Claude Code's model picker work without any changes.

---

## Fake-Key / Local Auth Behavior

**Rausu ignores the API key sent by local clients.** Local tools (Codex CLI, Claude Code, curl, SDKs) typically require an API key field to be non-empty, but in local proxy mode it does not matter what value you set — Rausu does not validate it.

Instead, Rausu **injects the real upstream credentials** it loads from its own config (API keys via environment variables, OAuth tokens via credentials files or environment variables).

This means:
- You can set `OPENAI_API_KEY=fake`, `ANTHROPIC_API_KEY=fake`, or any placeholder when pointing a client at Rausu.
- Credentials never leave your machine in plain text through client config.
- Subscription-based auth (Claude OAuth, ChatGPT OAuth) works without the client knowing anything about OAuth tokens.

---

## Connecting Codex CLI

Codex CLI uses the OpenAI Responses API (`/v1/responses`) as its primary endpoint. Rausu implements this endpoint as a passthrough.

**Step 1 — Configure Rausu** with a model that Codex will request:

```yaml
models:
  - name: gpt-5.3-codex
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.3-codex
        token_source: auto
```

Or using an OpenAI API key:

```yaml
models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"
```

**Step 2 — Start Rausu:**

```bash
./target/release/rausu --config config.yaml
```

**Step 3 — Point Codex CLI at Rausu:**

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="local-proxy"   # any non-empty value
codex --model gpt-5.3-codex
```

Codex will send requests to `http://localhost:4000/v1/responses`, and Rausu will relay them upstream with the real credentials.

---

## Codex CLI with Claude Models (via Protocol Bridge)

Codex CLI can use Claude models via the GitHub Copilot provider. Rausu automatically bridges the Responses API request to the Anthropic Messages API format.

**Step 1 — Configure Rausu:**

```yaml
models:
  - name: claude-sonnet-4-6
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.6
    aliases:
      - claude-sonnet-4.6
```

**Step 2 — Start Rausu:**

```bash
./target/release/rausu --config config.yaml
```

**Step 3 — Point Codex CLI at Rausu:**

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="local-proxy"
codex --model claude-sonnet-4-6
```

Rausu bridges the `/v1/responses` request to Copilot's native `/v1/messages` endpoint, converts the response back, and streams events with zero buffering.

---

## Connecting Claude Code

Claude Code uses the Anthropic Messages API (`/v1/messages`) as its primary endpoint. Rausu implements this endpoint as a passthrough.

**Step 1 — Configure Rausu** with Claude model names matching what Claude Code expects:

```yaml
models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto   # reads ~/.claude/.credentials.json

  - name: claude-opus-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-opus-4-20250514
        token_source: auto
```

**Step 2 — Start Rausu:**

```bash
./target/release/rausu --config config.yaml
```

**Step 3 — Point Claude Code at Rausu:**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="local-proxy"   # any non-empty value
claude
```

Claude Code will send requests to `http://localhost:4000/v1/messages`, and Rausu will relay them to the Claude subscription endpoint with the real OAuth token.

> **Note:** `ANTHROPIC_BASE_URL` should be the base without `/v1` — Claude Code appends `/v1/messages` itself.

---

## Claude Code with GPT Models (via Protocol Bridge)

Claude Code can use GPT models via the ChatGPT subscription provider. Rausu automatically bridges the Messages API request to the Responses API format.

**Step 1 — Configure Rausu:**

```yaml
models:
  - name: gpt-5.4
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto
```

**Step 2 — Start Rausu:**

```bash
./target/release/rausu --config config.yaml
```

**Step 3 — Point Claude Code at Rausu:**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="local-proxy"
claude
```

In Claude Code's model picker, select `gpt-5.4`. Rausu bridges the `/v1/messages` request to ChatGPT's native Responses API, converts the response back, and streams events with zero buffering. Tool calling is fully supported.

---

## Supported Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/health` | Health check |
| `GET` | `/v1/models` | List configured models |
| `POST` | `/v1/chat/completions` | OpenAI Chat Completions — routing + format translation |
| `POST` | `/v1/responses` | OpenAI Responses API — transparent passthrough (Codex CLI) |
| `POST` | `/v1/responses/compact` | OpenAI Responses API compact variant — transparent passthrough |
| `POST` | `/v1/messages` | Anthropic Messages API — transparent passthrough (Claude Code) |

**Passthrough vs. protocol bridge:**
- `/v1/responses` — forwarded as-is when the upstream supports Responses API natively (OpenAI, ChatGPT subscription, Copilot GPT models). For Claude models via Copilot, Rausu bridges Responses→Messages automatically.
- `/v1/messages` — forwarded as-is for Claude providers. For GPT models via ChatGPT subscription, Rausu bridges Messages→Responses automatically.
- `/v1/chat/completions` — routed through the provider abstraction layer; Rausu normalizes the request/response format as needed.

---

## Current Limitations

The following are known limitations of the current local runtime. They are intentional for this phase and will be addressed in later phases.

| Limitation | Details |
|------------|---------|
| **No automatic base_url takeover** | Clients must set `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` manually. Transparent system-wide interception (e.g., proxying via `/etc/hosts` or system proxy settings) is not yet implemented. |
| **Single-user only** | No virtual API keys, no per-user routing, no spend tracking. One config file serves one user's local session. |
| **No routing or fallback** | Each virtual model maps to a single provider deployment. Multi-provider fallback and load balancing are not yet implemented. |
| **No admin UI** | Configuration is file-based only. |
| **No rate limiting or budget enforcement** | Requests are forwarded without local quotas. |
| **Responses API: provider support varies** | `/v1/responses` passthrough works natively for OpenAI and ChatGPT subscription. For Claude models via Copilot, a protocol bridge is used. Providers without Responses API support and without a bridge will return an unsupported error. |

---

## Tips

- Use `format: pretty` in `logging` during local development for human-readable logs.
- Set `level: debug` to see request/response details.
- Run `curl http://localhost:4000/health` to verify Rausu is up.
- Run `curl http://localhost:4000/v1/models` to verify your model names are registered.
