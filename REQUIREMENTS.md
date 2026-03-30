# Requirements Document — Rust LLM Gateway

> [中文版](./REQUIREMENTS_CN.md)

## 1. Project Overview

### 1.1 Vision

A high-performance, low-resource LLM API Gateway written in Rust — a drop-in replacement for LiteLLM Proxy with significantly better performance, smaller footprint, and simpler deployment.

### 1.2 Goals

- **Unified Interface**: OpenAI-compatible API that proxies requests to 100+ LLM providers
- **Single Binary**: One executable for local install or Docker — zero runtime dependencies
- **High Performance**: P95 latency < 8ms at 1,000 RPS (proxy overhead only)
- **Low Resource**: Idle memory < 50MB, minimal CPU usage
- **Extensible**: Plugin/module system for adding new providers without forking

### 1.3 Non-Goals (v1)

- Not a model serving engine (no inference)
- Not a multi-tenant SaaS platform (single-org focus first)
- Not a full observability platform (integrates with existing ones)

### 1.4 Current Execution Strategy

> Architecture details: see [docs/ARCHITECTURE_DIRECTION.md](docs/ARCHITECTURE_DIRECTION.md)

Rausu is executing in a **local-first, gateway-compatible** order.

**Current focus — Local Proxy Productization:**
- Single-user localhost proxy for AI coding tools (Codex CLI, Claude Code)
- Subscription-backed providers: `claude-subscription` (Claude web auth) and `chatgpt-subscription` (ChatGPT web auth)
- No upstream API key required for local use; Rausu handles real auth
- No database, no virtual keys, no admin UI — file-based config only
- Priority endpoints: `/v1/responses`, `/v1/responses/compact`, `/v1/chat/completions`, `/v1/messages`

**Later phases — Gateway Expansion:**
- Multi-user / remote deployment concerns (authn/authz, virtual keys, rate limits)
- Admin UI, spend tracking, guardrails
- Broad provider coverage (Bedrock, Azure, Vertex AI, Ollama, 100+ providers)

The architecture is designed to keep the gateway path open without forcing premature complexity into the local runtime. Both runtimes share a common core layer (provider abstraction, routing, streaming relay, error mapping). See Section 5 for the phased delivery plan.

---

## 2. Functional Requirements

### 2.1 API Endpoints (OpenAI-Compatible)

All endpoints follow the OpenAI API specification and return unified response schemas regardless of the upstream provider.

| Endpoint | Description | Priority |
|---|---|---|
| `POST /v1/chat/completions` | Chat completions (streaming & non-streaming) | P0 |
| `POST /v1/responses` | OpenAI Responses API (Codex CLI primary endpoint) | P0 |
| `POST /v1/responses/compact` | Responses API compact variant (Codex CLI) | P0 |
| `POST /v1/messages` | Anthropic Messages API (native passthrough, Claude Code) | P0 |
| `GET /v1/models` | List available models | P0 |
| `GET /health` | Health check | P0 |
| `POST /v1/embeddings` | Text embeddings | P1 |
| `POST /v1/images/generations` | Image generation | P1 |
| `POST /v1/audio/transcriptions` | Audio transcription (Whisper-compatible) | P1 |
| `POST /v1/audio/speech` | Text-to-speech | P1 |
| `POST /v1/rerank` | Reranking | P1 |
| `POST /v1/moderations` | Content moderation | P2 |
| `POST /v1/batches` | Batch processing | P2 |

### 2.2 Provider Abstraction

#### 2.2.1 Core Provider Trait

Each provider implements a unified trait that covers all supported endpoint types. Providers only need to implement the endpoints they support; unsupported endpoints return a standardized `405 Unsupported` error.

#### 2.2.2 Provider List (by Priority)

**Phase 1 (MVP — local proxy):**
- `claude-subscription` (Claude web subscription, local auth injection)
- `chatgpt-subscription` (ChatGPT web subscription, local auth injection)
- OpenAI (API key)
- Anthropic (API key)

