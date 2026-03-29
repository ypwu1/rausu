# ChatGPT Subscription Provider — Design Document

## Overview

Add a `chatgpt-subscription` provider that allows Rausu users to route requests through their ChatGPT Plus/Pro/Max subscription, using OAuth-based authentication against the ChatGPT backend API.

Rausu exposes a standard **OpenAI-compatible `/v1/chat/completions`** endpoint. Internally, the provider **bridges** Chat Completions requests to the ChatGPT **Responses API** (`chatgpt.com/backend-api/codex/responses`), then maps the response back to Chat Completions format. Downstream clients see standard OpenAI-compatible responses — the bridging is transparent.

## References

| Source | Location | Notes |
|--------|----------|-------|
| pi-ai (OpenClaw dep) | `@mariozechner/pi-ai/dist/providers/openai-codex-responses.js` | Full streaming impl, request building, header construction |
| pi-ai OAuth module | `@mariozechner/pi-ai/dist/utils/oauth/openai-codex.js` | OAuth PKCE flow, token exchange, refresh, JWT decode |
| LiteLLM docs | https://docs.litellm.ai/docs/providers/chatgpt | Chat Completions bridging, field stripping, device code flow |
| Codex CLI binary | `strings` extraction (2026-03-29) | OAuth client_id, auth endpoints, token structure |

## Architecture

```
Client (OpenAI-compatible)
  │
  │  POST /v1/chat/completions
  │  { model: "gpt-5.4", messages: [...] }
  │
  ▼
┌──────────────────────────────┐
│  Rausu Gateway               │
│  ┌────────────────────────┐  │
│  │ chatgpt-subscription   │  │
│  │ provider               │  │
│  │                        │  │
│  │ 1. Strip unsupported   │  │
│  │    fields (max_tokens, │  │
│  │    metadata)           │  │
│  │ 2. Convert Chat        │  │
│  │    Completions →       │  │
│  │    Responses format    │  │
│  │ 3. Forward to ChatGPT  │  │
│  │    backend API         │  │
│  │ 4. Stream SSE back     │  │
│  │ 5. Convert Responses → │  │
│  │    Chat Completions    │  │
│  └────────────────────────┘  │
└──────────────────────────────┘
  │
  │  POST chatgpt.com/backend-api/codex/responses
  │  { model, input, instructions, stream: true, ... }
  │
  ▼
ChatGPT Backend API
```

## API Mapping

### Request: Chat Completions → Responses

| Chat Completions field | Responses field | Notes |
|----------------------|-----------------|-------|
| `model` | `model` | Pass through |
| `messages[role=system]` | `instructions` | System prompt becomes instructions |
| `messages[role=user/assistant]` | `input` | Converted to Responses input format |
| `stream` | `stream: true` | Always stream internally; aggregate if `stream: false` |
| `temperature` | `temperature` | Pass through |
| `tools` | `tools` | Convert function format → Responses tool format |
| `tool_choice` | `tool_choice` | Pass through |
| ~~`max_tokens`~~ | — | **Strip** (rejected by backend) |
| ~~`max_output_tokens`~~ | — | **Strip** |
| ~~`max_completion_tokens`~~ | — | **Strip** |
| ~~`metadata`~~ | — | **Strip** |

### Response: Responses → Chat Completions

| Responses event | Chat Completions mapping |
|----------------|------------------------|
| `response.output_text.delta` | `choices[0].delta.content` |
| `response.function_call_arguments.delta` | `choices[0].delta.tool_calls[].function.arguments` |
| `response.completed` | `choices[0].finish_reason = "stop"` |
| `response.output_text.done` | Content block complete |
| Usage in `response.completed` | `usage` object |

## Authentication

### OAuth Flow (Authorization Code + PKCE)

Based on pi-ai implementation:

```
1. Generate PKCE challenge (code_verifier + code_challenge)
2. Open browser → auth.openai.com/oauth/authorize
   - client_id: app_EMoamEEZ73f0CkXaXp7hrann
   - redirect_uri: http://localhost:1455/auth/callback
   - scope: openid profile email offline_access
   - code_challenge_method: S256
3. Local HTTP server listens on localhost:1455
4. User authenticates in browser → callback with auth code
5. Exchange code → access_token + refresh_token
   - POST auth.openai.com/oauth/token
6. Extract chatgpt_account_id from JWT claims
```

### Token Structure

```json
{
  "access_token": "<JWT>",
  "refresh_token": "<opaque>",
  "expires_in": 3600
}
```

JWT payload contains:
```json
{
  "https://api.openai.com/auth": {
    "chatgpt_account_id": "<uuid>"
  }
}
```

### Token Refresh

```
POST auth.openai.com/oauth/token
Content-Type: application/x-www-form-urlencoded

grant_type=refresh_token
refresh_token=<token>
client_id=app_EMoamEEZ73f0CkXaXp7hrann
```

### Required Headers

```
Authorization: Bearer <access_token>
chatgpt-account-id: <extracted from JWT>
OpenAI-Beta: responses=experimental
originator: pi
User-Agent: pi (<os> <release>; <arch>)
Content-Type: application/json
```

## Configuration

```yaml
models:
  - name: gpt-5.4
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        # token_source: env | credentials_file
        # credentials_path: ~/.config/rausu/chatgpt-auth.json (optional)
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `CHATGPT_ACCESS_TOKEN` | Static access token (no refresh) |
| `CHATGPT_REFRESH_TOKEN` | Refresh token for auto-refresh |
| `CHATGPT_ACCOUNT_ID` | Override account ID (skip JWT decode) |

### Credentials File

```json
{
  "access_token": "...",
  "refresh_token": "...",
  "account_id": "...",
  "expires_at": 1743000000000
}
```

## Available Models (ChatGPT Plus/Pro/Max)

From LiteLLM docs + pi-ai source:

| Model ID | Description |
|----------|-------------|
| `gpt-5.4` | GPT-5.4 |
| `gpt-5.4-pro` | GPT-5.4 Pro |
| `gpt-5.3-codex` | GPT-5.3 Codex |
| `gpt-5.3-codex-spark` | GPT-5.3 Codex Spark |
| `gpt-5.3-instant` | GPT-5.3 Instant |
| `gpt-5.3-chat-latest` | GPT-5.3 Chat Latest |

## Files to Create

| File | Purpose |
|------|---------|
| `src/providers/chatgpt_subscription.rs` | Provider implementation + Chat↔Responses bridging |
| `src/auth/chatgpt_oauth.rs` | OAuth token manager (PKCE flow, JWT decode, refresh) |

## Files to Modify

| File | Change |
|------|--------|
| `src/providers/mod.rs` | Add `pub mod chatgpt_subscription;` |
| `src/auth/mod.rs` | Add `pub mod chatgpt_oauth;` |
| `src/config/schema.rs` | Support `provider: chatgpt-subscription` |
| `src/server/mod.rs` | Register new provider in initialization |
| `Cargo.toml` | Add `jsonwebtoken` or manual base64 JWT decode |

## Non-Goals (Phase 2)

- Native `/v1/responses` endpoint (future phase)
- WebSocket transport (SSE first)
- Interactive OAuth browser flow in Rausu itself (users provide tokens via env/file)
- Device Code Flow (simpler but less standard than PKCE)

## Key Constraints

1. **chatgpt-subscription is completely independent** of existing openai/anthropic providers
2. Always stream internally; aggregate only when client sends `stream: false`
3. Must strip `max_tokens` / `metadata` before forwarding (backend rejects them)
4. Account ID extracted from JWT, NOT from a separate API call
5. Existing providers must continue to work unchanged
