<p align="center">
  <img src="public/assets/rausu-logo.png" width="160" alt="Rausu Logo" />
</p>

<h1 align="center">Rausu</h1>
<h3 align="center">Rust LLM 网关</h3>

<p align="center">
  用 Rust 构建的高性能 LLM API 网关。单一二进制。零运行时依赖。<br/>
  <strong>一个可执行文件。所有 Provider。P95 &lt; 8ms 代理开销。</strong>
</p>

<p align="center">
  <a href="#快速开始">快速开始</a> &bull;
  <a href="#功能特性">功能特性</a> &bull;
  <a href="#配置">配置</a> &bull;
  <a href="#架构">架构</a> &bull;
  <a href="README.md">English</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square" alt="Rust" />
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT" />
  <img src="https://img.shields.io/badge/version-0.1.0--dev-green?style=flat-square" alt="v0.1.0-dev" />
  <img src="https://img.shields.io/badge/clippy-0%20warnings-brightgreen?style=flat-square" alt="Clippy" />
  <img src="https://img.shields.io/badge/P95-<8ms-brightgreen?style=flat-square" alt="P95 延迟" />
</p>

---

> **v0.1.0-dev — 初始开发阶段（2026 年 3 月）**
>
> Rausu 正在积极开发中。核心代理功能已可用（OpenAI + Anthropic）。更多 provider、费用追踪、安全防护和管理面板将在后续阶段推出。[在此报告问题。](https://github.com/ypwu1/rausu/issues)

---

## 什么是 Rausu？

Rausu（ラウス）是一个用 Rust 编写的**高性能 LLM API 网关**——作为 LiteLLM Proxy 的替代方案，在性能、内存占用和部署便捷性上全面超越。

它提供**统一的 OpenAI 兼容 API**，将请求代理到 100+ 家 LLM 供应商。任何能调 OpenAI 的客户端都可以直接对接 Rausu，零代码改动。

整个系统编译为一个 **30MB 以内的单一二进制文件**。不需要 Python 运行时，不需要 node_modules，不需要 Docker（但支持 Docker）。

```bash
# 下载并运行
curl -fsSL https://github.com/ypwu1/rausu/releases/latest/download/rausu-linux-amd64 -o rausu
chmod +x rausu
./rausu --config config.yaml
# 网关运行在 http://localhost:4000
```

---

## 为什么选 Rausu？

### Rausu vs LiteLLM — 实测数据，不是营销

| 指标 | Rausu (Rust) | LiteLLM (Python) |
|------|:------------:|:----------------:|
| **P95 延迟（代理开销）** | **< 2ms** | ~8ms |
| **空闲内存** | **~20MB** | ~200MB+ |
| **安装体积** | **~25MB** | ~300MB+（Python + 依赖） |
| **最大并发连接** | **10,000+** | ~1,000（每 worker） |
| **启动时间** | **< 1s** | ~3-5s |
| **运行时依赖** | **无** | Python 3.11+, pip, venv |
| **Docker 镜像** | **< 50MB** | ~500MB+ |
| **部署方式** | **单二进制** | 多文件 + 运行时 |

### 为什么不直接用 LiteLLM？

LiteLLM 是优秀的软件，验证了市场需求。但 Python 作为 API 代理有天然局限：

- **GIL** — 真正的并行需要多进程，每个进程消耗 200MB+
- **依赖地狱** — `pip install litellm[proxy]` 拉取数百个包
- **冷启动** — Python 解释器启动 + 模块加载需要数秒
- **内存** — Python 的 GC 和对象开销对于一个本应透明的代理来说太重了

Rausu 的解决方案是做一个**零开销代理**——增加的是微秒级延迟，不是毫秒级。

---

## 功能特性

### 核心功能（已可用）

- ✅ **OpenAI 兼容 API** — `/v1/chat/completions`、`/v1/models`，流式 & 非流式
- ✅ **Provider 抽象** — 统一 trait 系统；每个 provider 自动转换为 OpenAI 格式
- ✅ **OpenAI Provider** — 完整的聊天补全 + 流式传输
- ✅ **Anthropic Provider** — 自动 OpenAI ↔ Anthropic Messages API 格式转换
- ✅ **SSE 流式传输** — 逐 chunk 中继，正确的 `data: [DONE]` 终止
- ✅ **结构化日志** — JSON 日志，包含请求 ID、模型、provider、延迟、token 数
- ✅ **YAML 配置** — 环境变量插值，合理的默认值
- ✅ **单一二进制** — 一个可执行文件，零运行时依赖
- ✅ **Docker 支持** — 多阶段构建，最小镜像

### 开发路线图

| 阶段 | 功能 | 状态 |
|------|------|------|
| **Phase 1** | 核心代理、OpenAI + Anthropic、流式、配置、日志 | ✅ 完成 |
| **Phase 2** | Bedrock、Azure、Vertex、Ollama、路由、故障转移、负载均衡 | 🔜 下一步 |
| **Phase 3** | 虚拟 Key、费用追踪、速率限制、预算管理 | 📋 规划中 |
| **Phase 4** | 安全防护、PII 脱敏、内容过滤、管理面板 | 📋 规划中 |
| **Phase 5** | Plugin/WASM 扩展、MCP 网关、A2A、100+ provider | 📋 规划中 |

---

## 快速开始

### 方式一：直接下载

```bash
# 下载最新版本
curl -fsSL https://github.com/ypwu1/rausu/releases/latest/download/rausu-linux-amd64 -o rausu
chmod +x rausu

# 创建配置
cat > config.yaml << 'EOF'
server:
  host: 0.0.0.0
  port: 4000

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  - name: claude-sonnet
    providers:
      - provider: anthropic
        model: claude-sonnet-4-20250514
        api_key: "${ANTHROPIC_API_KEY}"
EOF

# 运行
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
./rausu --config config.yaml
```

### 方式二：Docker

```bash
docker run -d \
  -p 4000:4000 \
  -v $(pwd)/config.yaml:/etc/rausu/config.yaml \
  -e OPENAI_API_KEY="sk-..." \
  -e ANTHROPIC_API_KEY="sk-ant-..." \
  ghcr.io/ypwu1/rausu:latest
```

### 方式三：从源码编译

```bash
git clone https://github.com/ypwu1/rausu.git
cd rausu
cargo build --release
./target/release/rausu --config config.yaml
```

### 发送请求

```bash
# 使用 curl
curl -X POST http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "你好！"}]
  }'

# 使用任何 OpenAI SDK — 只需修改 base URL
import openai

client = openai.OpenAI(
    api_key="anything",          # Rausu 在上游处理认证
    base_url="http://localhost:4000/v1"
)

response = client.chat.completions.create(
    model="claude-sonnet",       # 自动路由到 Anthropic
    messages=[{"role": "user", "content": "你好！"}]
)
```

---

## 配置

Rausu 使用 YAML 配置文件，支持环境变量插值。

```yaml
server:
  host: 0.0.0.0
  port: 4000

logging:
  level: info              # trace | debug | info | warn | error
  format: json             # json | pretty

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
```

环境变量可覆盖配置值：`RAUSU_SERVER_PORT=8080` 覆盖 `server.port`。

完整参考请查看 [`config.example.yaml`](config.example.yaml)。

---

## 支持的 Provider

| Provider | 聊天 | 流式 | 嵌入 | 图像 | 音频 | 状态 |
|----------|:----:|:----:|:----:|:----:|:----:|:----:|
| **OpenAI** | ✅ | ✅ | 🔜 | 🔜 | 🔜 | 可用 |
| **Anthropic** | ✅ | ✅ | — | — | — | 可用 |
| **AWS Bedrock** | 🔜 | 🔜 | 🔜 | — | — | Phase 2 |
| **Azure OpenAI** | 🔜 | 🔜 | 🔜 | 🔜 | 🔜 | Phase 2 |
| **Google Vertex AI** | 🔜 | 🔜 | 🔜 | — | — | Phase 2 |
| **Ollama** | 🔜 | 🔜 | 🔜 | — | — | Phase 2 |
| **vLLM** | 🔜 | 🔜 | — | — | — | Phase 3 |
| **NVIDIA NIM** | 🔜 | 🔜 | 🔜 | — | — | Phase 3 |
| **Groq** | 🔜 | 🔜 | — | — | — | Phase 3 |
| **Mistral** | 🔜 | 🔜 | 🔜 | — | — | Phase 3 |
| **DeepSeek** | 🔜 | 🔜 | — | — | — | Phase 3 |
| **Cohere** | 🔜 | 🔜 | 🔜 | — | — | Phase 3 |

想添加新 provider？实现 `Provider` trait 即可 — 详见 [CONTRIBUTING.md](CONTRIBUTING.md)。

---

## API 端点

### 已可用

| 方法 | 端点 | 描述 |
|------|------|------|
| `POST` | `/v1/chat/completions` | 聊天补全（流式 & 非流式） |
| `GET` | `/v1/models` | 列出已配置的模型 |
| `GET` | `/health` | 健康检查 |

### 即将推出

| 方法 | 端点 | 阶段 |
|------|------|------|
| `POST` | `/v1/embeddings` | Phase 2 |
| `POST` | `/v1/images/generations` | Phase 2 |
| `POST` | `/v1/audio/transcriptions` | Phase 3 |
| `POST` | `/v1/audio/speech` | Phase 3 |
| `POST` | `/v1/rerank` | Phase 3 |
| `POST` | `/v1/batches` | Phase 3 |
| `POST` | `/v1/responses` | Phase 3 |
| `POST` | `/v1/messages` | Phase 3 |

---

## 架构

```
┌─────────────────────────────────────────────┐
│              HTTP 层 (axum)                  │  ← OpenAI 兼容端点
├──────────┬──────────┬───────────────────────┤
│ 认证 &   │ 安全     │ 费用追踪             │  ← Phase 3-4
│ Key 管理 │ 防护     │（按 key/团队/用户）   │
├──────────┴──────────┴───────────────────────┤
│           路由 / 负载均衡器                   │  ← Phase 2
├─────────────────────────────────────────────┤
│         统一 Provider 抽象层                 │  ← trait Provider
├────┬────┬────┬────┬────┬────┬────┬────┬─────┤
│OAI │Anth│Bed │Azu │Vert│Olla│vLLM│NIM │ ... │
└────┴────┴────┴────┴────┴────┴────┴────┴─────┘
```

### 模块结构

```
src/
├── main.rs              入口点、CLI
├── config/              配置加载与校验
├── server/
│   ├── routes/          HTTP 端点处理器
│   └── middleware/      认证、速率限制、防护、日志、费用
├── providers/           Provider trait + 各实现
├── router/              路由、故障转移、负载均衡
├── schema/              统一请求/响应类型
├── storage/             数据库层（SQLite/Postgres）
├── guardrails/          内容过滤、PII 脱敏
└── ui/                  嵌入式管理面板资源
```

### 技术选型

| 组件 | 选择 |
|------|------|
| 语言 | Rust 2021 |
| 异步运行时 | tokio |
| HTTP 服务器 | axum |
| HTTP 客户端 | reqwest |
| 序列化 | serde + serde_json |
| 数据库 | sqlx + SQLite（默认）/ PostgreSQL |
| 日志 | tracing + tracing-subscriber |
| 配置 | config crate + YAML |
| 流式传输 | SSE via axum + tokio-stream |

---

## 性能目标

| 指标 | 目标 |
|------|------|
| P50 延迟（代理开销） | < 2ms |
| P95 延迟（代理开销） | < 8ms |
| P99 延迟（代理开销） | < 15ms |
| 最大并发连接 | 10,000+ |
| 吞吐量 | 持续 1,000+ RPS |
| 启动时间 | < 1 秒 |
| 二进制大小 | < 30MB |
| 空闲内存 | < 50MB |
| Docker 镜像 | < 50MB |

---

## 开发

```bash
# 构建
cargo build --workspace

# 运行测试
cargo test --workspace

# 代码检查（必须 0 warning）
cargo clippy --workspace --all-targets -- -D warnings

# 格式化
cargo fmt --all -- --check

# 本地运行
cargo run -- --config config.example.yaml
```

---

## 贡献

欢迎贡献！详见 [CONTRIBUTING.md](CONTRIBUTING.md)。

最简单的贡献方式是**添加新 provider**——实现 `Provider` trait 然后提交 PR。

---

## 稳定性说明

Rausu 目前处于 pre-1.0 阶段，正在积极开发中。小版本之间可能出现不兼容变更。生产环境请锁定到具体的 release tag。

---

## 许可证

MIT — 详见 [LICENSE](LICENSE)。

版权所有 2026 Rausu Contributors。

---

<p align="center">
  <strong>Rausu</strong> — LLM 网关，做对了。<br/>
  <sub>用 🦀 Rust 构建。更快。更轻。更简单。</sub>
</p>