**Phase 3 (API gateway expansion):**
- AWS Bedrock
- Azure OpenAI
- Google Vertex AI
- Ollama

**Phase 4+:**
- vLLM
- NVIDIA NIM
- Groq
- Mistral
- Cohere
- DeepSeek

**Phase 6+:**
- Remaining providers via community contributions and/or plugin system
- Target: 100+ providers

#### 2.2.3 Unified Schema

- **Request**: All incoming requests follow OpenAI format; the gateway translates to each provider's native format internally
- **Response**: All responses are normalized to OpenAI format before returning to the client
- **Errors**: Provider-specific errors are mapped to OpenAI error codes (`401`, `429`, `500`, etc.) with original error details preserved in metadata
- **Streaming**: SSE-based streaming with unified chunk format, regardless of upstream protocol

### 2.3 Routing & Load Balancing

| Feature | Description | Priority |
|---|---|---|
| **Model Routing** | Map virtual model names to one or more provider deployments | P0 |
| **Fallback** | Automatic failover to next provider on error (configurable per error type) | P0 |
| **Retry** | Configurable retry with exponential backoff | P0 |
| **Weighted Routing** | Distribute traffic by weight across deployments | P1 |
| **Latency-Based** | Route to lowest-latency provider | P2 |
| **Cost-Based** | Route to cheapest provider for equivalent models | P2 |

### 2.4 Authentication & Key Management

> **Scope note:** Virtual keys, team/user binding, budget limits, and rate limiting are **gateway-era features** (see §1.4). The local proxy MVP uses file-based config; the local HTTP server accepts any API key from clients (fake-key compatibility) while Rausu handles real upstream auth.

| Feature | Description | Priority |
|---|---|---|
| **Virtual Keys** | Issue proxy API keys that map to upstream provider credentials | P0 |
| **Key CRUD** | Create / list / revoke / rotate virtual keys via API | P0 |
| **Team/User Binding** | Associate keys with teams and users | P1 |
| **Budget Limits** | Set max spend per key / team / user (hard & soft limits) | P1 |
| **Rate Limiting** | Requests per minute / tokens per minute per key | P1 |
| **Key Scoping** | Restrict keys to specific models or endpoints | P2 |

### 2.5 Spend Tracking

> **Scope note:** Full spend tracking with a database and spend API is a **gateway-era feature** (Phase 4+). The local proxy MVP logs usage locally without a database requirement.

| Feature | Description | Priority |
|---|---|---|
| **Per-Request Cost** | Calculate cost per request using provider pricing tables | P0 |
| **Cost Aggregation** | Aggregate by key / team / user / model / provider | P1 |
| **Pricing Config** | User-configurable pricing overrides | P1 |
| **Spend API** | Query spend data via REST API | P1 |
| **Budget Alerts** | Webhook/log alerts when approaching budget limits | P2 |
| **Export** | Export spend data as CSV/JSON | P2 |

### 2.6 Guardrails

| Feature | Description | Priority |
|---|---|---|
| **Content Filtering** | Block requests/responses matching configurable patterns | P1 |
| **PII Masking** | Detect and mask/redact PII in requests before forwarding | P1 |
| **Prompt Injection Detection** | Basic heuristic detection of prompt injection attempts | P2 |
| **Custom Rules** | User-defined guardrail rules via config | P1 |
| **Guardrail Pipeline** | Ordered middleware chain: pre-request → request → response → post-response | P1 |

### 2.7 Logging & Observability

| Feature | Description | Priority |
|---|---|---|
| **Structured Logging** | JSON-formatted logs with request ID, model, provider, latency, tokens, cost | P0 |
| **Request/Response Logging** | Optional full request/response body logging (configurable) | P1 |
| **Metrics Endpoint** | Prometheus-compatible `/metrics` endpoint | P1 |
| **OpenTelemetry** | OTLP trace export | P2 |
| **Callback Integrations** | Langfuse / Helicone / custom webhook | P2 |

