# Acceptance Report — Phase 2.8A: GitHub Copilot Provider

> **Chinese version:** [ACCEPTANCE_2_8A_CN.md](ACCEPTANCE_2_8A_CN.md)

**Status: PARTIAL — implementation complete, local E2E blocked by credentials**

## Summary

The GitHub Copilot provider has been implemented and integrated into Rausu.
All unit tests pass (83 total).  Local end-to-end acceptance was partially
completed: server startup and model listing succeeded; the upstream Copilot API
call could not be exercised because the available local credentials are not
compatible with the `/copilot_internal/v2/token` endpoint (see §Blocker below).

---

## Supported endpoints

| Route | Support |
|---|---|
| `POST /v1/chat/completions` | ✅ (streaming + non-streaming) |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/messages` | ❌ Copilot does not expose Anthropic Messages API |
| `POST /v1/responses` | ❌ Copilot does not expose OpenAI Responses API |

---

## Auth mode implemented

Two-step token exchange:

1. Load a **GitHub OAuth token** (`ghu_...` device-flow token) from:
   - `GH_TOKEN` or `GITHUB_TOKEN` env var (`token_source: env`)
   - `~/.config/github-copilot/hosts.json` → `github.com.oauth_token`
     (`token_source: hosts_file` or `auto`)
2. Exchange it for a **short-lived Copilot API token** via
   `GET https://api.github.com/copilot_internal/v2/token`
   with `Authorization: token {github_oauth_token}`.
3. Cache the Copilot API token; re-exchange 5 minutes before expiry.

The Copilot API token is then used as `Authorization: Bearer {api_token}` for
requests to `https://api.githubcopilot.com/chat/completions`.

---

## Config format

```yaml
models:
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o
        token_source: auto        # auto | env | hosts_file  (default: auto)
        # credentials_path: /custom/path/to/hosts.json  (optional)
```

**`token_source` values:**

| Value | Behaviour |
|---|---|
| `auto` (default) | Try env vars first, then hosts.json |
| `env` | `GH_TOKEN` or `GITHUB_TOKEN` env var only |
| `hosts_file` | `~/.config/github-copilot/hosts.json` (or `credentials_path`) |

---

## Files changed

| File | Change |
|---|---|
| `src/auth/copilot.rs` | **New** — CopilotTokenManager |
| `src/auth/mod.rs` | Added `pub mod copilot;` |
| `src/providers/github_copilot.rs` | **New** — GitHubCopilotProvider |
| `src/providers/mod.rs` | Added `pub mod github_copilot;` |
| `src/server/mod.rs` | Added `"github-copilot"` case in `build_providers()` |
| `config.example.yaml` | Added GitHub Copilot section |
| `docs/GITHUB_COPILOT_PROVIDER.md` | **New** — provider docs (EN) |
| `docs/GITHUB_COPILOT_PROVIDER_CN.md` | **New** — provider docs (CN) |
| `docs/ACCEPTANCE_2_8A.md` | **New** — this file |
| `docs/ACCEPTANCE_2_8A_CN.md` | **New** — CN version |

---

## Tests run

```
cargo test
running 83 tests
test result: ok. 83 passed; 0 failed
```

New tests added:

**`auth::copilot::tests`** (13 tests):
- `test_token_not_expired`
- `test_token_expired`
- `test_token_within_refresh_margin`
- `test_load_from_env_gh_token`
- `test_load_from_env_github_token_fallback`
- `test_load_from_env_missing`
- `test_load_from_hosts_file`
- `test_load_from_hosts_file_missing`
- `test_load_from_hosts_file_missing_github_key`
- `test_auto_prefers_env_over_hosts_file`
- `test_default_endpoint`
- `test_hosts_file_path_default`

**`providers::github_copilot::tests`** (3 tests):
- `test_provider_name`
- `test_models_list`
- `test_empty_model_list`

---

## Local E2E acceptance

| Step | Result |
|---|---|
| `cargo build` | ✅ clean (0 warnings relevant to new code) |
| `cargo test` | ✅ 83/83 pass |
| Server starts with Copilot config | ✅ `Server listening, address: 127.0.0.1:14321` |
| `GET /v1/models` | ✅ returns `copilot-gpt-4o` and `copilot-claude-sonnet` models |
| `POST /v1/chat/completions` | ⚠️ auth blocked (see below) |

---

## Blocker / limitation

**Local E2E blocked by credential compatibility.**

The `/copilot_internal/v2/token` endpoint on GitHub requires a GitHub OAuth
device-flow token (`ghu_...`).  The local machine has:

- A `ghu_...` token in `~/.config/github-copilot/hosts.json` → **expired**
  (returns HTTP 401 Bad credentials).
- A fine-grained PAT (`github_pat_...`) in `GH_TOKEN` env → **incompatible**
  (returns HTTP 403 "Resource not accessible by personal access token").
  Fine-grained PATs cannot call Copilot internal endpoints.

**This is a credential availability issue, not an implementation bug.**

**To get a working token:**

```bash
# Option A — GitHub CLI (creates ghu_... OAuth token):
gh auth login --scopes read:user

# Option B — set a classic PAT (ghp_...) with `read:user` + Copilot access:
export GH_TOKEN=ghp_yourClassicPAT
```

---

## Known limitations

1. The `base_url` config field is ignored; endpoint is from the token exchange
   response (defaults to `https://api.githubcopilot.com`).
2. Tool/function calling support depends on the upstream Copilot model.
3. Copilot rate limits and model availability are controlled by GitHub; Rausu
   propagates upstream HTTP status codes unchanged.
4. No passthrough for `/v1/messages` or `/v1/responses` — Copilot does not
   implement those APIs.
