# 可观测性 — OpenTelemetry 链路追踪

> [English version](OBSERVABILITY.md)

Rausu 支持通过 OTLP/HTTP 将分布式链路（trace）导出到任意 OpenTelemetry 后端
（Collector、Jaeger、Honeycomb、Tempo、Datadog 等）。本文档说明如何启用，以及
被记录与不会被记录的内容。

---

## 一图看懂

- **默认关闭。** 不主动启用就不会导出任何 span。
- **OTLP/HTTP 导出器**（基于 HTTP 的 protobuf）。gRPC 暂未集成。
- **W3C 链路上下文透传。** 入站请求若携带 `traceparent`/`tracestate`/`baggage`
  这类标准头，Rausu 会延续该 trace。
- **隐私优先。** Span 永远不会记录请求或响应正文，仅保留 HTTP 方法、路由、状态
  码、provider 名称、模型名、能力开关等安全元数据。

---

## Span 结构

每个进入网关的请求会产生：

```
Received Proxy Server Request          （父 span — 每个入站 HTTP 请求一个）
├── llm_request                         （子 span — 每次上游 chat completions 调用）
│     llm.provider, llm.request_model, llm.is_stream
└── llm_messages                        （子 span — 每次上游 Anthropic Messages 调用）
      llm.provider, llm.request_model, llm.is_stream
```

当触发 provider 故障转移（例如第一个 provider 返回 429）时，每次尝试都会产生
独立的子 span，可以完整看到失败重试链。

### 安全属性一览

| Span                              | 属性（节选）                                                           |
| --------------------------------- | ---------------------------------------------------------------------- |
| `Received Proxy Server Request`   | `http.method`、`http.route`、`http.status_code`                        |
| `llm_request` / `llm_messages`    | `llm.provider`、`llm.request_model`、`llm.is_stream`                   |
| `chat_completions`（已 instrument） | `model`、`stream`、`provider`                                          |

**绝不会**出现的内容：

- 请求或响应消息正文
- 系统提示词、用户内容、工具调用参数
- API key、OAuth token 或任何敏感请求头

---

## 配置

在 `config.yaml` 中加入 `observability.otel` 块即可：

```yaml
observability:
  otel:
    enabled: true
    exporter: otlp_http
    endpoint: "http://localhost:4318/v1/traces"
    service_name: rausu
    headers:
      # 视后端需要而定
      # api-key: "${HONEYCOMB_API_KEY}"
      # x-honeycomb-team: "${HONEYCOMB_TEAM}"
```

### 配置项参考

| 字段                       | 默认值                             | 说明                                                  |
| -------------------------- | ---------------------------------- | ----------------------------------------------------- |
| `enabled`                  | `false`                            | 总开关。                                              |
| `exporter`                 | `otlp_http`                        | 目前仅支持 `otlp_http`。                              |
| `endpoint`                 | `http://localhost:4318/v1/traces`  | OTLP/HTTP traces 端点 URL。                           |
| `service_name`             | `rausu`                            | 上报为 `service.name` 资源属性。                      |
| `headers`                  | `{}`                               | 附加在每个 OTLP 请求上的静态请求头（如鉴权 token）。  |

### 环境变量覆盖

环境变量优先级高于 `config.yaml`，同时支持 Rausu 自有变量与 OpenTelemetry 标准
变量：

| Rausu 变量                  | 标准 OTel 变量                                                          |
| --------------------------- | ----------------------------------------------------------------------- |
| `RAUSU_OTEL_ENABLED`        | `OTEL_SDK_DISABLED`（反义：此处 `true` 表示禁用）                       |
| `RAUSU_OTEL_EXPORTER`       | `OTEL_TRACES_EXPORTER`                                                  |
| `RAUSU_OTEL_ENDPOINT`       | `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`、`OTEL_EXPORTER_OTLP_ENDPOINT`     |
| `RAUSU_OTEL_SERVICE_NAME`   | `OTEL_SERVICE_NAME`                                                     |
| `RAUSU_OTEL_HEADERS`        | `OTEL_EXPORTER_OTLP_HEADERS`                                            |

`RAUSU_OTEL_HEADERS` 与 `OTEL_EXPORTER_OTLP_HEADERS` 接收逗号分隔的
`key=value` 列表：

```bash
RAUSU_OTEL_HEADERS="api-key=abc,x-tenant=team-1"
```

---

## 快速上手：本地 OpenTelemetry Collector

将 Rausu 指向同主机上运行的 OTel Collector：

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

随后启用 tracing 启动 Rausu（配置或环境变量任选其一）：

```bash
RAUSU_OTEL_ENABLED=true \
RAUSU_OTEL_ENDPOINT=http://localhost:4318/v1/traces \
cargo run
```

向网关发起任意请求并查看 Collector 日志，即可看到父 span
`Received Proxy Server Request` 以及若干 `llm_request` / `llm_messages` 子 span。

---

## 续接已存在的 trace

下游客户端（例如已用 OpenTelemetry 自动埋点的 SDK）若在请求中携带
`traceparent` 头：

```
traceparent: 00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01
```

Rausu 会提取 W3C 上下文，并以此为父 span 开始自身根 span，从而让分布式 trace
横跨调用方与网关。

---

## 性能开销

- Span 通过后台批处理任务异步导出，请求关键路径不会阻塞在导出器上。
- `enabled: false`（默认）下根本不会安装 OTel 层，启动期仅多一次 `bool` 判断。

---

## 排障

- **后端看不到 span。** 检查 `enabled: true` 是否真的生效（环境变量可能反向覆盖）。
  当 OTel 层被启用时，启动日志会打印 `OpenTelemetry tracing enabled`。
- **`Connection refused`，访问 `http://localhost:4318` 失败。** 确认 OTLP/HTTP
  端点可达。许多后端用 `4317` 端口跑 gRPC、用 `4318` 跑 HTTP — Rausu 当前仅支持
  HTTP。
- **启动报错 `Unsupported OTel exporter`。** 当前仅支持 `otlp_http`，移除
  `exporter` 字段即可使用默认值。