### 2.8 Admin UI

> **Scope note:** The Admin UI is a **gateway-era feature** (Phase 5). The local proxy MVP has no web dashboard. A lightweight local stats page may be added later as a local-runtime convenience feature.

| Feature | Description | Priority |
|---|---|---|
| **Dashboard** | Overview: request volume, latency, error rate, spend | P1 |
| **Key Management** | Create / revoke / inspect virtual keys | P1 |
| **Spend Explorer** | Drill down spend by key / team / user / model | P2 |
| **Model Config** | View/edit model routing and provider config | P2 |
| **Log Viewer** | Search and filter request logs | P2 |
| **Guardrail Config** | Manage guardrail rules | P2 |

The Admin UI is a static SPA embedded into the binary (served at `/ui`). No separate deployment needed.

---

## 3. Non-Functional Requirements

### 3.1 Performance

| Metric | Target |
|---|---|
| P50 latency (proxy overhead) | < 2ms |
| P95 latency (proxy overhead) | < 8ms |
| P99 latency (proxy overhead) | < 15ms |
| Max concurrent connections | 10,000+ |
| Throughput | 1,000+ RPS sustained |
| Startup time | < 1 second |

### 3.2 Resource Usage

| Metric | Target |
|---|---|
| Binary size | < 30MB |
| Idle memory | < 50MB |
| Memory at 1k RPS | < 200MB |
| CPU (idle) | ~0% |

### 3.3 Deployment

- **Single binary**: `./gateway` or `./gateway --config config.yaml`
- **Docker**: Official image, < 50MB compressed
- **Configuration**: YAML file + environment variable overrides
- **Graceful shutdown**: Drain in-flight requests on SIGTERM
- **Hot reload**: Config reload on SIGHUP (no restart needed)

### 3.4 Security

- TLS termination support (optional, typically handled by reverse proxy)
- API key hashing (keys stored as hashes, never plaintext)
- Audit log for all admin operations
- No secrets in logs

### 3.5 Reliability

- Zero-downtime config reload
- Circuit breaker per provider (auto-disable unhealthy providers)
- Request timeout with configurable per-provider limits
- Graceful degradation: if spend DB is unavailable, continue proxying (log warning)

---

## 4. Technical Architecture

### 4.1 Technology Stack

| Component | Choice | Rationale |
|---|---|---|
| Language | Rust (2021 edition) | Performance, safety, single binary |
| Async Runtime | `tokio` | Industry standard, mature ecosystem |
| HTTP Server | `axum` | Best ergonomics in tokio ecosystem |
| HTTP Client | `reqwest` | Mature, streaming support, TLS |
| Serialization | `serde` + `serde_json` | De facto standard |
| Database | `sqlx` + SQLite (default) / PostgreSQL (optional) | Embedded-first, zero external deps |
| Logging | `tracing` + `tracing-subscriber` | Structured, high-performance |
| Config | `config` crate + YAML | Flexible, env var overlay |
| Admin UI | Static SPA + `rust-embed` | Embedded in binary |
| Streaming | SSE via `axum` + `tokio-stream` | OpenAI streaming = SSE |

### 4.2 Module Structure

