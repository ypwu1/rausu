# 需求文档 — Rust LLM Gateway

> [English Version](./REQUIREMENTS.md)

## 1. 项目概述

### 1.1 愿景

一个用 Rust 编写的高性能、低资源消耗的 LLM API 网关——作为 LiteLLM Proxy 的替代方案，在性能、内存占用和部署便捷性上全面超越。

### 1.2 目标

- **统一接口**：提供 OpenAI 兼容 API，将请求代理到 100+ 家 LLM 供应商
- **单一二进制**：一个可执行文件搞定本地安装或 Docker 部署——零运行时依赖
- **高性能**：代理开销 P95 延迟 < 8ms @ 1,000 RPS
- **低资源**：空闲内存 < 50MB，CPU 占用极低
- **可扩展**：通过插件/模块机制添加新 provider，无需 fork

### 1.3 非目标（v1）

- 不是模型推理引擎（不做 inference）
- 不是多租户 SaaS 平台（优先单组织场景）
- 不是完整的可观测性平台（与现有平台集成）

### 1.4 当前执行策略

> 架构细节详见 [docs/ARCHITECTURE_DIRECTION_CN.md](docs/ARCHITECTURE_DIRECTION_CN.md)

Rausu 采用 **本地优先、网关兼容** 的执行顺序。

**当前重点——本地代理产品化：**
- 面向 AI 编程工具（Codex CLI、Claude Code）的单用户本地代理
- 订阅型 provider：`claude-subscription`（Claude 网页订阅，本地认证注入）和 `chatgpt-subscription`（ChatGPT 网页订阅，本地认证注入）
- 本地使用无需上游 API Key；Rausu 负责处理真实认证
- 无数据库、无虚拟 Key、无管理面板——仅基于文件的配置
- 优先端点：`/v1/responses`、`/v1/responses/compact`、`/v1/chat/completions`、`/v1/messages`

**后续阶段——网关扩展：**
- 多用户 / 远程部署相关能力（认证授权、虚拟 Key、速率限制）
- 管理面板、费用追踪、安全防护
- 广泛的 provider 覆盖（Bedrock、Azure、Vertex AI、Ollama、100+ provider）

架构设计保持网关扩展路径开放，同时不向本地运行时引入过早的复杂性。两个运行时共享同一个核心层（provider 抽象、路由、流式中继、错误映射）。详见第 5 节的分阶段交付计划。

---

## 2. 功能需求

### 2.1 API 端点（OpenAI 兼容）

所有端点遵循 OpenAI API 规范，无论上游 provider 是谁，均返回统一的响应格式。

| 端点 | 描述 | 优先级 |
|---|---|---|
| `POST /v1/chat/completions` | 聊天补全（流式 & 非流式） | P0 |
| `POST /v1/responses` | OpenAI Responses API（Codex CLI 主端点） | P0 |
| `POST /v1/responses/compact` | Responses API 紧凑变体（Codex CLI） | P0 |
| `POST /v1/messages` | Anthropic Messages API（原生透传，Claude Code） | P0 |
| `GET /v1/models` | 列出可用模型 | P0 |
| `GET /health` | 健康检查 | P0 |
| `POST /v1/embeddings` | 文本嵌入 | P1 |
| `POST /v1/images/generations` | 图像生成 | P1 |
| `POST /v1/audio/transcriptions` | 音频转文字（Whisper 兼容） | P1 |
| `POST /v1/audio/speech` | 文字转语音 | P1 |
| `POST /v1/rerank` | 重排序 | P1 |
| `POST /v1/moderations` | 内容审核 | P2 |
| `POST /v1/batches` | 批处理 | P2 |

### 2.2 Provider 抽象

#### 2.2.1 核心 Provider Trait

每个 provider 实现一个统一的 trait，覆盖所有支持的端点类型。Provider 只需实现其支持的端点；不支持的端点返回标准化的 `405 Unsupported` 错误。

#### 2.2.2 Provider 列表（按优先级）

**Phase 1（MVP——本地代理）：**
- `claude-subscription`（Claude 网页订阅，本地认证注入）
- `chatgpt-subscription`（ChatGPT 网页订阅，本地认证注入）
- OpenAI（API Key）
- Anthropic（API Key）

**Phase 3（API 网关扩展）：**
- AWS Bedrock
- Azure OpenAI
- Google Vertex AI
- Ollama

