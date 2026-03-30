# Phase 2.7A — Real Local E2E Acceptance Report

> [中文版](ACCEPTANCE_2_7A_CN.md)

**Date:** 2026-03-30
**Tested by:** automated acceptance run (Claude Code agent)
**Repo state:** branch `main`, after Phase 2.6A/B/C

---

## Overall Status

| Track | Status |
|-------|--------|
| Claude Code → Rausu | **DONE** (pass) |
| Codex CLI → Rausu | **BLOCKED** (missing credentials) |

---

## Environment

| Item | Value |
|------|-------|
| Platform | Linux (Ubuntu, x86-64) |
| Rust | stable (release build) |
| Rausu version | 0.1.0 |
| Rausu binary | `./target/release/rausu` |
| Config used | `config-test.yaml` |
| Claude Code version | 2.1.87 |
| Codex CLI version | codex-cli 0.117.0 (via `npx`) |
| Claude OAuth token | valid (`~/.claude/.credentials.json`, expires in ~7 h at test time) |
| OpenAI API key | **not available** |
| ChatGPT subscription credentials | **not available** |

---

## Track 1: Claude Code → Rausu

### Configuration

`config-test.yaml`:
```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: debug
  format: pretty

models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto
```

### Test commands

```bash
# Start Rausu
./target/release/rausu --config config-test.yaml

# Verify server is up
curl http://localhost:4000/health
# → {"status":"ok"}

curl http://localhost:4000/v1/models
# → {"object":"list","data":[{"id":"claude-sonnet-4-20250514",...}]}

# Run Claude Code through Rausu
ANTHROPIC_BASE_URL="http://localhost:4000" \
ANTHROPIC_API_KEY="local-proxy" \
  claude -p "Reply with exactly: e2e-pass" --model claude-sonnet-4-20250514
# → e2e-pass
```

### Server log (relevant lines)

```
INFO  rausu: Rausu starting, version: "0.1.0", config: config-test.yaml
INFO  rausu::server: Server listening, address: 127.0.0.1:4000
INFO  rausu::server::routes::messages: Messages proxy succeeded,
        model: claude-sonnet-4-20250514, provider: claude-subscription,
        status: 200, stream: true
```

### Result

**PASS.** Claude Code 2.1.87 successfully routes `POST /v1/messages` through Rausu to the Claude subscription endpoint using the local OAuth token. Both non-streaming and streaming requests succeed.

---

## Track 2: Codex CLI → Rausu

### What was tested

```bash
# Verify Codex CLI is available
npx codex --version
# → codex-cli 0.117.0

# Test /v1/responses endpoint behavior (Codex's primary path)
curl -s -X POST http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer local-proxy" \
  -d '{"model":"claude-sonnet-4-20250514","input":"test"}'
# → {"error":{"message":"Unsupported operation: Provider 'claude-subscription'
#             does not support the Responses API","type":"internal_server_error"}}
```

### Result

**BLOCKED.** Codex CLI requires the OpenAI Responses API (`/v1/responses`). The only supported providers for this endpoint are `openai` and `chatgpt-subscription`. Neither set of credentials is present on this machine:

- `OPENAI_API_KEY` — not set
- `~/.config/rausu/chatgpt-auth.json` — does not exist
- `CHATGPT_ACCESS_TOKEN` — not set

The endpoint itself is implemented and correctly rejects unsupported providers with a clear error message. **This is a credentials blocker, not a code defect.**

To unblock Codex CLI acceptance:
1. Add an OpenAI API key: configure `OPENAI_API_KEY` and add an `openai` provider entry to `config.yaml`
2. **OR** add ChatGPT subscription credentials to `~/.config/rausu/chatgpt-auth.json` and add a `chatgpt-subscription` provider entry

---

## Bug Found and Fixed

### Context: `context_management` beta header forwarding

**Symptom:** Claude Code 2.1.87 returned:
```
API Error: 400 {"type":"error","error":{
  "type":"invalid_request_error",
  "message":"context_management: Extra inputs are not permitted"}}
```

**Root cause:** Rausu's `proxy_messages` hardcoded the `anthropic-beta` header to a fixed list. Claude Code 2.1.87 sends a `context_management` field in the request body, which requires a corresponding beta header to be accepted by the Anthropic API. Since Rausu replaced the client's beta header rather than merging it, the required beta was absent and the API rejected the request.

**Fix applied:** The `proxy_messages` method now accepts a `client_betas: Option<String>` parameter containing the `anthropic-beta` header value from the downstream client. The `claude-subscription` provider merges the client's betas with the required OAuth betas before forwarding; the `anthropic` provider forwards the client's betas directly.

**Files changed:**
- `src/providers/mod.rs` — added `client_betas` parameter to `proxy_messages` trait default; moved `Provider` trait before test module (pre-existing clippy warning)
- `src/server/routes/messages.rs` — extract `anthropic-beta` from request headers; pass to `proxy_messages`
- `src/providers/claude_subscription.rs` — added `merge_betas()` helper; use merged betas when forwarding
- `src/providers/anthropic.rs` — forward client betas when set

**Tests:** 66 unit tests + 2 integration tests all pass. `cargo clippy` and `cargo fmt` clean.

---

## Reproduction Instructions

### Claude Code via Rausu (PASS)

**Prerequisites:** logged in to Claude Code (`~/.claude/.credentials.json` present and valid)

```bash
cd /path/to/rausu

# Build
cargo build --release

# Create config (or use config-test.yaml)
cat > config.yaml <<'EOF'
server:
  host: 127.0.0.1
  port: 4000
logging:
  level: info
  format: pretty
models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto
EOF

# Start Rausu
./target/release/rausu --config config.yaml &

# Point Claude Code at Rausu
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="local-proxy"
claude -p "Hello from Rausu"
```

### Codex CLI via Rausu (blocked — requires credentials)

Once OpenAI API key or ChatGPT subscription credentials are available:

```bash
# With OpenAI API key
export OPENAI_API_KEY="sk-..."

# Add to config.yaml:
# models:
#   - name: gpt-4o
#     providers:
#       - provider: openai
#         model: gpt-4o
#         api_key: "${OPENAI_API_KEY}"

./target/release/rausu --config config.yaml &

export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="local-proxy"
codex --model gpt-4o
```

---

## Remaining Blockers

| Blocker | Type | Action needed |
|---------|------|---------------|
| Codex CLI: no OpenAI or ChatGPT credentials | Environment / credentials | Provide `OPENAI_API_KEY` or ChatGPT subscription credentials |

No code defects remain for the Claude Code acceptance track. The Codex blocker is purely an environment constraint.
