# Observability — OpenTelemetry tracing

> [中文版](OBSERVABILITY_CN.md)

Rausu can export distributed traces over OTLP/HTTP to any OpenTelemetry
backend (Collector, Jaeger, Honeycomb, Tempo, Datadog, etc.).  This page
explains how to turn it on and what is — and is not — recorded.

---

## At a glance

- **Off by default.**  No spans are exported until you opt in.
- **OTLP/HTTP exporter** (protobuf over HTTP).  gRPC is not yet wired in.
- **W3C trace-context propagation.**  When an inbound request carries
  `traceparent`/`tracestate`/`baggage` headers, Rausu continues that trace.
- **Privacy-first attributes.**  Spans never carry prompt or response bodies.
  Only safe metadata (HTTP method, route, status code, provider name, model,
  capability flags) is recorded.

---

## Span structure

For every inbound request that reaches the gateway, Rausu emits:

```
Received Proxy Server Request          (parent — one per inbound HTTP request)
├── llm_request                         (child — one per upstream chat-completions call)
│     llm.provider, llm.request_model, llm.is_stream
└── llm_messages                        (child — one per upstream Anthropic Messages call)
      llm.provider, llm.request_model, llm.is_stream
```

When provider failover kicks in (e.g. the first provider returns 429), each
attempt produces its own child span so you can see the full retry chain.

### Safe attributes

| Span                              | Attributes (non-exhaustive)                                                                |
| --------------------------------- | ------------------------------------------------------------------------------------------ |
| `Received Proxy Server Request`   | `http.method`, `http.route`, `http.status_code`                                            |
| `llm_request` / `llm_messages`    | `llm.provider`, `llm.request_model`, `llm.is_stream`                                       |
| `chat_completions` (instrumented) | `model`, `stream`, `provider`                                                              |

What you will **never** see in a span:

- request or response message bodies
- system prompts, user content, tool call arguments
- API keys, OAuth tokens, or any header secrets

---

## Configuration

Add an `observability.otel` block to your `config.yaml`:

```yaml
observability:
  otel:
    enabled: true
    exporter: otlp_http
    endpoint: "http://localhost:4318/v1/traces"
    service_name: rausu
    headers:
      # Examples — pick whichever your backend needs
      # api-key: "${HONEYCOMB_API_KEY}"
      # x-honeycomb-team: "${HONEYCOMB_TEAM}"
```

### Configuration reference

| Key                        | Default                              | Description                                                  |
| -------------------------- | ------------------------------------ | ------------------------------------------------------------ |
| `enabled`                  | `false`                              | Master switch.                                               |
| `exporter`                 | `otlp_http`                          | Only `otlp_http` is supported today.                         |
| `endpoint`                 | `http://localhost:4318/v1/traces`    | Full OTLP/HTTP traces URL.                                   |
| `service_name`             | `rausu`                              | Reported as `service.name` resource attribute.               |
| `headers`                  | `{}`                                 | Static headers attached to every OTLP request (auth tokens). |

### Environment overrides

Environment variables take precedence over `config.yaml`.  Both Rausu-prefixed
and the standard OpenTelemetry environment variables are honoured:

| Rausu variable             | Standard OTel variable                                                  |
| -------------------------- | ----------------------------------------------------------------------- |
| `RAUSU_OTEL_ENABLED`       | `OTEL_SDK_DISABLED` (inverted: `true` here disables OTel)               |
| `RAUSU_OTEL_EXPORTER`      | `OTEL_TRACES_EXPORTER`                                                  |
| `RAUSU_OTEL_ENDPOINT`      | `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, `OTEL_EXPORTER_OTLP_ENDPOINT`     |
| `RAUSU_OTEL_SERVICE_NAME`  | `OTEL_SERVICE_NAME`                                                     |
| `RAUSU_OTEL_HEADERS`       | `OTEL_EXPORTER_OTLP_HEADERS`                                            |

`RAUSU_OTEL_HEADERS` and `OTEL_EXPORTER_OTLP_HEADERS` accept a
comma-separated list of `key=value` pairs:

```bash
RAUSU_OTEL_HEADERS="api-key=abc,x-tenant=team-1"
```

---

## Quickstart: local OpenTelemetry Collector

Point Rausu at an OTel Collector running on the same host:

```yaml
# otel-collector-config.yaml
receivers:
  otlp:
    protocols:
      http:
        endpoint: 0.0.0.0:4318

exporters:
  debug:
    verbosity: detailed

service:
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [debug]
```

```bash
docker run --rm -p 4318:4318 \
  -v "$(pwd)/otel-collector-config.yaml":/etc/otelcol/config.yaml \
  otel/opentelemetry-collector:latest \
  --config /etc/otelcol/config.yaml
```

Then run Rausu with tracing on (either via config or env):

```bash
RAUSU_OTEL_ENABLED=true \
RAUSU_OTEL_ENDPOINT=http://localhost:4318/v1/traces \
cargo run
```

Issue any request to the gateway and inspect the Collector logs — you should
see the parent span `Received Proxy Server Request` with one or more
`llm_request` / `llm_messages` children.

---

## Continuing existing traces

If a downstream client (e.g. an SDK already instrumented with OpenTelemetry)
sends a `traceparent` header along with its request:

```
traceparent: 00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01
```

Rausu extracts the W3C context, attaches it as the parent of its own root
span, and continues the same trace — so distributed traces span both the
caller and the gateway.

---

## Cost and overhead

- Spans are batch-exported on a background task; the request hot-path does not
  block on the exporter.
- When `enabled: false` (default), no OTel layer is installed at all.  The
  only cost is a one-time `bool` check on startup.

---

## Troubleshooting

- **No spans appear in the backend.** Check that `enabled: true` is actually
  reflected in the running process (env override could be unsetting it).  The
  startup banner logs `OpenTelemetry tracing enabled` when the layer is
  active.
- **Connection refused on `http://localhost:4318`.** Make sure the OTLP/HTTP
  endpoint is reachable.  Many backends use port `4317` for gRPC and `4318`
  for HTTP — Rausu only speaks HTTP today.
- **`Unsupported OTel exporter` on startup.** Only `otlp_http` is supported.
  Remove the `exporter` field to use the default.