**Phase 4+：**
- vLLM
- NVIDIA NIM
- Groq
- Mistral
- Cohere
- DeepSeek

**Phase 6+：**
- 其余 provider 通过社区贡献和/或插件系统扩展
- 目标：100+ provider

#### 2.2.3 统一 Schema

- **请求**：所有入站请求遵循 OpenAI 格式；网关内部翻译为各 provider 的原生格式
- **响应**：所有响应在返回客户端前标准化为 OpenAI 格式
- **错误**：provider 特定的错误映射为 OpenAI 错误码（`401`、`429`、`500` 等），原始错误详情保留在 metadata 中
- **流式**：基于 SSE 的流式传输，统一 chunk 格式，不受上游协议影响

### 2.3 路由与负载均衡

| 功能 | 描述 | 优先级 |
|---|---|---|
| **模型路由** | 将虚拟模型名称映射到一个或多个 provider 部署 | P0 |
| **故障转移** | 出错时自动切换到下一个 provider（可按错误类型配置） | P0 |
| **重试** | 可配置的指数退避重试 | P0 |
| **加权路由** | 按权重分配流量到各部署 | P1 |
| **延迟优先** | 路由到延迟最低的 provider | P2 |
| **成本优先** | 对等模型路由到最便宜的 provider | P2 |

### 2.4 认证与 Key 管理

> **范围说明：** 虚拟 Key、团队/用户绑定、预算限制和速率限制是**网关阶段功能**（见 §1.4）。本地代理 MVP 使用基于文件的配置；本地 HTTP 服务器接受客户端传入的任意 API Key（假 Key 兼容），Rausu 负责处理真实的上游认证。

| 功能 | 描述 | 优先级 |
|---|---|---|
| **虚拟 Key** | 发放代理 API Key，映射到上游 provider 凭证 | P0 |
| **Key CRUD** | 通过 API 创建 / 列出 / 吊销 / 轮换虚拟 Key | P0 |
| **团队/用户绑定** | 将 Key 关联到团队和用户 | P1 |
| **预算限制** | 按 Key / 团队 / 用户设置最大花费（硬限制 & 软限制） | P1 |
| **速率限制** | 按 Key 限制每分钟请求数 / 每分钟 token 数 | P1 |
| **Key 作用域** | 限制 Key 只能访问特定模型或端点 | P2 |

### 2.5 费用追踪

> **范围说明：** 带数据库和费用 API 的完整费用追踪是**网关阶段功能**（Phase 4+）。本地代理 MVP 在本地记录用量，无需数据库。

| 功能 | 描述 | 优先级 |
|---|---|---|
| **单请求成本** | 使用 provider 定价表计算每次请求的成本 | P0 |
| **成本聚合** | 按 Key / 团队 / 用户 / 模型 / provider 聚合 | P1 |
| **定价配置** | 用户可配置的定价覆盖 | P1 |
| **费用 API** | 通过 REST API 查询费用数据 | P1 |
| **预算告警** | 接近预算限制时通过 Webhook/日志告警 | P2 |
| **导出** | 导出费用数据为 CSV/JSON | P2 |

### 2.6 安全防护（Guardrails）

| 功能 | 描述 | 优先级 |
|---|---|---|
| **内容过滤** | 阻止匹配可配置模式的请求/响应 | P1 |
| **PII 脱敏** | 检测并遮蔽/删除请求中的个人身份信息 | P1 |
| **提示注入检测** | 基于启发式规则的基础提示注入检测 | P2 |
| **自定义规则** | 通过配置文件定义自定义防护规则 | P1 |
| **防护管道** | 有序中间件链：请求前 → 请求 → 响应 → 响应后 | P1 |

### 2.7 日志与可观测性

| 功能 | 描述 | 优先级 |
|---|---|---|
| **结构化日志** | JSON 格式日志，包含请求 ID、模型、provider、延迟、token 数、成本 | P0 |
| **请求/响应日志** | 可选的完整请求/响应体日志记录（可配置） | P1 |
| **指标端点** | Prometheus 兼容的 `/metrics` 端点 | P1 |
| **OpenTelemetry** | OTLP 链路追踪导出 | P2 |
| **回调集成** | Langfuse / Helicone / 自定义 Webhook | P2 |

### 2.8 管理面板（Admin UI）

