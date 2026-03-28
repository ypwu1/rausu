# Rausu (ラウス)

> [中文版](./README_CN.md)

A high-performance LLM API Gateway written in Rust — a drop-in replacement for [LiteLLM Proxy](https://github.com/BerriAI/litellm) with better performance, smaller footprint, and simpler deployment (single binary).

## Features

- **OpenAI-compatible API** — works with any OpenAI SDK client
- **Multi-provider** — supports OpenAI and Anthropic (Phase 1); more providers coming in later phases
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
```

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
| `/v1/chat/completions` | POST | Chat completions (streaming & non-streaming) |

## Build

Requirements: Rust 1.70+

```bash
cargo build --release
cargo test
cargo clippy
```

## License

MIT — see [LICENSE](./LICENSE)
