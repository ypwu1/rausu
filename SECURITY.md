# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------:|
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

If you discover a security vulnerability in Rausu, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

### How to Report

1. Email: **security@rausu.dev** (or open a [private security advisory](https://github.com/ypwu1/rausu/security/advisories/new))
2. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Affected versions
   - Potential impact assessment
   - Suggested fix (if any)

### What to Expect

- **Acknowledgment** within 48 hours
- **Initial assessment** within 7 days
- **Fix timeline** communicated within 14 days
- **Credit** given in the advisory (unless you prefer anonymity)

### Scope

The following are in scope for security reports:

- Authentication/authorization bypass (virtual key system)
- Remote code execution
- Path traversal / directory traversal
- Server-Side Request Forgery (SSRF) via provider URLs
- API key leakage (logs, responses, error messages)
- Information disclosure (internal state, configuration secrets)
- Denial of service via resource exhaustion
- Proxy request smuggling / header injection
- Credential exposure in transit or at rest
- Provider credential escalation (accessing providers beyond key scope)

## Security Architecture

Rausu implements defense-in-depth at every layer of the proxy pipeline.

### API Key Security
- **Hashed storage**: Virtual keys are stored as SHA-256 hashes — never plaintext
- **Secret redaction**: API keys are never logged, even at `trace` level
- **Memory safety**: Rust's ownership model prevents use-after-free and buffer overflows
- **No `unsafe` blocks**: Core proxy code uses zero `unsafe` Rust

### Input Validation
- **Request validation**: All incoming requests are validated against the OpenAI schema before forwarding
- **Header sanitization**: Upstream provider credentials are injected server-side; client-supplied auth headers are not forwarded
- **Size limits**: Request body size limits prevent memory exhaustion attacks
- **JSON parsing**: Strict deserialization — malformed payloads are rejected, not coerced

### Network Security
- **TLS support**: Optional TLS termination for direct deployment without reverse proxy
- **CORS policy**: Configurable CORS with restrictive defaults
- **Security headers**: `X-Content-Type-Options`, `X-Frame-Options`, `X-Request-Id` on every response
- **Provider isolation**: Provider credentials are scoped — a compromised virtual key cannot access other providers' API keys
- **Timeout enforcement**: Per-provider request timeouts prevent hung connections from exhausting resources

### Audit & Observability
- **Request logging**: Every proxied request is logged with request ID, model, provider, and status (bodies optional)
- **Error context**: Provider errors are logged with full context but sensitive fields redacted
- **Structured logs**: JSON-formatted logs for integration with SIEM and log aggregation systems

### Rate Limiting (Phase 3)
- **Per-key limits**: Requests per minute (RPM) and tokens per minute (TPM)
- **Budget enforcement**: Hard and soft spend limits per key, team, and user
- **Circuit breaker**: Automatic provider disablement on repeated failures

### Guardrails (Phase 4)
- **Content filtering**: Pattern-based request/response blocking
- **PII masking**: Detect and redact personal information before forwarding to providers
- **Prompt injection detection**: Heuristic scanning for common injection patterns
- **Middleware pipeline**: Ordered guardrail chain with pre-request and post-response hooks

## Dependencies

Security-critical dependencies:

| Dependency | Purpose |
|------------|---------|
| `axum` | HTTP server with built-in extraction validation |
| `reqwest` | HTTP client with TLS (rustls/native-tls) |
| `serde` | Safe serialization/deserialization |
| `tracing` | Structured logging without format string vulnerabilities |
| `tokio` | Async runtime with bounded task spawning |
| `uuid` | Request ID generation (v4 random) |

### Dependency Policy

- All dependencies are pinned via `Cargo.lock`
- `cargo audit` is run in CI to detect known vulnerabilities
- Minimal dependency tree — fewer deps = smaller attack surface
- No C dependencies in the core proxy (pure Rust where possible)

## Hardening Recommendations

For production deployments:

1. **Run behind a reverse proxy** (nginx, Caddy) for TLS termination and additional rate limiting
2. **Use environment variables** for all secrets — never commit API keys to config files
3. **Set a strong master key** — use `openssl rand -hex 32` or equivalent
4. **Enable request logging** to an external log aggregation system
5. **Restrict network access** — Rausu should only be reachable by authorized clients
6. **Monitor spend** — configure budget alerts to detect compromised keys early
7. **Rotate keys regularly** — both virtual keys and upstream provider credentials
8. **Pin the release** — use a specific version tag, not `latest`