```
src/
├── main.rs                  # Entry point, CLI
├── config/                  # Configuration loading & validation
│   ├── mod.rs
│   └── schema.rs
├── server/                  # HTTP server setup, routes
│   ├── mod.rs
│   ├── routes/
│   │   ├── chat.rs          # /v1/chat/completions
│   │   ├── embeddings.rs    # /v1/embeddings
│   │   ├── images.rs        # /v1/images/generations
│   │   ├── audio.rs         # /v1/audio/*
│   │   ├── rerank.rs        # /v1/rerank
│   │   ├── batch.rs         # /v1/batches
│   │   ├── models.rs        # /v1/models
│   │   └── admin.rs         # Admin API
│   └── middleware/
│       ├── auth.rs          # API key validation
│       ├── rate_limit.rs    # Rate limiting
│       ├── guardrails.rs    # Guardrail pipeline
│       ├── logging.rs       # Request/response logging
│       └── spend.rs         # Cost tracking middleware
├── providers/               # Provider implementations
│   ├── mod.rs               # Provider trait definition
│   ├── openai.rs
│   ├── anthropic.rs
│   ├── bedrock.rs
│   ├── azure.rs
│   ├── vertex.rs
│   ├── ollama.rs
│   └── ...
├── router/                  # Routing & load balancing
│   ├── mod.rs
│   ├── fallback.rs
│   ├── weighted.rs
│   └── latency.rs
├── schema/                  # Unified request/response types
│   ├── mod.rs
│   ├── chat.rs
│   ├── embedding.rs
│   ├── image.rs
│   ├── audio.rs
│   └── error.rs
├── storage/                 # Database layer
│   ├── mod.rs
│   ├── keys.rs              # Virtual key storage
│   ├── spend.rs             # Spend records
│   └── audit.rs             # Audit log
├── guardrails/              # Guardrail implementations
│   ├── mod.rs
│   ├── pii.rs
│   ├── content_filter.rs
│   └── custom.rs
└── ui/                      # Embedded admin UI assets
    └── mod.rs
```

### 4.3 Configuration Example

```yaml
# config.yaml
server:
  host: 0.0.0.0
  port: 4000
  workers: auto          # defaults to CPU count

database:
  driver: sqlite         # sqlite | postgres
  url: "sqlite://data/gateway.db"

logging:
  level: info
  format: json           # json | pretty
  log_requests: true     # log full request/response bodies
  log_responses: false

auth:
  master_key: "sk-master-xxx"   # admin key (env: GATEWAY_MASTER_KEY)

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

guardrails:
  - name: block-pii
    type: pii_masking
    enabled: true
    config:
      entities: [email, phone, ssn, credit_card]
      action: mask    # mask | block | log

  - name: content-filter
    type: content_filter
    enabled: true
    config:
      blocked_patterns: ["ignore previous instructions"]
      action: block

spend:
  enabled: true
  alert_threshold_usd: 100.0
  pricing_overrides:
    gpt-4o:
      input_per_1k: 0.0025
      output_per_1k: 0.01
```

---

## 5. Delivery Phases

> For the architectural rationale behind this sequencing, see [docs/ARCHITECTURE_DIRECTION.md](docs/ARCHITECTURE_DIRECTION.md).

The delivery order is **local-first**: get the single-user localhost proxy solid before expanding into gateway/multi-user territory.

### Phase 1 — Local Proxy MVP
**Goal**: A working localhost proxy for Codex CLI and Claude Code using existing subscriptions.

- [x] `axum` HTTP server with `/v1/chat/completions`, `/v1/responses`, `/v1/responses/compact`, `/v1/messages`
- [x] `claude-subscription` provider (Claude web auth, `/v1/messages` native passthrough)
- [x] `chatgpt-subscription` provider (ChatGPT web auth, `/v1/responses` native passthrough)
- [x] Provider trait + OpenAI provider + Anthropic provider (API key)
- [x] SSE streaming passthrough
- [x] YAML configuration + environment variable interpolation
- [x] `tracing` structured logging (JSON)
- [x] Single binary build + Dockerfile
- [x] Basic error mapping (provider errors → OpenAI error codes)
- [x] Health endpoint (`/health`)
- [x] README (EN + CN)

**Exit Criteria**: Codex CLI and Claude Code can be pointed at Rausu and use existing subscriptions without providing real API keys.

### Phase 2 — Local Proxy Hardening
**Goal**: Reliable, friction-free single-user local proxy experience.

