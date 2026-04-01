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

- Remote code execution
- Path traversal / directory traversal
- Server-Side Request Forgery (SSRF) via provider URLs
- API key leakage (logs, responses, error messages)
- Information disclosure (internal state, configuration secrets)
- Denial of service via resource exhaustion
- Proxy request smuggling / header injection
- Credential exposure in transit or at rest

## Current Security Posture

This section describes what is **actually implemented today**. We believe honest
documentation is more useful than aspirational claims.

### Memory Safety
- **No `unsafe` blocks**: Core proxy code uses zero `unsafe` Rust
- **Rust ownership model**: Prevents use-after-free, buffer overflows, and data races at compile time

### Supply Chain & Static Analysis (CI)
- **`cargo audit`**: Run in CI to detect dependencies with known CVEs
- **Dependencies pinned**: `Cargo.lock` ensures reproducible builds
- **Secrets scanning**: TruffleHog scans each commit for accidentally committed credentials
- **Container scanning**: Trivy scans the Docker image for OS and library vulnerabilities
- **Static analysis**: Semgrep performs SAST on each push

### Input Handling
- **Serde deserialization**: Incoming JSON is strictly deserialized — malformed payloads are rejected with a 422 error, not silently coerced
- **Header isolation**: Upstream provider credentials are injected server-side; client-supplied `Authorization` headers are replaced, not forwarded
- **Request ID**: Every request is assigned a UUID v4 for tracing

### Network Defaults
- **Local-first**: Rausu binds to `127.0.0.1` by default, not `0.0.0.0`
- **Per-provider timeouts**: Configurable timeouts prevent hung connections from exhausting resources

### Logging
- **Request logging**: Every proxied request is logged with request ID, model, provider, and status code
- **Log format**: Default output is human-readable (pretty); JSON format is not currently the default

## Known Limitations

The following security controls are **not yet implemented**. We document them here
so operators can apply compensating controls.

| Limitation | Impact | Recommended Mitigation |
|---|---|---|
| No request body size limits | A large payload could cause memory pressure | Enforce limits at the reverse proxy (e.g., `client_max_body_size` in nginx) |
| No TLS termination | Traffic is plaintext if not wrapped externally | Always deploy behind a TLS-terminating reverse proxy (nginx, Caddy) |
| CORS allows all origins (`allow_origin(Any)`) | Any browser origin can call the API | Acceptable for a local proxy; restrict at the reverse proxy for multi-user deployments |
| No log redaction | API keys and prompt content may appear in logs if included in request fields | Avoid logging to untrusted sinks; use a log scrubbing pipeline for production |
| No security response headers | `X-Content-Type-Options`, `X-Frame-Options`, etc. are not set | Add these headers at the reverse proxy layer |
| No virtual key / auth system | Any client that can reach the port can make requests | Restrict network access; do not expose Rausu to untrusted networks |
| No rate limiting or budget enforcement | A single client can exhaust provider quotas | Enforce at the reverse proxy or via provider-side spend limits |

## Roadmap

Planned security features by phase:

### Phase 3 — Rate Limiting
- Per-key requests per minute (RPM) and tokens per minute (TPM) limits
- Hard and soft spend limits per key, team, and user
- Circuit breaker: automatic provider disablement on repeated failures

### Phase 4 — Guardrails
- Content filtering: pattern-based request/response blocking
- PII masking: detect and redact personal information before forwarding to providers
- Prompt injection detection: heuristic scanning for common injection patterns
- Middleware pipeline: ordered guardrail chain with pre-request and post-response hooks

### Future
- Virtual key system with hashed storage (SHA-256), scoped provider access, and key rotation
- Log redaction for sensitive fields (API keys, credentials)
- Optional TLS termination
- Configurable CORS policy
- Security response headers (`X-Content-Type-Options`, `X-Frame-Options`, `X-Request-Id`)

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

1. **Run behind a reverse proxy** (nginx, Caddy) for TLS termination, size limits, and additional rate limiting
2. **Use environment variables** for all secrets — never commit API keys to config files
3. **Restrict network access** — Rausu should only be reachable by authorized clients; do not expose it to the public internet
4. **Enable request logging** to an external log aggregation system with appropriate access controls
5. **Monitor spend** — configure budget alerts at the provider level to detect unusual usage early
6. **Pin the release** — use a specific version tag, not `latest`
7. **Rotate credentials regularly** — upstream provider API keys should be rotated on a schedule
