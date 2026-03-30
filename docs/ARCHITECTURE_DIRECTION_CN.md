# 架构方向 — 本地优先，网关兼容

> [English Version](ARCHITECTURE_DIRECTION.md)

## 决策

Rausu 将优先推进**本地代理产品化**。当前目标是构建一个稳定的单用户 localhost 代理，与 Codex CLI、Claude Code 等工具无缝协作，利用现有的 ChatGPT / Claude 订阅。

网关能力（多用户、远程绑定、认证授权、管理 API）是**未来兼容的扩展方向**，不属于当前 MVP 范围。架构必须保留这条路径，但不能将不必要的复杂性引入本地运行时。

## 三层架构

```
┌─────────────────────────────────────────────────────────┐
│                  网关运行时（未来）                        │
│  守护进程 / 远程绑定、认证授权、多用户、                    │
│  速率限制、配额、策略、管理 API                            │
├─────────────────────────────────────────────────────────┤
│                  本地运行时（当前重点）                     │
│  localhost HTTP 代理、单用户配置、                         │
│  Codex 兼容端点、本地认证注入、                            │
│  fake-key 兼容                                           │
├─────────────────────────────────────────────────────────┤
│                  核心层（共享）                            │
│  provider 抽象、认证/令牌管理、                            │
│  请求规范化、路由原语、                                    │
│  上游传输、流式中继、                                      │
│  模型注册表、错误映射、用量统计                             │
└─────────────────────────────────────────────────────────┘
```

### 第一层：核心层

核心层与运行时无关，包含本地和网关运行时共同依赖的所有可复用逻辑：

- **Provider 抽象** — 统一 trait 系统，支持 OpenAI、Anthropic、Claude 订阅、ChatGPT 订阅等
- **认证/令牌管理** — OAuth 令牌加载、刷新、凭证解析（但不绑定到特定文件路径如 `~/.codex/auth.json`）
- **请求规范化/路由原语** — 模型查找、provider 选择、请求转换
- **上游传输** — HTTP 连接池、连接管理、超时/重试原语
- **流式中继** — SSE 逐 chunk 代理、背压处理
- **模型注册表** — 模型 → provider 映射、支持的模型列表
- **错误映射** — 上游错误 → 一致的下游错误格式
- **用量统计** — token 计数、请求日志（存储无关）

### 第二层：本地运行时（当前重点）

本地运行时是核心层的薄壳，为单用户 localhost 场景优化：

- **Localhost HTTP 代理** — `axum` 服务器绑定 `127.0.0.1` 或 `0.0.0.0`，最小开销
- **单用户本地配置** — YAML 配置文件、环境变量覆盖、无需数据库
- **Codex 兼容端点** — `/v1/responses`、`/v1/chat/completions` 及其他 Codex CLI / Claude Code 需要的端点
- **本地认证注入** — 从本地凭证文件读取 OAuth 令牌，向上游注入认证头，接受本地客户端发送的任意 fake API key
- **Fake-key 兼容** — 客户端可以发送 `api_key: "anything"`，因为 Rausu 在上游处理真实认证

**后续扩展（仍属本地运行时）：**

- **本地接管 / base_url 切换** — 拦截硬编码上游 URL 的工具调用（如 `OPENAI_BASE_URL` 覆盖）
- **本地用量面板** — 可选的轻量级统计页面

### 第三层：网关运行时（未来）

网关运行时在核心层基础上扩展多用户/远程部署能力：

- **守护进程/远程绑定** — 监听非 localhost 接口、TLS 终止
- **认证/授权** — 虚拟 API key、按用户身份、RBAC
- **多用户/多账户** — 多套上游凭证、按用户路由
- **速率限制/配额** — 按 key、按用户、按模型的速率限制
- **策略** — 内容过滤、PII 脱敏、安全防护
- **管理 API** — 用户管理、key 分发、用量报告

## 近期端点优先级

本地代理 MVP 优先支持以下端点：

| 端点 | 用途 | 备注 |
|------|------|------|
| `POST /v1/responses` | Codex CLI 主要端点 | 透传到 ChatGPT Responses API |
| `POST /v1/responses/compact` | Codex CLI 紧凑变体 | 透传 |
| `POST /v1/chat/completions` | 通用 OpenAI 兼容 | 已有实现；适用于所有 provider |

