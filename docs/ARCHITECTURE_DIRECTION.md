# Architecture Direction — Local-First, Gateway-Compatible

> [中文版](ARCHITECTURE_DIRECTION_CN.md)

## Decision

Rausu will prioritize **local proxy productization** first. The immediate goal is a solid, single-user localhost proxy that works seamlessly with tools like Codex CLI and Claude Code, using existing ChatGPT / Claude subscriptions.

Gateway capabilities (multi-user, remote bind, authn/authz, admin APIs) are a **future-compatible expansion direction**, not current MVP scope. The architecture must keep this path open without forcing premature complexity into the local runtime.

## 3-Layer Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  Gateway Runtime (future)                │
│  daemon / remote bind, authn/authz, multi-user,         │
│  rate limits, quota, policy, admin APIs                  │
├─────────────────────────────────────────────────────────┤
│                  Local Runtime (current focus)           │
│  localhost HTTP proxy, single-user config,               │
│  Codex-compatible endpoints, local auth injection,       │
│  fake-key compatibility                                  │
├─────────────────────────────────────────────────────────┤
│                  Core Layer (shared)                     │
│  provider abstraction, auth/token manager,               │
│  request normalization, routing primitives,               │
│  upstream transport, streaming relay,                    │
│  model registry, error mapping, usage accounting         │
└─────────────────────────────────────────────────────────┘
```

### Layer 1: Core

The core layer is runtime-agnostic. It contains all reusable logic that both local and gateway runtimes depend on:

- **Provider abstraction** — unified trait system for OpenAI, Anthropic, Claude subscription, ChatGPT subscription, etc.
- **Auth / token manager** — OAuth token loading, refresh, credential resolution (but NOT tied to a specific file path like `~/.codex/auth.json`)
- **Request normalization / routing primitives** — model lookup, provider selection, request transformation
- **Upstream transport** — HTTP client pool, connection management, timeout/retry primitives
- **Streaming relay** — SSE chunk-by-chunk proxying, backpressure handling
- **Model registry** — model → provider mapping, supported model lists
- **Error mapping** — upstream errors → consistent downstream error format
- **Usage accounting** — token counting, request logging (storage-agnostic)

### Layer 2: Local Runtime (current focus)

The local runtime is a thin shell around the core, optimized for single-user localhost use:

- **Localhost HTTP proxy** — `axum` server bound to `127.0.0.1` or `0.0.0.0` with minimal overhead
- **Single-user local config** — YAML config file, environment variable overrides, no database required
- **Codex-compatible endpoints** — `/v1/responses`, `/v1/chat/completions`, and other endpoints that Codex CLI / Claude Code expect
- **Local auth injection** — read OAuth tokens from local credential files, inject auth headers upstream, accept any fake API key from local clients
- **Fake-key compatibility** — clients can send `api_key: "anything"` because Rausu handles real auth upstream

**Later extensions (still local runtime):**

- **Local takeover / base_url switching** — intercept calls from tools that hardcode upstream URLs (e.g. `OPENAI_BASE_URL` override)
- **Local usage dashboard** — optional lightweight stats page

### Layer 3: Gateway Runtime (future)

The gateway runtime extends core with multi-user / remote deployment concerns:

- **Daemon / remote bind** — listen on non-localhost interfaces, TLS termination
- **Authn / authz** — virtual API keys, per-user identity, RBAC
- **Multi-user / multi-account** — multiple upstream credential sets, per-user routing
- **Rate limits / quota** — per-key, per-user, per-model rate limiting
- **Policy** — content filtering, PII masking, guardrails
- **Admin APIs** — user management, key provisioning, usage reports

## Near-Term Endpoint Priorities

For the local proxy MVP, these endpoints take priority:

| Endpoint | Purpose | Notes |
|----------|---------|-------|
| `POST /v1/responses` | Codex CLI primary endpoint | Passthrough to ChatGPT Responses API |
| `POST /v1/responses/compact` | Codex CLI compact variant | Passthrough |
| `POST /v1/chat/completions` | Universal OpenAI-compatible | Existing; works with all providers |

These are the endpoints that Codex CLI and Claude Code actually call. Everything else is secondary until these are solid.

## Passthrough over Conversion

**Decision:** For ChatGPT subscription / Codex CLI proxying, prefer **native passthrough** over format conversion.

**Rationale:**

1. **Codex CLI speaks Responses API natively.** When a user points Codex CLI at Rausu via `OPENAI_BASE_URL`, the CLI sends Responses API requests. Converting these to Chat Completions and back would be:
   - Lossy — Responses API has fields with no Chat Completions equivalent
   - Slow — unnecessary serialization/deserialization round-trips
   - Fragile — upstream API changes break two conversion layers instead of zero

2. **Claude Code speaks Messages API natively.** Same principle — `/v1/messages` should pass through to Anthropic's API as-is.

3. **Conversion is only needed at boundaries.** When a client speaks OpenAI Chat Completions but the upstream is Anthropic, conversion is unavoidable and valuable. When client and upstream speak the same protocol, conversion is pure overhead.

**Rule of thumb:** If the client protocol matches the upstream protocol, passthrough. If they differ, convert at the boundary closest to the upstream.

## Non-Goals / Deferred Scope

The following are explicitly **not** in scope for the local proxy MVP:

- **Multi-tenant SaaS** — no user isolation, no billing, no tenant management
- **Heavy web UI** — no admin dashboard, no model playground (a lightweight local stats page is fine later)
- **Distributed control plane** — no service mesh, no multi-node coordination
- **Advanced quota / billing systems** — no credit tracking, no usage-based billing, no invoice generation
- **Plugin / WASM extension system** — premature until core is stable
- **Database requirement** — local runtime must work without SQLite/Postgres; file-based config only

These are all valid gateway-era features. They are deferred, not rejected.

## Phased Roadmap

| Phase | Scope | Description |
|-------|-------|-------------|
| **Phase 2.5** | Responses passthrough for Codex CLI | Add `/v1/responses` endpoint that passes through to ChatGPT Responses API via `chatgpt-subscription` provider. This is the critical path for Codex CLI support. |
| **Phase 2.6** | Local fake-key / takeover support | Accept arbitrary API keys from local clients. Support `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` override patterns so tools can be pointed at Rausu without code changes. |
| **Phase 2.7** | Reliability hardening | Timeouts, retries with backoff, structured request/response logging, usage tracking (local file), health check improvements, graceful shutdown. |
| **Future** | Gateway runtime expansion | Remote bind, authn/authz, multi-user, rate limits, admin APIs. Only started after local proxy is production-solid. |

## Implementation Guidance: Avoiding Core ↔ Runtime Coupling

The most important architectural discipline is keeping the core layer free of runtime assumptions.

### Do: Abstract credential sources

```rust
// Good: core defines a trait
trait TokenSource: Send + Sync {
    async fn get_token(&self) -> Result<String>;
}

// Local runtime provides file-based impl
struct FileTokenSource { path: PathBuf }

// Gateway runtime could provide DB-backed impl
struct DbTokenSource { pool: PgPool, user_id: Uuid }
```

### Don't: Hardcode file paths in core

```rust
// Bad: core knows about ~/.codex/auth.json
fn load_token() -> String {
    std::fs::read_to_string(home_dir().join(".codex/auth.json"))
}
```

### Concrete rules

1. **Core must not reference specific file paths** — no `~/.claude/`, no `~/.codex/`, no `~/.config/rausu/`. These belong in the local runtime's configuration layer.
2. **Core must not assume single-user** — token sources, config loaders, and request handlers should accept injected dependencies, not reach for global state.
3. **Core must not assume localhost** — no hardcoded `127.0.0.1`, no assumptions about TLS. The runtime decides bind address and transport.
4. **Core must not require a database** — usage accounting should accept a trait (could be in-memory, file, SQLite, or Postgres depending on runtime).
5. **Config parsing belongs in runtime** — core accepts typed config structs, not raw YAML or file paths.

This discipline has a near-term payoff too: it makes the local runtime easier to test, because you can inject mock token sources and in-memory stores instead of touching the filesystem.