> **范围说明：** 管理面板是**网关阶段功能**（Phase 5）。本地代理 MVP 不包含 Web 仪表盘。后续可能作为本地运行时的便捷功能添加轻量级本地统计页面。

| 功能 | 描述 | 优先级 |
|---|---|---|
| **仪表盘** | 概览：请求量、延迟、错误率、花费 | P1 |
| **Key 管理** | 创建 / 吊销 / 查看虚拟 Key | P1 |
| **费用浏览器** | 按 Key / 团队 / 用户 / 模型 下钻费用 | P2 |
| **模型配置** | 查看/编辑模型路由和 provider 配置 | P2 |
| **日志查看器** | 搜索和过滤请求日志 | P2 |
| **防护规则配置** | 管理防护规则 | P2 |

Admin UI 是一个嵌入到二进制文件中的静态 SPA（在 `/ui` 路径提供服务），无需额外部署。

---

## 3. 非功能需求

### 3.1 性能

| 指标 | 目标 |
|---|---|
| P50 延迟（代理开销） | < 2ms |
| P95 延迟（代理开销） | < 8ms |
| P99 延迟（代理开销） | < 15ms |
| 最大并发连接数 | 10,000+ |
| 吞吐量 | 持续 1,000+ RPS |
| 启动时间 | < 1 秒 |

### 3.2 资源使用

| 指标 | 目标 |
|---|---|
| 二进制大小 | < 30MB |
| 空闲内存 | < 50MB |
| 1k RPS 下内存 | < 200MB |
| CPU（空闲） | ~0% |

### 3.3 部署

- **单二进制**：`./gateway` 或 `./gateway --config config.yaml`
- **Docker**：官方镜像，压缩后 < 50MB
- **配置**：YAML 文件 + 环境变量覆盖
- **优雅关闭**：收到 SIGTERM 时排空进行中的请求
- **热重载**：收到 SIGHUP 时重载配置（无需重启）

### 3.4 安全

- 支持 TLS 终止（可选，通常由反向代理处理）
- API Key 哈希存储（Key 以哈希形式存储，永不明文）
- 所有管理操作的审计日志
- 日志中不出现密钥

### 3.5 可靠性

- 零停机配置重载
- 每个 provider 的熔断器（自动禁用不健康的 provider）
- 请求超时，可按 provider 配置
- 优雅降级：如果费用数据库不可用，继续代理请求（记录警告）

---

## 4. 技术架构

### 4.1 技术选型

| 组件 | 选择 | 理由 |
|---|---|---|
| 语言 | Rust（2021 edition） | 性能、安全、单二进制 |
| 异步运行时 | `tokio` | 行业标准，生态成熟 |
| HTTP 服务器 | `axum` | tokio 生态中最佳开发体验 |
| HTTP 客户端 | `reqwest` | 成熟，支持流式传输、代理、TLS |
| 序列化 | `serde` + `serde_json` | 事实标准 |
| 数据库 | `sqlx` + SQLite（默认）/ PostgreSQL（可选） | 嵌入式优先，零外部依赖 |
| 日志 | `tracing` + `tracing-subscriber` | 结构化、高性能 |
| 配置 | `config` crate + YAML | 灵活，支持环境变量叠加 |
| Admin UI | 静态 SPA + `rust-embed` | 嵌入二进制文件 |
| 流式传输 | SSE via `axum` + `tokio-stream` | OpenAI streaming = SSE |

### 4.2 模块结构