这些是 Codex CLI 和 Claude Code 实际调用的端点。在这些端点稳固之前，其他端点均为次要。

## 透传优于转换

**决策：** 对于 ChatGPT 订阅 / Codex CLI 代理，优先选择**原生透传**而非格式转换。

**理由：**

1. **Codex CLI 原生使用 Responses API。** 当用户通过 `OPENAI_BASE_URL` 将 Codex CLI 指向 Rausu 时，CLI 发送的是 Responses API 请求。将其转换为 Chat Completions 再转回来会导致：
   - 有损 — Responses API 有 Chat Completions 无法表达的字段
   - 慢 — 不必要的序列化/反序列化往返
   - 脆弱 — 上游 API 变更会同时破坏两个转换层

2. **Claude Code 原生使用 Messages API。** 同理 — `/v1/messages` 应原样透传到 Anthropic API。

3. **转换只在边界处需要。** 当客户端使用 OpenAI Chat Completions 但上游是 Anthropic 时，转换不可避免且有价值。当客户端和上游使用相同协议时，转换纯属开销。

**经验法则：** 客户端协议与上游协议匹配时，透传。不匹配时，在最靠近上游的边界处转换。

## 非目标 / 延后范围

以下内容明确**不在**本地代理 MVP 范围内：

- **多租户 SaaS** — 无用户隔离、无计费、无租户管理
- **重型 Web UI** — 无管理面板、无模型 Playground（后期可添加轻量级本地统计页面）
- **分布式控制面** — 无服务网格、无多节点协调
- **高级配额/计费系统** — 无额度追踪、无按用量计费、无发票生成
- **Plugin / WASM 扩展系统** — 核心稳定前为时过早
- **数据库依赖** — 本地运行时必须无需 SQLite/Postgres，仅使用文件配置

这些都是合理的网关阶段功能，是延后而非否决。

## 分阶段路线图

| 阶段 | 范围 | 描述 |
|------|------|------|
| **Phase 2.5** | Codex CLI 的 Responses 透传 | 添加 `/v1/responses` 端点，通过 `chatgpt-subscription` provider 透传到 ChatGPT Responses API。这是 Codex CLI 支持的关键路径。 |
| **Phase 2.6** | 本地 fake-key / 接管支持 | 接受本地客户端的任意 API key。支持 `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` 覆盖模式，使工具无需代码修改即可指向 Rausu。 |
| **Phase 2.7** | 可靠性加固 | 超时、带退避的重试、结构化请求/响应日志、用量追踪（本地文件）、健康检查改进、优雅关闭。 |
| **未来** | 网关运行时扩展 | 远程绑定、认证授权、多用户、速率限制、管理 API。仅在本地代理达到生产级稳定后启动。 |

## 实现指南：避免核心层与运行时耦合

最重要的架构纪律是保持核心层不包含运行时假设。

### 正确做法：抽象凭证来源

```rust
// 正确：核心层定义 trait
trait TokenSource: Send + Sync {
    async fn get_token(&self) -> Result<String>;
}

// 本地运行时提供基于文件的实现
struct FileTokenSource { path: PathBuf }

// 网关运行时可以提供基于数据库的实现
struct DbTokenSource { pool: PgPool, user_id: Uuid }
```

### 错误做法：在核心层硬编码文件路径

```rust
// 错误：核心层知道 ~/.codex/auth.json
fn load_token() -> String {
    std::fs::read_to_string(home_dir().join(".codex/auth.json"))
}
```

### 具体规则

1. **核心层不得引用特定文件路径** — 不能有 `~/.claude/`、`~/.codex/`、`~/.config/rausu/`。这些属于本地运行时的配置层。
2. **核心层不得假设单用户** — 令牌来源、配置加载器和请求处理器应接受注入的依赖，而非访问全局状态。
3. **核心层不得假设 localhost** — 不硬编码 `127.0.0.1`，不假设 TLS 配置。运行时决定绑定地址和传输方式。
4. **核心层不得依赖数据库** — 用量统计应接受一个 trait（可以是内存、文件、SQLite 或 Postgres，取决于运行时）。
5. **配置解析属于运行时** — 核心层接受类型化的配置结构体，而非原始 YAML 或文件路径。

这种纪律在近期也有回报：它使本地运行时更容易测试，因为可以注入 mock 令牌源和内存存储，而无需触及文件系统。