- [ ] Fake-key compatibility — accept any API key from local clients (Rausu handles real upstream auth)
- [ ] `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` override support for transparent tool takeover
- [ ] Timeouts, retries with exponential backoff
- [ ] Structured per-request logging (local file, no database required)
- [ ] Graceful shutdown improvements
- [ ] `/v1/models` list reflecting configured providers

**Exit Criteria**: Any OpenAI SDK or Anthropic SDK client pointed at Rausu works reliably with no configuration friction.

### Phase 3 — API Gateway Expansion
**Goal**: Production-grade multi-provider routing for team / self-hosted use.

- [ ] AWS Bedrock / Azure OpenAI / Google Vertex AI / Ollama providers
- [ ] Router: retry with exponential backoff
- [ ] Router: fallback chain
- [ ] Router: weighted load balancing
- [ ] `/v1/embeddings` endpoint
- [ ] `/v1/images/generations` endpoint
- [ ] Basic API key authentication (master key)
- [ ] Circuit breaker per provider
- [ ] Remote bind (non-localhost) + optional TLS termination

**Exit Criteria**: Can route traffic across multiple providers with automatic failover in a self-hosted deployment.

### Phase 4 — Spend Tracking + Key Management
**Goal**: Multi-key access control with cost visibility.

- [ ] SQLite storage layer (sqlx)
- [ ] Virtual Key CRUD API
- [ ] Per-request cost calculation
- [ ] Spend aggregation by key / team / user
- [ ] Budget limits (hard + soft)
- [ ] Rate limiting (RPM / TPM per key)
- [ ] `/v1/audio/transcriptions` + `/v1/audio/speech`
- [ ] `/v1/rerank`
- [ ] `/v1/batches`
- [ ] Spend query API

**Exit Criteria**: Can issue virtual keys with budget limits and query spend data.

### Phase 5 — Guardrails + Admin UI
**Goal**: Content safety and visual management for gateway deployments.

- [ ] Guardrail middleware pipeline
- [ ] PII detection & masking
- [ ] Content filtering (pattern-based)
- [ ] Custom guardrail rules via config
- [ ] Admin UI SPA (embedded in binary)
  - Dashboard (volume, latency, errors, spend)
  - Key management
  - Spend explorer
  - Log viewer
- [ ] Prometheus `/metrics` endpoint
- [ ] Audit log

**Exit Criteria**: Non-technical admins can manage the gateway through the UI.

### Phase 6 — Ecosystem & Extensions
**Goal**: Community growth and advanced features.

- [ ] Plugin/WASM extension mechanism for custom providers
- [ ] vLLM / NIM / Groq / Mistral / Cohere / DeepSeek providers
- [ ] OpenTelemetry trace export
- [ ] Callback integrations (Langfuse, Helicone, webhooks)
- [ ] MCP Gateway integration
- [ ] A2A protocol support
- [ ] PostgreSQL storage backend option
- [ ] Hot config reload (SIGHUP)
- [ ] Cost-based / latency-based smart routing

**Exit Criteria**: Extensible platform with 20+ providers and a growing community.

---

## 6. Success Metrics

| Metric | Target |
|---|---|
| Proxy overhead P95 | < 8ms |
| Binary size | < 30MB |
| Idle memory | < 50MB |
| Docker image size | < 50MB |
| Startup time | < 1s |
| Provider coverage (Phase 3) | 6+ providers |
| Provider coverage (Phase 6) | 20+ providers |

---

## 7. Risks & Mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| Provider API format drift | Medium | Pin provider SDK versions; monitor changelogs; abstract behind trait |
| Scope creep (too many providers too early) | Medium | Strict phase gates; community PRs for long-tail providers |
| Admin UI complexity | Low | Keep UI minimal; use proven SPA framework; embed as static assets |
| SQLite contention at high write volume | Low | WAL mode; batch writes; optional Postgres for heavy workloads |
| Streaming edge cases per provider | Medium | Comprehensive integration tests; fuzz testing on SSE parsing |
