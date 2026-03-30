# Anthropic Messages API Proxy (`/v1/messages`) — Design Document

## Overview

Add a native `/v1/messages` endpoint to Rausu so that tools expecting the
Anthropic Messages API (e.g. **Claude Code CLI**) can use Rausu as a transparent
proxy.

### Use Case

```
Claude Code CLI
  → ANTHROPIC_BASE_URL=http://localhost:4000
  → ANTHROPIC_API_KEY=fake-key-ignored
  → POST /v1/messages  (Anthropic native format)
  → Rausu receives request
  → Routes to claude-subscription provider (OAuth token)
  → Transparent proxy to https://api.anthropic.com/v1/messages
  → Streams response back to Claude Code CLI
```

The user does not need a paid Anthropic API key — Rausu uses their existing
Claude Pro/Max subscription OAuth token.

## Design Decisions

### 1. Native pass-through, NOT format conversion

The existing `/v1/chat/completions` endpoint converts OpenAI format → Anthropic
format → OpenAI format. For `/v1/messages` we do **zero format conversion**:

- Request comes in as Anthropic Messages format → forwarded as-is
- Response comes back as Anthropic Messages format → returned as-is
- SSE streaming events are proxied byte-for-byte

This is simpler, faster, and preserves full API compatibility (tool_use,
thinking blocks, images, etc.).

### 2. Provider routing

The `/v1/messages` endpoint only routes to providers that support the Anthropic
Messages API:

- `anthropic` (API key auth)
- `claude-subscription` (OAuth token auth)

Routing logic: extract `model` from the JSON request body → look up in
`model_registry` → find a matching provider of type `anthropic` or
`claude-subscription`.

### 3. Auth injection

Rausu replaces/injects authentication headers regardless of what the client
sends:

| Provider | Auth header |
|----------|-------------|
| `anthropic` | `x-api-key: <configured_key>` |
| `claude-subscription` | `Authorization: Bearer <oauth_token>` + beta headers + user-agent + x-app |

The client's `x-api-key` or `Authorization` header is **ignored** (can be a
fake key).

### 4. Claude Code identity headers (claude-subscription only)

For `claude-subscription`, the proxy injects all required Claude Code identity
headers (already implemented in the existing provider):

- `anthropic-beta: claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14`
- `user-agent: claude-cli/2.1.75`
- `x-app: cli`
- `anthropic-version: 2023-06-01`

The system prompt Claude Code identity prefix is **NOT injected** at the proxy
level — Claude Code CLI already sends its own system prompt.

## Implementation Plan

### New files

| File | Purpose |
|------|---------|
| `src/server/routes/messages.rs` | `/v1/messages` route handler |
| `src/server/routes/messages.rs` | Streaming + non-streaming proxy logic |

### Modified files

| File | Change |
|------|--------|
| `src/server/routes/mod.rs` | Add `pub mod messages;` |
| `src/server/mod.rs` | Register `/v1/messages` route |
| `src/providers/mod.rs` | Add `MessagesProvider` trait |
| `src/providers/anthropic.rs` | Implement `MessagesProvider` |
| `src/providers/claude_subscription.rs` | Implement `MessagesProvider` |

### New trait: `MessagesProvider`

```rust
#[async_trait]
pub trait MessagesProvider: Send + Sync {
    /// Forward a raw Anthropic Messages API request.
    /// Returns the raw response (status + headers + body stream).
    async fn proxy_messages(
        &self,
        body: serde_json::Value,
        is_stream: bool,
    ) -> Result<reqwest::Response, ProviderError>;
}
```

Only `anthropic` and `claude-subscription` providers implement this trait.
The route handler checks if the resolved provider implements `MessagesProvider`
via downcast or a separate registry.

### Route handler pseudo-code

```rust
async fn messages(State(state): State<AppState>, body: Json<Value>) -> Response {
    let model = body["model"].as_str()?;
    let is_stream = body["stream"].as_bool().unwrap_or(false);

    // Find provider for model (must be anthropic or claude-subscription)
    let provider = find_messages_provider(&state, model)?;

    // Forward request
    let upstream_resp = provider.proxy_messages(body.0, is_stream).await?;

    if is_stream {
        // Stream SSE bytes directly back to client
        proxy_sse_stream(upstream_resp)
    } else {
        // Return JSON response as-is
        proxy_json_response(upstream_resp)
    }
}
```

### Streaming strategy

For SSE streaming, the response is **byte-proxied** — no parsing of individual
SSE events. This ensures:

- Zero overhead
- Full compatibility with all event types (content_block_delta, thinking, tool_use, etc.)
- No risk of breaking on new event types

```rust
async fn proxy_sse_stream(upstream: reqwest::Response) -> Response {
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let body = Body::from_stream(upstream.bytes_stream());
    (status, headers, body).into_response()
}
```

### Config — no changes needed

The existing `config.yaml` model definitions already specify `provider: "claude-subscription"`.
The `/v1/messages` route reuses the same `model_registry` and provider instances.

Example config (already works):

```yaml
models:
  - name: claude-sonnet-4-6
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-6

  - name: claude-opus-4-6
    providers:
      - provider: claude-subscription
        model: claude-opus-4-6
```

### Client usage

```bash
# Point Claude Code at Rausu
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="rausu-proxy"   # any non-empty string

# Claude Code now uses your subscription through Rausu
claude
```

## Error Handling

- Model not found → 404 with Anthropic-format error
- Provider is not a Messages-capable provider → 400
- Upstream error → proxy status code + body as-is (preserves Anthropic error format)
- OAuth token refresh failure → 500 with descriptive message

## Testing

1. Unit tests for route handler (mock provider)
2. Integration test: send Anthropic-format request to `/v1/messages`, verify response format
3. Manual test: `ANTHROPIC_BASE_URL=http://localhost:4000 claude` with fake key

## Non-goals (this phase)

- No format conversion between OpenAI ↔ Anthropic at the `/v1/messages` level
- No load balancing or failover (single provider per model)
- No request modification (system prompt injection happens in claude-subscription provider only for `/v1/chat/completions`)
- No rate limiting or quota tracking