```
src/
├── main.rs                  # 入口点、CLI
├── config/                  # 配置加载与校验
│   ├── mod.rs
│   └── schema.rs
├── server/                  # HTTP 服务器设置、路由
│   ├── mod.rs
│   ├── routes/
│   │   ├── chat.rs          # /v1/chat/completions
│   │   ├── embeddings.rs    # /v1/embeddings
│   │   ├── images.rs        # /v1/images/generations
│   │   ├── audio.rs         # /v1/audio/*
│   │   ├── rerank.rs        # /v1/rerank
│   │   ├── batch.rs         # /v1/batches
│   │   ├── models.rs        # /v1/models
│   │   └── admin.rs         # 管理 API
│   └── middleware/
│       ├── auth.rs          # API Key 验证
│       ├── rate_limit.rs    # 速率限制
│       ├── guardrails.rs    # 防护管道
│       ├── logging.rs       # 请求/响应日志
│       └── spend.rs         # 成本追踪中间件
├── providers/               # Provider 实现
│   ├── mod.rs               # Provider trait 定义
│   ├── openai.rs
│   ├── anthropic.rs
│   ├── bedrock.rs
│   ├── azure.rs
│   ├── vertex.rs
│   ├── ollama.rs
│   └── ...
├── router/                  # 路由与负载均衡
│   ├── mod.rs
│   ├── fallback.rs
│   ├── weighted.rs
│   └── latency.rs
├── schema/                  # 统一请求/响应类型
│   ├── mod.rs
│   ├── chat.rs
│   ├── embedding.rs
│   ├── image.rs
│   ├── audio.rs
│   └── error.rs
├── storage/                 # 数据库层
│   ├── mod.rs
│   ├── keys.rs              # 虚拟 Key 存储
│   ├── spend.rs             # 费用记录
│   └── audit.rs             # 审计日志
├── guardrails/              # 防护实现
│   ├── mod.rs
│   ├── pii.rs
│   ├── content_filter.rs
│   └── custom.rs
└── ui/                      # 嵌入式 Admin UI 资源
    └── mod.rs
```

### 4.3 配置示例

```yaml
# config.yaml
server:
  host: 0.0.0.0
  port: 4000
  workers: auto          # 默认为 CPU 核数

database:
  driver: sqlite         # sqlite | postgres
  url: "sqlite://data/gateway.db"

logging:
  level: info
  format: json           # json | pretty
  log_requests: true     # 记录完整请求/响应体
  log_responses: false

auth:
  master_key: "sk-master-xxx"   # 管理员 Key（环境变量：GATEWAY_MASTER_KEY）

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"
        weight: 70
      - provider: azure
        model: gpt-4o
        endpoint: "https://my-resource.openai.azure.com"
        api_key: "${AZURE_API_KEY}"
        weight: 30
    fallback_order: [openai, azure]
    retry:
      max_retries: 3
      backoff_ms: 1000

  - name: claude-sonnet
    providers:
      - provider: anthropic
        model: claude-sonnet-4-20250514
        api_key: "${ANTHROPIC_API_KEY}"

  - name: local-llama
    providers:
      - provider: ollama
        model: llama3
        base_url: "http://localhost:11434"

guardrails:
  - name: block-pii
    type: pii_masking
    enabled: true
    config:
      entities: [email, phone, ssn, credit_card]
      action: mask    # mask | block | log

  - name: content-filter
    type: content_filter
    enabled: true
    config:
      blocked_patterns: ["ignore previous instructions"]
      action: block

spend:
  enabled: true
  alert_threshold_usd: 100.0
  pricing_overrides:
    gpt-4o:
      input_per_1k: 0.0025
      output_per_1k: 0.01
```

---

## 5. 交付阶段

> 本阶段排序的架构设计依据详见 [docs/ARCHITECTURE_DIRECTION_CN.md](docs/ARCHITECTURE_DIRECTION_CN.md)。

交付顺序为**本地优先**：先将单用户本地代理做扎实，再扩展到网关/多用户领域。

### Phase 1 — 本地代理 MVP
**目标**：一个可工作的本地代理，供 Codex CLI 和 Claude Code 使用现有订阅。

- [x] `axum` HTTP 服务器，支持 `/v1/chat/completions`、`/v1/responses`、`/v1/responses/compact`、`/v1/messages`
- [x] `claude-subscription` provider（Claude 网页认证，`/v1/messages` 原生透传）
- [x] `chatgpt-subscription` provider（ChatGPT 网页认证，`/v1/responses` 原生透传）
- [x] Provider trait + OpenAI provider + Anthropic provider（API Key）
- [x] SSE 流式透传
- [x] YAML 配置 + 环境变量插值
- [x] `tracing` 结构化日志（JSON）
- [x] 单二进制构建 + Dockerfile
- [x] 基础错误映射（provider 错误 → OpenAI 错误码）
- [x] 健康检查端点（`/health`）
- [x] README（中英文）

**退出标准**：Codex CLI 和 Claude Code 可以指向 Rausu，无需提供真实 API Key 即可使用现有订阅发起请求。

### Phase 2 — 本地代理稳固化
**目标**：可靠、无摩擦的单用户本地代理体验。

