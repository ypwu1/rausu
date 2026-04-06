<p align="center">
  <img src="assets/icon.jpg" width="160" alt="Rausu Icon" />
</p>

<h1 align="center">Rausu</h1>
<p align="center"><em>ラウス</em></p>

<p align="center">
  <a href="./README_CN.md">中文版</a>
</p>

A high-performance LLM API Gateway written in Rust — a drop-in replacement for [LiteLLM Proxy](https://github.com/BerriAI/litellm) with better performance, smaller footprint, and simpler deployment (single binary).

## Features

- **OpenAI-compatible API** — works with any OpenAI SDK client
- **Multi-provider** — supports OpenAI, Anthropic (API key), Claude Subscription (OAuth), GitHub Copilot, ChatGPT Subscription (OAuth), and any OpenAI-compatible provider (DeepSeek, Qwen, Ollama, GLM, Moonshot, etc.)
- **Protocol Bridge** — bi-directional conversion between OpenAI Responses API and Anthropic Messages API; Codex CLI can use Claude models or any OpenAI-compatible provider, Claude Code can use GPT models or any OpenAI-compatible provider
- **True SSE Streaming** — zero-buffer per-event streaming for all paths including protocol bridge (first-token latency matches passthrough)
- **Streaming** — full SSE streaming support
- **Single binary** — zero runtime dependencies
- **YAML configuration** — with environment variable interpolation
- **API key authentication** — optional static-key auth to protect remotely-exposed proxies
- **Structured logging** — JSON logs with request tracing

## Quickstart

### Option 1: From source

```bash
cargo build --release

# Generate a template config (written to ~/.config/rausu/config.yaml)
./target/release/rausu init
# Edit it, then start:
./target/release/rausu
```

Alternatively, point to an explicit config file:

```bash
./target/release/rausu --config config.yaml
```

### Option 2: Docker

```bash
docker build -t rausu .
docker run -p 4000:4000 -v $(pwd)/config.yaml:/app/config.yaml rausu
```

## Configuration

### Auto-discovery

When you run `rausu` without `--config`, it searches for a config file in order:

1. `RAUSU_CONFIG` environment variable
2. `./config.yaml`
3. `./rausu-config.yaml`
4. `${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml`
5. `${XDG_CONFIG_HOME:-~/.config}/rausu/rausu-config.yaml`
6. `~/.rausu/config.yaml`
7. `~/rausu-config.yaml`

If no file is found, a commented template is written to
`${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml` and the process exits with a
message telling you to edit it.

### `rausu init`

```bash
rausu init                    # write template to ~/.config/rausu/config.yaml
rausu init --path ./my.yaml   # write to a custom path
rausu init --force            # overwrite an existing file
```

### `rausu setup`

Interactive config editor — create or edit configs without writing YAML by hand:

```bash
rausu setup                    # create or edit at default location
rausu setup --path ./my.yaml   # target a specific file
```

The editor is model-centric: you create a virtual model first, then attach provider deployments with failover ordering. It supports adding, editing, deleting, and reordering both models and providers. Existing configs are loaded automatically.

Pre-save validation checks for errors (unknown providers, missing fields, duplicates) and warnings (missing credentials, unreachable endpoints). See [docs/SETUP_EDITOR.md](docs/SETUP_EDITOR.md) for details.

### `rausu check`

Validate your configuration and test provider connectivity:

```bash
rausu check                    # use auto-discovered config
rausu check --config my.yaml   # use a specific config file
```

Example output:

```
📋 Config: ~/.config/rausu/config.yaml
   Server: 127.0.0.1:4000
   Auth: static (2 keys)

📦 Models (3):
   ✓ gpt-5.4 → chatgpt-subscription
   ✓ claude-opus-4.6 → github-copilot
   ✓ deepseek-chat → openai (https://api.deepseek.com/v1)

🔌 Connectivity:
   ✓ chatgpt-subscription: token available (codex auth)
   ✓ github-copilot: hosts.json found (~/.config/github-copilot/hosts.json)
   ✓ openai (https://api.deepseek.com/v1): reachable (HTTP 200)
   ✗ openai (http://localhost:11434/v1): connection refused

✅ 3/4 providers OK
```

The check runs four steps: config loading, model validation (required fields, valid provider types), provider connectivity (HTTP reachability or credential file existence), and auth verification.

> **Startup validation**: the same validation logic runs automatically when `rausu` starts in server mode. Hard errors (unknown providers, missing required fields, duplicate names) block startup. Warnings (missing credentials, unreachable endpoints) are logged but allow the server to proceed.

### Manual setup

Copy `config.example.yaml` and customise:

```bash
cp config.example.yaml config.yaml
# Edit config.yaml with your API keys
```

```yaml
server:
  host: 0.0.0.0
  port: 4000

logging:
  level: info
  format: json   # json | pretty

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  - name: claude-sonnet
    providers:
      - provider: anthropic
        model: claude-sonnet-4-20250514
        api_key: "${ANTHROPIC_API_KEY}"

  # Claude Pro/Max subscription — no API key required
  - name: claude-sonnet-sub
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        # token_source: auto   # auto (default) | env | credentials_file
        # credentials_path: /custom/path/.credentials.json  # optional

  # ChatGPT Plus/Pro/Max subscription — no API key required
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        # token_source: auto   # auto (default) | env | credentials_file
        # credentials_path: ~/.config/rausu/chatgpt-auth.json  # optional
```

### `claude-subscription` provider

Uses your Claude Pro/Max subscription via OAuth instead of a paid API key.

**Token sources (checked in priority order):**

1. **`env`** — set `CLAUDE_OAUTH_TOKEN=<access_token>` (static, no refresh)
2. **`credentials_file`** — reads `~/.claude/.credentials.json` written by the Claude CLI; supports automatic token refresh
3. **`auto`** (default) — tries `env` first, then `credentials_file`

```yaml
models:
  - name: claude-sonnet-sub
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: credentials_file          # optional, default: auto
        # credentials_path: ~/.claude/.credentials.json  # optional override
```

### `chatgpt-subscription` provider

Uses your ChatGPT Plus/Pro/Max subscription via OAuth instead of a paid API key. Requests are bridged internally from Chat Completions format to the ChatGPT Responses API.

**Token sources (checked in priority order):**

1. **`env`** — set `CHATGPT_ACCESS_TOKEN=<access_token>` (optionally also `CHATGPT_REFRESH_TOKEN` and `CHATGPT_ACCOUNT_ID`)
2. **`credentials_file`** — reads `~/.config/rausu/chatgpt-auth.json`; supports automatic token refresh
3. **`auto`** (default) — tries `env` first, then `credentials_file`

```yaml
models:
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: env              # optional, default: auto

  - name: gpt-5-pro
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4-pro
        token_source: credentials_file
        credentials_path: /custom/path/chatgpt-auth.json  # optional override
```

**Credentials file format** (`~/.config/rausu/chatgpt-auth.json`):

```json
{
  "access_token": "eyJ...",
  "refresh_token": "...",
  "expires_at": 1900000000000,
  "account_id": "acc_..."
}
```

**Supported models:** `gpt-5.4`, `gpt-5.4-pro`, `gpt-5.3-codex`, `gpt-5.3-codex-spark`, `gpt-5.3-instant`, `gpt-5.3-chat-latest`

> **Note:** All four providers (`openai`, `anthropic`, `claude-subscription`, `chatgpt-subscription`) are completely independent and can coexist in the same config, serving different virtual model names.

### Authentication

Rausu supports optional API key authentication to protect a remotely-exposed proxy. Two modes are available:

- **`disabled`** (default) — no authentication; all requests are forwarded.
- **`static`** — incoming requests must carry a valid `Authorization: Bearer <key>` header matching one of the configured keys.

```yaml
auth:
  mode: static
  keys:
    - name: "my-laptop"
      key: "rausu-sk-abc123"
    - name: "remote-client"
      key: "${RAUSU_API_KEY}"    # supports env var interpolation
```

Key values support `${ENV_VAR}` interpolation. The recommended key prefix convention is `rausu-sk-`.

The `/health` endpoint is always exempt from authentication.

If the `auth` section is omitted entirely, authentication defaults to `disabled`.

Environment variable overrides use the `RAUSU__` prefix with `__` as separator:

```bash
RAUSU__SERVER__PORT=8080 rausu
```

## Usage

Point your OpenAI SDK at `http://localhost:4000`:

```python
from openai import OpenAI

client = OpenAI(
    api_key="not-used",
    base_url="http://localhost:4000/v1",
)

# Route to OpenAI
response = client.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "Hello!"}],
)

# Route to Anthropic (same API!)
response = client.chat.completions.create(
    model="claude-sonnet",
    messages=[{"role": "user", "content": "Hello!"}],
)
```

## Client × Model Matrix

All client-model combinations are supported via passthrough or protocol bridge:

| Client | Protocol | Target | Path |
|--------|---------|--------|------|
| Claude Code | `/v1/messages` | Claude (Copilot) | Passthrough |
| Claude Code | `/v1/messages` | Claude (Anthropic) | Passthrough |
| Claude Code | `/v1/messages` | GPT (ChatGPT sub) | Messages→Responses bridge |
| Claude Code | `/v1/messages` | Any OpenAI-compatible | Messages→Responses→ChatCompletions |
| Codex CLI | `/v1/responses` | GPT (ChatGPT sub) | Passthrough |
| Codex CLI | `/v1/responses` | GPT (Copilot) | Passthrough |
| Codex CLI | `/v1/responses` | Claude (Copilot) | Responses→Messages bridge |
| Codex CLI | `/v1/responses` | Any OpenAI-compatible | Responses→ChatCompletions bridge |

See [docs/PROTOCOL_BRIDGE_PLAN.md](docs/PROTOCOL_BRIDGE_PLAN.md) for protocol conversion details.

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/v1/models` | GET | List configured models |
| `/v1/chat/completions` | POST | Chat completions — routing + format translation |
| `/v1/responses` | POST | OpenAI Responses API — passthrough or Responses→Messages bridge |
| `/v1/responses/compact` | POST | OpenAI Responses API compact variant — transparent passthrough |
| `/v1/messages` | POST | Anthropic Messages API — passthrough or Messages→Responses bridge |

> **Note:** All `/v1/...` routes are also available without the prefix (e.g. `/responses`, `/chat/completions`, `/models`, `/messages`). This allows clients like Codex CLI that use `{base_url}/responses` instead of `{base_url}/v1/responses` to work without extra configuration.

## Local Proxy Usage

Rausu can run locally as a single-user proxy for Codex CLI and Claude Code. Local clients pass a placeholder API key; Rausu injects the real upstream credentials automatically.

See [docs/LOCAL_PROXY_USAGE.md](docs/LOCAL_PROXY_USAGE.md) for a full guide covering config examples, fake-key behavior, and connection instructions for Codex CLI and Claude Code.

## Architecture

See [docs/ARCHITECTURE_DIRECTION.md](docs/ARCHITECTURE_DIRECTION.md) for the full architecture decision record (local-first, gateway-compatible design).

## Build

Requirements: Rust 1.70+

```bash
cargo build --release
cargo test
cargo clippy
```

## License

MIT — see [LICENSE](./LICENSE)
