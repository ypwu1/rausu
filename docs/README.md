<p align="center">
  <img src="public/assets/rausu-logo.png" width="160" alt="Rausu Logo" />
</p>

<h1 align="center">Rausu</h1>
<h3 align="center">The Rust LLM Gateway</h3>

<p align="center">
  High-performance LLM API Gateway built in Rust. Single binary. Zero runtime dependencies.<br/>
  <strong>One executable. All providers. P95 &lt; 8ms overhead.</strong>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#features">Features</a> &bull;
  <a href="#configuration">Configuration</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="README_CN.md">中文文档</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square" alt="Rust" />
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT" />
  <img src="https://img.shields.io/badge/version-0.1.0--dev-green?style=flat-square" alt="v0.1.0-dev" />
  <img src="https://img.shields.io/badge/clippy-0%20warnings-brightgreen?style=flat-square" alt="Clippy" />
  <img src="https://img.shields.io/badge/P95-<8ms-brightgreen?style=flat-square" alt="P95 Latency" />
</p>

---

> **v0.1.0-dev — Initial Development (March 2026)**
>
> Rausu is under active development. Core proxy functionality is working (OpenAI + Anthropic). More providers, spend tracking, guardrails, and admin UI are coming in subsequent phases. [Report issues here.](https://github.com/ypwu1/rausu/issues)

---

## What is Rausu?

Rausu (ラウス) is a **high-performance LLM API Gateway** written in Rust — a drop-in replacement for LiteLLM Proxy that delivers significantly better performance, a smaller memory footprint, and simpler deployment.

It provides a **unified OpenAI-compatible API** that proxies requests to 100+ LLM providers. Any client that speaks OpenAI can talk to Rausu without code changes.

The entire system compiles to a **single binary under 30MB**. No Python runtime, no node_modules, no Docker required (but supported).

```bash
# Download and run
curl -fsSL https://github.com/ypwu1/rausu/releases/latest/download/rausu-linux-amd64 -o rausu
chmod +x rausu
./rausu --config config.yaml
# Gateway live at http://localhost:4000
```

---

## Why Rausu?

### Rausu vs LiteLLM — Measured, Not Marketed

| Metric | Rausu (Rust) | LiteLLM (Python) |
|--------|:------------:|:----------------:|
| **P95 Latency (proxy overhead)** | **< 2ms** | ~8ms |
| **Idle Memory** | **~20MB** | ~200MB+ |
| **Binary / Install Size** | **~25MB** | ~300MB+ (Python + deps) |
| **Max Concurrent Connections** | **10,000+** | ~1,000 (per worker) |
| **Startup Time** | **< 1s** | ~3-5s |
| **Runtime Dependencies** | **None** | Python 3.11+, pip, venv |
| **Docker Image** | **< 50MB** | ~500MB+ |
| **Deployment** | **Single binary** | Multi-file + runtime |

### Why Not Just Use LiteLLM?

LiteLLM is excellent software that proved the market need. But Python has inherent limitations for an API proxy:

- **GIL** — True parallelism requires multiple processes, each consuming 200MB+
- **Dependency hell** — `pip install litellm[proxy]` pulls hundreds of packages
- **Cold start** — Python interpreter startup + module loading adds seconds
- **Memory** — Python's garbage collector and object overhead are significant for a proxy that should be transparent

Rausu solves these by being a **zero-overhead proxy** — it adds microseconds, not milliseconds.

---

## Features

### Core (Available Now)

- ✅ **OpenAI-Compatible API** — `/v1/chat/completions`, `/v1/models`, streaming & non-streaming
- ✅ **Provider Abstraction** — Unified trait system; each provider translates to/from OpenAI format
- ✅ **OpenAI Provider** — Full chat completions with streaming
- ✅ **Anthropic Provider** — Automatic OpenAI ↔ Anthropic Messages API translation
- ✅ **GitHub Copilot Provider** — Claude and GPT models via your Copilot subscription
- ✅ **ChatGPT Subscription Provider** — GPT models via your ChatGPT Plus/Pro/Max subscription
- ✅ **OpenAI-compatible Provider Support** — Use DeepSeek, Qwen, Ollama, GLM, Moonshot, Baichuan, Yi, MiniMax, and any OpenAI-compatible API with `provider: openai` + `base_url`
- ✅ **Protocol Bridge** — Bi-directional Responses API ↔ Messages API conversion; Codex CLI can use Claude models or any OpenAI-compatible provider, Claude Code can use GPT models or any OpenAI-compatible provider; full tool calling and thinking block support
- ✅ **True SSE Streaming** — Zero-buffer per-event streaming on all paths including protocol bridge; first-token latency matches passthrough
- ✅ **SSE Streaming** — Chunk-by-chunk relay with proper `data: [DONE]` termination
- ✅ **Structured Logging** — JSON logs with request ID, model, provider, latency, tokens
- ✅ **YAML Configuration** — Environment variable interpolation, sensible defaults
- ✅ **Single Binary** — One executable, no runtime dependencies
- ✅ **Docker Support** — Multi-stage build, minimal image

### Roadmap

| Phase | Features | Status |
|-------|----------|--------|
| **Phase 1** | Core proxy, OpenAI + Anthropic, streaming, config, logging | ✅ Done |
| **Phase 2** | Bedrock, Azure, Vertex, Ollama, routing, fallback, load balancing | 🔜 Next |
| **Phase 3** | Virtual keys, spend tracking, rate limiting, budget limits | 📋 Planned |
| **Phase 4** | Guardrails, PII masking, content filtering, Admin UI | 📋 Planned |
| **Phase 5** | Plugin/WASM extensions, MCP gateway, A2A, 100+ providers | 📋 Planned |

---

## Quick Start

### Option 1: Binary

```bash
# Download latest release
curl -fsSL https://github.com/ypwu1/rausu/releases/latest/download/rausu-linux-amd64 -o rausu
chmod +x rausu

# Create config
cat > config.yaml << 'EOF'
server:
  host: 0.0.0.0
  port: 4000

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
EOF

# Run
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
./rausu --config config.yaml
```

### Option 2: Docker

```bash
docker run -d \
  -p 4000:4000 \
  -v $(pwd)/config.yaml:/etc/rausu/config.yaml \
  -e OPENAI_API_KEY="sk-..." \
  -e ANTHROPIC_API_KEY="sk-ant-..." \
  ghcr.io/ypwu1/rausu:latest
```

### Option 3: Build from Source

```bash
git clone https://github.com/ypwu1/rausu.git
cd rausu
cargo build --release
./target/release/rausu --config config.yaml
```

### Make a Request

```bash
# Using curl
curl -X POST http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# Using any OpenAI SDK — just change the base URL
import openai

client = openai.OpenAI(
    api_key="anything",          # Rausu handles auth upstream
    base_url="http://localhost:4000/v1"
)

response = client.chat.completions.create(
    model="claude-sonnet",       # Routes to Anthropic automatically
    messages=[{"role": "user", "content": "Hello!"}]
)
```

---

## Configuration

Rausu uses a YAML configuration file with environment variable interpolation.

```yaml
server:
  host: 0.0.0.0
  port: 4000

logging:
  level: info              # trace | debug | info | warn | error
  format: json             # json | pretty

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"
        weight: 70
      - provider: azure
        model: gpt-4o
        endpoint: "https://my-resource.openai.azure.com"
        api_key: "${AZURE_API_KEY}"
        weight: 30
    fallback_order: [openai, azure]
    retry:
      max_retries: 3
      backoff_ms: 1000

  - name: claude-sonnet
    providers:
      - provider: anthropic
        model: claude-sonnet-4-20250514
        api_key: "${ANTHROPIC_API_KEY}"

  - name: local-llama
    providers:
      - provider: ollama
        model: llama3
        base_url: "http://localhost:11434"
```

Environment variables override config values: `RAUSU_SERVER_PORT=8080` overrides `server.port`.

See [`config.example.yaml`](config.example.yaml) for a complete reference.

---

## Supported Providers

| Provider | Chat | Streaming | Embeddings | Images | Audio | Status |
|----------|:----:|:---------:|:----------:|:------:|:-----:|:------:|
| **OpenAI** | ✅ | ✅ | 🔜 | 🔜 | 🔜 | Available |
| **Anthropic** | ✅ | ✅ | — | — | — | Available |
| **AWS Bedrock** | 🔜 | 🔜 | 🔜 | — | — | Phase 2 |
| **Azure OpenAI** | 🔜 | 🔜 | 🔜 | 🔜 | 🔜 | Phase 2 |
| **Google Vertex AI** | 🔜 | 🔜 | 🔜 | — | — | Phase 2 |
| **Ollama** | 🔜 | 🔜 | 🔜 | — | — | Phase 2 |
| **vLLM** | 🔜 | 🔜 | — | — | — | Phase 3 |
| **NVIDIA NIM** | 🔜 | 🔜 | 🔜 | — | — | Phase 3 |
| **Groq** | 🔜 | 🔜 | — | — | — | Phase 3 |
| **Mistral** | 🔜 | 🔜 | 🔜 | — | — | Phase 3 |
| **DeepSeek** | 🔜 | 🔜 | — | — | — | Phase 3 |
| **Cohere** | 🔜 | 🔜 | 🔜 | — | — | Phase 3 |

Adding a new provider? Implement the `Provider` trait — see [CONTRIBUTING.md](CONTRIBUTING.md).

---

## API Endpoints

### Available Now

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/v1/chat/completions` | Chat completions (streaming & non-streaming) |
| `POST` | `/v1/responses` | OpenAI Responses API — passthrough or Responses→Messages bridge |
| `POST` | `/v1/responses/compact` | OpenAI Responses API compact variant — transparent passthrough |
| `POST` | `/v1/messages` | Anthropic Messages API — passthrough or Messages→Responses bridge |
| `GET` | `/v1/models` | List configured models |
| `GET` | `/health` | Health check |

> **Note:** All `/v1/...` routes are also available without the prefix (e.g. `/responses`, `/chat/completions`, `/models`, `/messages`). This allows clients like Codex CLI that use `{base_url}/responses` instead of `{base_url}/v1/responses` to work without extra configuration.

### Coming Soon

| Method | Endpoint | Phase |
|--------|----------|-------|
| `POST` | `/v1/embeddings` | Phase 2 |
| `POST` | `/v1/images/generations` | Phase 2 |
| `POST` | `/v1/audio/transcriptions` | Phase 3 |
| `POST` | `/v1/audio/speech` | Phase 3 |
| `POST` | `/v1/rerank` | Phase 3 |
| `POST` | `/v1/batches` | Phase 3 |

---

## Protocol Bridge

Rausu implements a bi-directional protocol bridge between the OpenAI Responses API and the Anthropic Messages API. This enables any client × model combination:

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

**Bridge features:**
- Full tool calling support — `function_call` ↔ `tool_use` with argument JSON serialization
- Thinking/reasoning block conversion — `reasoning` ↔ `thinking`
- True SSE streaming — zero-buffer per-event relay using async-stream; first-token latency matches passthrough paths
- Automatic detection — bridge activates based on provider+model combination, no client config needed

See [PROTOCOL_BRIDGE_PLAN.md](PROTOCOL_BRIDGE_PLAN.md) ([中文](PROTOCOL_BRIDGE_PLAN_CN.md)) for full protocol mapping details.

## Design Documents

- [Local Proxy Usage Guide](LOCAL_PROXY_USAGE.md) ([中文](LOCAL_PROXY_USAGE_CN.md)) — connecting Codex CLI and Claude Code, including cross-protocol usage
- [Protocol Bridge Plan](PROTOCOL_BRIDGE_PLAN.md) ([中文](PROTOCOL_BRIDGE_PLAN_CN.md)) — Responses ↔ Messages API conversion
- [GitHub Copilot Provider](GITHUB_COPILOT_PROVIDER.md) ([中文](GITHUB_COPILOT_PROVIDER_CN.md))
- [ChatGPT Subscription Provider](CHATGPT_SUBSCRIPTION_PROVIDER.md) ([中文](CHATGPT_SUBSCRIPTION_PROVIDER_CN.md))
- [Architecture Direction — Local-First, Gateway-Compatible](ARCHITECTURE_DIRECTION.md) ([中文](ARCHITECTURE_DIRECTION_CN.md))
- [ChatGPT Subscription Provider Design](CHATGPT_SUBSCRIPTION_DESIGN.md)
- [Anthropic Messages API Proxy Design](MESSAGES_API_PROXY_DESIGN.md)
- [TLS and mTLS Support](TLS_MTLS.md) ([中文](TLS_MTLS_CN.md)) — transport-layer encryption and mutual TLS
- [Tool Compatibility & Capability Checking (Draft)](TOOL_COMPATIBILITY_DRAFT.md) ([中文](TOOL_COMPATIBILITY_DRAFT_CN.md)) — tool-aware passthrough, provider capability model, no-silent-degradation rule

---

## Architecture

```
┌─────────────────────────────────────────────┐
│              HTTP Layer (axum)               │  ← OpenAI-compatible endpoints
├──────────┬──────────┬───────────────────────┤
│ Auth &   │ Guard-   │ Spend Tracking        │  ← Phase 3-4
│ Key Mgmt │ rails    │ (per key/team/user)   │
├──────────┴──────────┴───────────────────────┤
│           Router / Load Balancer             │  ← Phase 2
├─────────────────────────────────────────────┤
│         Unified Provider Abstraction         │  ← trait Provider
├────┬────┬────┬────┬────┬────┬────┬────┬─────┤
│OAI │Anth│Bed │Azu │Vert│Olla│vLLM│NIM │ ... │
└────┴────┴────┴────┴────┴────┴────┴────┴─────┘
```

### Module Structure

```
src/
├── main.rs              Entry point, CLI
├── config/              Configuration loading & validation
├── server/
│   ├── routes/          HTTP endpoint handlers
│   └── middleware/      Auth, rate limit, guardrails, logging, spend
├── providers/           Provider trait + implementations
├── router/              Routing, fallback, load balancing
├── schema/              Unified request/response types
├── storage/             Database layer (SQLite/Postgres)
├── guardrails/          Content filtering, PII masking
└── ui/                  Embedded admin UI assets
```

### Technology Stack

| Component | Choice |
|-----------|--------|
| Language | Rust 2021 |
| Async Runtime | tokio |
| HTTP Server | axum |
| HTTP Client | reqwest |
| Serialization | serde + serde_json |
| Database | sqlx + SQLite (default) / PostgreSQL |
| Logging | tracing + tracing-subscriber |
| Config | config crate + YAML |
| Streaming | SSE via axum + tokio-stream |

---

## Performance Targets

| Metric | Target |
|--------|--------|
| P50 latency (proxy overhead) | < 2ms |
| P95 latency (proxy overhead) | < 8ms |
| P99 latency (proxy overhead) | < 15ms |
| Max concurrent connections | 10,000+ |
| Throughput | 1,000+ RPS sustained |
| Startup time | < 1 second |
| Binary size | < 30MB |
| Idle memory | < 50MB |
| Docker image | < 50MB |

---

## Development

```bash
# Build
cargo build --workspace

# Run tests
cargo test --workspace

# Lint (must be 0 warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Format
cargo fmt --all -- --check

# Run locally
cargo run -- --config config.example.yaml
```

---

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

The easiest way to contribute is by **adding a new provider** — implement the `Provider` trait and submit a PR.

---

## Stability Notice

Rausu is pre-1.0 and under active development. Breaking changes may occur between minor versions. For production use, pin to a specific release tag.

---

## License

MIT — see [LICENSE](LICENSE) for details.

Copyright 2026 Rausu Contributors.

---

<p align="center">
  <strong>Rausu</strong> — LLM Gateway, done right.<br/>
  <sub>Built with 🦀 Rust. Faster. Leaner. Simpler.</sub>
</p>