- [ ] 假 Key 兼容——接受本地客户端传入的任意 API Key（Rausu 负责处理真实上游认证）
- [ ] 支持 `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` 覆盖（工具透明接管）
- [ ] 超时、指数退避重试
- [ ] 结构化单请求日志记录（本地文件，无需数据库）
- [ ] 优雅关闭改进
- [ ] `/v1/models` 列表反映已配置的 provider

**退出标准**：任意指向 Rausu 的 OpenAI SDK 或 Anthropic SDK 客户端均可稳定运行，无需额外配置。

### Phase 3 — API 网关扩展
**目标**：面向团队/自托管场景的生产级多 provider 路由。

- [ ] AWS Bedrock / Azure OpenAI / Google Vertex AI / Ollama provider
- [ ] 路由器：指数退避重试
- [ ] 路由器：故障转移链
- [ ] 路由器：加权负载均衡
- [ ] `/v1/embeddings` 端点
- [ ] `/v1/images/generations` 端点
- [ ] 基础 API Key 认证（master key）
- [ ] 每个 provider 的熔断器
- [ ] 远程绑定（非 localhost）+ 可选 TLS 终止

**退出标准**：能在多个 provider 间路由流量，支持自动故障转移，可用于自托管部署。

### Phase 4 — 费用追踪 + Key 管理
**目标**：多 Key 访问控制与成本可见性。

- [ ] SQLite 存储层（sqlx）
- [ ] 虚拟 Key CRUD API
- [ ] 单请求成本计算
- [ ] 按 Key / 团队 / 用户的费用聚合
- [ ] 预算限制（硬限制 + 软限制）
- [ ] 速率限制（RPM / TPM per key）
- [ ] `/v1/audio/transcriptions` + `/v1/audio/speech`
- [ ] `/v1/rerank`
- [ ] `/v1/batches`
- [ ] 费用查询 API

**退出标准**：能发放带预算限制的虚拟 Key 并查询费用数据。

### Phase 5 — 防护 + 管理面板
**目标**：面向网关部署的内容安全和可视化管理。

- [ ] 防护中间件管道
- [ ] PII 检测与脱敏
- [ ] 内容过滤（基于模式匹配）
- [ ] 通过配置的自定义防护规则
- [ ] 嵌入式 Admin SPA（嵌入二进制文件）
  - 仪表盘（请求量、延迟、错误、花费）
  - Key 管理
  - 费用浏览器
  - 日志查看器
- [ ] Prometheus `/metrics` 端点
- [ ] 审计日志

**退出标准**：非技术人员可通过 UI 管理网关。

### Phase 6 — 生态扩展
**目标**：社区增长与高级功能。

- [ ] Plugin/WASM 扩展机制（自定义 provider 热加载）
- [ ] vLLM / NIM / Groq / Mistral / Cohere / DeepSeek provider
- [ ] OpenTelemetry 链路追踪导出
- [ ] 回调集成（Langfuse、Helicone、Webhook）
- [ ] MCP Gateway 集成
- [ ] A2A 协议支持
- [ ] PostgreSQL 存储后端选项
- [ ] 热配置重载（SIGHUP）
- [ ] 成本优先 / 延迟优先智能路由

**退出标准**：可扩展平台，20+ provider，且社区在持续增长。

---

## 6. 成功指标

| 指标 | 目标 |
|---|---|
| 代理开销 P95 | < 8ms |
| 二进制大小 | < 30MB |
| 空闲内存 | < 50MB |
| Docker 镜像大小 | < 50MB |
| 启动时间 | < 1s |
| Provider 覆盖（Phase 3） | 6+ provider |
| Provider 覆盖（Phase 6） | 20+ provider |

---

## 7. 风险与应对

| 风险 | 严重度 | 应对措施 |
|---|---|---|
| Provider API 格式变更 | 中 | 锁定 provider SDK 版本；监控 changelog；trait 抽象隔离 |
| 范围蔓延（过早做太多 provider） | 中 | 严格的阶段门禁；长尾 provider 由社区 PR 贡献 |
| Admin UI 复杂度 | 低 | 保持 UI 精简；使用成熟的 SPA 框架；嵌入为静态资源 |
| SQLite 高写入量下竞争 | 低 | 开启 WAL 模式；批量写入；重负载可选 Postgres |
| 各 provider 流式传输的边界情况 | 中 | 全面的集成测试；SSE 解析的 fuzz 测试 |
