<p align="center">
  <img src="assets/icon.jpg" width="160" alt="Rausu Icon" />
</p>

<h1 align="center">Rausu</h1>
<p align="center"><em>ラウス</em></p>

<p align="center">
  <a href="./README.md">English Version</a>
</p>

一个用 Rust 编写的高性能 LLM API 网关——作为 [LiteLLM Proxy](https://github.com/BerriAI/litellm) 的替代方案，在性能、内存占用和部署便捷性上全面超越（单一二进制文件）。

## 特性

- **OpenAI 兼容 API** — 适配任何 OpenAI SDK 客户端
- **多 Provider 支持** — 支持 OpenAI、Anthropic（API Key）、Claude 订阅（OAuth）及 ChatGPT 订阅（OAuth）
- **流式传输** — 完整的 SSE 流式支持
- **单一二进制** — 零运行时依赖
- **YAML 配置** — 支持环境变量插值
- **结构化日志** — 带请求追踪的 JSON 日志

## 快速开始

### 方式一：从源码构建

```bash
cargo build --release
./target/release/rausu --config config.yaml
```

### 方式二：Docker

```bash
docker build -t rausu .
docker run -p 4000:4000 -v $(pwd)/config.yaml:/app/config.yaml rausu
```

## 配置

复制示例配置并修改：

```bash
cp config.example.yaml config.yaml
# 编辑 config.yaml，填入你的 API Key
```

```yaml
server:
  host: 0.0.0.0
  port: 4000

logging:
  level: info
  format: json   # json | pretty

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

  # Claude Pro/Max 订阅 —— 无需 API Key
  - name: claude-sonnet-sub
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        # token_source: auto   # auto（默认）| env | credentials_file
        # credentials_path: /custom/path/.credentials.json  # 可选

  # ChatGPT Plus/Pro/Max 订阅 —— 无需 API Key
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        # token_source: auto   # auto（默认）| env | credentials_file
        # credentials_path: ~/.config/rausu/chatgpt-auth.json  # 可选
```

### `claude-subscription` Provider

通过 OAuth 使用你的 Claude Pro/Max 订阅，无需付费 API Key。

**Token 来源（按优先级顺序）：**

1. **`env`** — 设置环境变量 `CLAUDE_OAUTH_TOKEN=<access_token>`（静态，不自动刷新）
2. **`credentials_file`** — 读取 Claude CLI 写入的 `~/.claude/.credentials.json`，支持自动刷新 Token
3. **`auto`**（默认）—— 先尝试 `env`，再尝试 `credentials_file`

```yaml
models:
  - name: claude-sonnet-sub
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: credentials_file          # 可选，默认：auto
        # credentials_path: ~/.claude/.credentials.json  # 可选路径覆盖
```

### `chatgpt-subscription` Provider

通过 OAuth 使用你的 ChatGPT Plus/Pro/Max 订阅，无需付费 API Key。请求会在内部从 Chat Completions 格式桥接到 ChatGPT Responses API。

**Token 来源（按优先级顺序）：**

1. **`env`** — 设置环境变量 `CHATGPT_ACCESS_TOKEN=<access_token>`（可选配置 `CHATGPT_REFRESH_TOKEN` 和 `CHATGPT_ACCOUNT_ID`）
2. **`credentials_file`** — 读取 `~/.config/rausu/chatgpt-auth.json`，支持自动刷新 Token
3. **`auto`**（默认）—— 先尝试 `env`，再尝试 `credentials_file`

```yaml
models:
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: env              # 可选，默认：auto

  - name: gpt-5-pro
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4-pro
        token_source: credentials_file
        credentials_path: /custom/path/chatgpt-auth.json  # 可选路径覆盖
```

**凭证文件格式**（`~/.config/rausu/chatgpt-auth.json`）：

```json
{
  "access_token": "eyJ...",
  "refresh_token": "...",
  "expires_at": 1900000000000,
  "account_id": "acc_..."
}
```

**支持的模型：** `gpt-5.4`、`gpt-5.4-pro`、`gpt-5.3-codex`、`gpt-5.3-codex-spark`、`gpt-5.3-instant`、`gpt-5.3-chat-latest`

> **注意：** 四个 Provider（`openai`、`anthropic`、`claude-subscription`、`chatgpt-subscription`）完全独立，可以在同一配置文件中共存，分别服务不同的虚拟模型名称。

环境变量覆盖使用 `RAUSU__` 前缀，以 `__` 为分隔符：

```bash
RAUSU__SERVER__PORT=8080 rausu
```

## 使用方法

将你的 OpenAI SDK 指向 `http://localhost:4000`：

```python
from openai import OpenAI

client = OpenAI(
    api_key="not-used",
    base_url="http://localhost:4000/v1",
)

# 路由到 OpenAI
response = client.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "你好！"}],
)

# 路由到 Anthropic（相同的 API！）
response = client.chat.completions.create(
    model="claude-sonnet",
    messages=[{"role": "user", "content": "你好！"}],
)
```

## API 端点

| 端点 | 方法 | 描述 |
|------|------|------|
| `/health` | GET | 健康检查 |
| `/v1/models` | GET | 列出已配置的模型 |
| `/v1/chat/completions` | POST | 聊天补全 — 路由 + 格式转换 |
| `/v1/responses` | POST | OpenAI Responses API — 透明直传（Codex CLI） |
| `/v1/responses/compact` | POST | OpenAI Responses API 紧凑变体 — 透明直传 |
| `/v1/messages` | POST | Anthropic Messages API — 透明直传（Claude Code） |

> **注意：** 所有 `/v1/...` 路由也可以不带前缀使用（例如 `/responses`、`/chat/completions`、`/models`、`/messages`）。这使得像 Codex CLI 这样使用 `{base_url}/responses` 而非 `{base_url}/v1/responses` 的客户端无需额外配置即可工作。

## 本地代理使用

Rausu 可作为 Codex CLI 和 Claude Code 的单用户本地代理运行。本地客户端传入占位 API Key，Rausu 自动注入真实的上游凭证。

详见 [docs/LOCAL_PROXY_USAGE_CN.md](docs/LOCAL_PROXY_USAGE_CN.md)，包含配置示例、伪 Key 行为说明以及 Codex CLI 和 Claude Code 的接入指南。

## 架构

详见 [docs/ARCHITECTURE_DIRECTION_CN.md](docs/ARCHITECTURE_DIRECTION_CN.md)，了解完整的架构决策记录（本地优先、网关兼容设计）。

## 构建

要求：Rust 1.70+

```bash
cargo build --release
cargo test
cargo clippy
```

## 开源协议

MIT — 详见 [LICENSE](./LICENSE)
