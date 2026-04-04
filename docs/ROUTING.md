# Priority Failover Routing

Rausu supports configuring multiple providers per model with automatic failover. When the primary provider fails with a retryable error, the request is automatically retried on the next provider in priority order.

## Configuration

List multiple providers under a model's `providers` field. The order in the YAML defines the priority (first = highest):

```yaml
models:
  - name: claude-sonnet-4-6
    aliases: ["claude-sonnet-4-6-20250514"]
    providers:
      - provider: github-copilot        # Priority 1: free via Copilot
        model: claude-sonnet-4.6
      - provider: claude-subscription    # Priority 2: subscription
        model: claude-sonnet-4-6
      - provider: anthropic              # Priority 3: pay-per-use API
        model: claude-sonnet-4-6

  - name: gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o
      - provider: chatgpt-subscription
        model: gpt-4o
      - provider: openai
        model: gpt-4o
```

Aliases resolve to the same provider list as the primary model name.

## How It Works

1. A request arrives for model `claude-sonnet-4-6`.
2. Rausu tries the first provider (`github-copilot`).
3. If that provider returns a retryable error (e.g. 429 rate limit), Rausu logs a warning and tries the next provider (`claude-subscription`).
4. If that also fails with a retryable error, Rausu tries the last provider (`anthropic`).
5. If all providers fail, Rausu returns 503 Service Unavailable.

For non-retryable errors (e.g. 400 Bad Request), the error is returned immediately without trying additional providers.

## Retryable Errors

The following trigger failover to the next provider:

| Status Code | Meaning |
|---|---|
| 429 | Rate limited |
| 500 | Internal server error |
| 502 | Bad gateway |
| 503 | Service unavailable |
| 504 | Gateway timeout |

Additionally:
- **Transport failures** (connection refused, DNS errors, timeouts) are always retryable.
- **Unsupported operations** (e.g. a provider that doesn't support the Messages API) are skipped automatically.

Non-retryable errors (400, 401, 403, 404, etc.) are returned to the client immediately.

## Streaming Safety

For proxy endpoints (Messages API, Responses API), the failover decision is made based on the upstream HTTP status code **before** any response bytes are streamed to the client. Once streaming begins, the provider cannot be switched.

For Chat Completions streaming, failover happens if the initial `chat_completions_stream()` call returns an error. Once the SSE stream starts yielding chunks, the provider is committed.

## Logging

Rausu logs failover activity at various levels:

- **INFO**: Which provider is being tried (`Trying provider`) and which succeeded (`Request served by provider`).
- **WARN**: When a provider fails and failover occurs (`Provider failed, falling back`).
- **ERROR**: When all providers are exhausted (`All providers failed`).

Example log output:
```
INFO  Trying provider model=claude-sonnet-4-6 provider=github-copilot attempt=1
WARN  Provider failed, falling back model=claude-sonnet-4-6 provider=github-copilot status=429
INFO  Trying provider model=claude-sonnet-4-6 provider=claude-subscription attempt=2
INFO  Request served by provider model=claude-sonnet-4-6 provider=claude-subscription
```

## Interaction with Auth and TLS

- **API key auth**: Each request to Rausu still requires a valid API key (if auth is configured). Failover is transparent to the client.
- **TLS/mTLS**: Failover happens at the upstream provider level. The client's TLS connection to Rausu is unaffected.
- Provider-specific credentials (API keys, OAuth tokens, service account keys) are managed per-provider and used automatically during failover.

## Single Provider (No Failover)

If a model has only one provider configured, the behavior is identical to before: errors are returned directly to the client. No failover logic is executed.
