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
- **Multi-provider** — supports OpenAI, Anthropic (API key), Claude Subscription (OAuth), and ChatGPT Subscription (OAuth)
- **Streaming** — full SSE streaming support
- **Single binary** — zero runtime dependencies
- **YAML configuration** — with environment variable interpolation
- **Structured logging** — JSON logs with request tracing

## Quickstart

### Option 1: From source

```bash
cargo build --release
./target/release/rausu --config config.yaml
```

### Option 2: Docker

```bash
docker build -t rausu .
docker run -p 4000:4000 -v $(pwd)/config.yaml:/app/config.yaml rausu
```

## Configuration

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

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/v1/models` | GET | List configured models |
| `/v1/chat/completions` | POST | Chat completions — routing + format translation |
| `/v1/responses` | POST | OpenAI Responses API — transparent passthrough (Codex CLI) |
| `/v1/responses/compact` | POST | OpenAI Responses API compact variant — transparent passthrough |
| `/v1/messages` | POST | Anthropic Messages API — transparent passthrough (Claude Code) |

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
