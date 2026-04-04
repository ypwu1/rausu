# 本地代理使用指南

> [English Version](LOCAL_PROXY_USAGE.md)

本指南说明如何将 Rausu 作为**本地代理**运行，供 Codex CLI 和 Claude Code 等 AI 编程工具使用。内容涵盖配置示例、伪 Key 行为、已支持端点及当前限制。

---

## 概览

Rausu 的**本地运行时**是一个运行在本机的单用户 HTTP 代理，主要用于：

- 注入真实的上游凭证（OAuth Token、API Key），让本地客户端无需直接持有这些凭证。
- 提供统一的 OpenAI 兼容 API，供多个工具共享。
- 对 Responses API 和 Messages API 流量实现透明直传。
- 在客户端与 Provider 之间进行协议桥接——Codex CLI 可使用 Claude 模型，Claude Code 可使用 GPT 模型。

```
  Codex CLI / Claude Code / 任意 OpenAI 客户端
         │  伪 API Key 或占位值
         ▼
  http://localhost:4000
         │  Rausu 注入真实上游凭证
         │  + 按需进行协议桥接
         ▼
  OpenAI / Anthropic / Claude 订阅 / ChatGPT 订阅 / GitHub Copilot
```

---

## 在本地启动 Rausu

```bash
# 构建（仅首次需要）
cargo build --release

# 使用你的配置运行
./target/release/rausu --config config.yaml
```

开发阶段也可使用 `cargo run`：

```bash
cargo run -- --config config.yaml
```

Rausu 默认监听 `http://localhost:4000`（可通过 `server.host` / `server.port` 修改）。

---

## 配置示例

以示例配置为起点：

```bash
cp config.example.yaml config.yaml
```

### OpenAI API Key

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty   # 本地开发用 pretty；生产环境用 json

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  - name: o3
    providers:
      - provider: openai
        model: o3
        api_key: "${OPENAI_API_KEY}"
```

启动前设置环境变量：

```bash
export OPENAI_API_KEY="sk-..."
./target/release/rausu --config config.yaml
```

### ChatGPT 订阅（Plus / Pro / Max）

无需 API Key——使用你的 ChatGPT OAuth 会话。

```yaml
models:
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto   # 先尝试 env，再尝试 ~/.config/rausu/chatgpt-auth.json

  - name: gpt-5-codex
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.3-codex
        token_source: auto
```

**Token 来源**（按优先级顺序）：

1. `env` — 设置 `CHATGPT_ACCESS_TOKEN=<token>`（可选配置 `CHATGPT_REFRESH_TOKEN`、`CHATGPT_ACCOUNT_ID`）
2. `credentials_file` — 读取 `~/.config/rausu/chatgpt-auth.json`，支持自动刷新 Token
3. `auto`（默认）— 先尝试 `env`，再尝试 `credentials_file`

凭证文件格式（`~/.config/rausu/chatgpt-auth.json`）：

```json
{
  "access_token": "eyJ...",
  "refresh_token": "...",
  "expires_at": 1900000000000,
  "account_id": "acc_..."
}
```

支持的模型：`gpt-5.4`、`gpt-5.4-pro`、`gpt-5.3-codex`、`gpt-5.3-codex-spark`、`gpt-5.3-instant`、`gpt-5.3-chat-latest`

### Claude 订阅（Pro / Max）

无需 API Key——使用 Claude CLI 管理的 Claude OAuth 会话。

```yaml
models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto   # 先尝试 env，再尝试 ~/.claude/.credentials.json

  - name: claude-opus-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-opus-4-20250514
        token_source: credentials_file   # 明确指定：从 Claude CLI 凭证文件读取
```

**Token 来源**（按优先级顺序）：

1. `env` — 设置 `CLAUDE_OAUTH_TOKEN=<access_token>`（静态，不自动刷新）
2. `credentials_file` — 读取 Claude CLI 写入的 `~/.claude/.credentials.json`，支持自动刷新 Token
3. `auto`（默认）— 先尝试 `env`，再尝试 `credentials_file`

如果你已通过 Claude Code 或 Claude CLI 登录，`credentials_file` 来源可自动生效，无需额外配置。

### OpenAI 兼容 Provider（DeepSeek、Qwen、Ollama 等）

任何实现 OpenAI 兼容 Chat Completions API 的服务，通过 `provider: openai` + `base_url` 即可接入。Codex CLI 可直接使用这些服务——Rausu 自动将 Responses API 桥接为 Chat Completions 格式（Phase 3）。

```yaml
models:
  # DeepSeek
  - name: deepseek-chat
    providers:
      - provider: openai
        model: deepseek-chat
        base_url: https://api.deepseek.com/v1
        api_key: "${DEEPSEEK_API_KEY}"

  # Qwen（阿里云 DashScope）
  - name: qwen-max
    providers:
      - provider: openai
        model: qwen-max
        base_url: https://dashscope.aliyuncs.com/compatible-mode/v1
        api_key: "${DASHSCOPE_API_KEY}"

  # Ollama（本地，无需 API Key）
  - name: llama3
    providers:
      - provider: openai
        model: llama3
        base_url: http://localhost:11434/v1
        api_key: ollama
```

```bash
# 使用 Codex CLI 访问任意上述模型
export OPENAI_BASE_URL=http://localhost:4000
export OPENAI_API_KEY=local-proxy
codex --model deepseek-chat
codex --model qwen-max
codex --model llama3
```

完整的支持 Provider 列表及其 `base_url` 值见 [OPENAI_PROVIDER_CN.md](OPENAI_PROVIDER_CN.md)。

### 混合模型配置（全部 Provider）

单一 Rausu 配置文件可同时暴露多个虚拟模型名称，各自对应不同的 Provider：

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty

models:
  # OpenAI — 使用 API Key
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  # ChatGPT 订阅 — 无需 API Key
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto

  # Anthropic — 使用 API Key
  - name: claude-sonnet
    providers:
      - provider: anthropic
        model: claude-sonnet-4-20250514
        api_key: "${ANTHROPIC_API_KEY}"

  # Claude 订阅 — 模型名与 Claude Code 期望的名称一致
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto

  - name: claude-opus-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-opus-4-20250514
        token_source: auto
```

> **提示：** 对于 Claude Code，将虚拟模型名称设置为与真实模型 ID 一致（如 `claude-sonnet-4-20250514`），可以让 Claude Code 的模型选择器无缝工作，无需任何修改。

---

## 伪 Key / 本地认证行为

**Rausu 会忽略本地客户端传入的 API Key。** 本地工具（Codex CLI、Claude Code、curl、SDK 等）通常要求 API Key 字段不为空，但在本地代理模式下，你设置什么值并不重要——Rausu 不会校验它。

Rausu 会**注入自身配置中加载的真实上游凭证**（通过环境变量获取的 API Key，或从凭证文件/环境变量获取的 OAuth Token）。

这意味着：
- 将客户端指向 Rausu 时，可以将 `OPENAI_API_KEY`、`ANTHROPIC_API_KEY` 设为 `fake` 或任意占位值。
- 凭证不会以明文形式通过客户端配置泄露。
- 订阅认证（Claude OAuth、ChatGPT OAuth）对客户端完全透明，客户端无需了解 OAuth Token 的任何细节。

---

## 连接 Codex CLI

Codex CLI 主要使用 OpenAI Responses API（`/v1/responses`）。Rausu 对该端点实现了透明直传。

**第一步 — 配置 Rausu**，声明 Codex 将请求的模型：

```yaml
models:
  - name: gpt-5.3-codex
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.3-codex
        token_source: auto
```

或使用 OpenAI API Key：

```yaml
models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"
```

**第二步 — 启动 Rausu：**

```bash
./target/release/rausu --config config.yaml
```

**第三步 — 将 Codex CLI 指向 Rausu：**

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="local-proxy"   # 任意非空值
codex --model gpt-5.3-codex
```

Codex 将把请求发送到 `http://localhost:4000/v1/responses`，Rausu 会携带真实凭证将请求转发到上游。

---

## Codex CLI 使用 OpenAI 兼容服务（通过 Phase 3 桥接）

Codex CLI 可使用 DeepSeek、Qwen、Ollama 及任意 OpenAI 兼容服务。Rausu 自动将 Responses API 请求桥接为 Chat Completions 格式。

**第一步 — 配置 Rausu**，声明 OpenAI 兼容服务：

```yaml
models:
  - name: deepseek-chat
    providers:
      - provider: openai
        model: deepseek-chat
        base_url: https://api.deepseek.com/v1
        api_key: "${DEEPSEEK_API_KEY}"

  - name: llama3
    providers:
      - provider: openai
        model: llama3
        base_url: http://localhost:11434/v1
        api_key: ollama
```

**第二步 — 启动 Rausu：**

```bash
./target/release/rausu --config config.yaml
```

**第三步 — 将 Codex CLI 指向 Rausu：**

```bash
export OPENAI_BASE_URL=http://localhost:4000
export OPENAI_API_KEY=local-proxy
codex --model deepseek-chat
```

Rausu 接收来自 Codex CLI 的 `/v1/responses` 请求，转换为 Chat Completions 格式，转发到上游服务，再将响应转换回 Responses 格式——全程透明。

---

## Codex CLI 使用 Claude 模型（通过协议桥接）

Codex CLI 可通过 GitHub Copilot provider 使用 Claude 模型。Rausu 自动将 Responses API 请求桥接为 Anthropic Messages API 格式。

**第一步 — 配置 Rausu：**

```yaml
models:
  - name: claude-sonnet-4-6
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.6
    aliases:
      - claude-sonnet-4.6
```

**第二步 — 启动 Rausu：**

```bash
./target/release/rausu --config config.yaml
```

**第三步 — 将 Codex CLI 指向 Rausu：**

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="local-proxy"
codex --model claude-sonnet-4-6
```

Rausu 将 `/v1/responses` 请求桥接到 Copilot 原生 `/v1/messages` 端点，转换响应格式，并以零缓冲方式流式传输事件。

---

## 连接 Claude Code

Claude Code 主要使用 Anthropic Messages API（`/v1/messages`）。Rausu 对该端点实现了透明直传。

**第一步 — 配置 Rausu**，声明与 Claude Code 期望一致的模型名称：

```yaml
models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto   # 读取 ~/.claude/.credentials.json

  - name: claude-opus-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-opus-4-20250514
        token_source: auto
```

**第二步 — 启动 Rausu：**

```bash
./target/release/rausu --config config.yaml
```

**第三步 — 将 Claude Code 指向 Rausu：**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="local-proxy"   # 任意非空值
claude
```

Claude Code 将把请求发送到 `http://localhost:4000/v1/messages`，Rausu 会携带真实 OAuth Token 将请求转发到 Claude 订阅端点。

> **注意：** `ANTHROPIC_BASE_URL` 应为不含 `/v1` 的基础地址——Claude Code 会自行追加 `/v1/messages`。

---

## Claude Code 使用 GPT 模型（通过协议桥接）

Claude Code 可通过 ChatGPT 订阅 provider 使用 GPT 模型。Rausu 自动将 Messages API 请求桥接为 Responses API 格式。

**第一步 — 配置 Rausu：**

```yaml
models:
  - name: gpt-5.4
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto
```

**第二步 — 启动 Rausu：**

```bash
./target/release/rausu --config config.yaml
```

**第三步 — 将 Claude Code 指向 Rausu：**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="local-proxy"
claude
```

在 Claude Code 的模型选择器中选择 `gpt-5.4`。Rausu 将 `/v1/messages` 请求桥接到 ChatGPT 原生 Responses API，转换响应格式，并以零缓冲方式流式传输事件。完整支持工具调用。

---

## 已支持的端点

| 方法 | 端点 | 描述 |
|------|------|------|
| `GET` | `/health` | 健康检查 |
| `GET` | `/v1/models` | 列出已配置的模型 |
| `POST` | `/v1/chat/completions` | OpenAI Chat Completions — 路由 + 格式转换 |
| `POST` | `/v1/responses` | OpenAI Responses API — 透明直传（Codex CLI） |
| `POST` | `/v1/responses/compact` | OpenAI Responses API 紧凑变体 — 透明直传 |
| `POST` | `/v1/messages` | Anthropic Messages API — 透明直传（Claude Code） |

**直传 vs. 协议桥接：**
- `/v1/responses` — 当上游原生支持 Responses API 时（OpenAI、ChatGPT 订阅、Copilot GPT 模型）原样转发。对于通过 Copilot 使用的 Claude 模型，Rausu 自动进行 Responses→Messages 桥接。对于通过 `base_url` 接入的 OpenAI 兼容服务，Rausu 自动进行 Responses→ChatCompletions 桥接（Phase 3）。
- `/v1/messages` — 对 Claude provider 原样转发。对于通过 ChatGPT 订阅使用的 GPT 模型，Rausu 自动进行 Messages→Responses 桥接。对于 OpenAI 兼容服务，Rausu 自动串联 Messages→Responses→ChatCompletions 桥接。
- `/v1/chat/completions` — 经过 Provider 抽象层路由，Rausu 根据需要进行请求/响应格式归一化处理。

---

## 当前限制

以下是本地运行时当前阶段的已知限制，均为有意为之，将在后续阶段解决。

| 限制 | 说明 |
|------|------|
| **不支持自动 base_url 接管** | 客户端必须手动设置 `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL`。通过 `/etc/hosts` 或系统代理设置实现透明全局拦截尚未实现。 |
| **仅支持单用户** | 无虚拟 API Key、无按用户路由、无费用追踪。一个配置文件服务本地单用户会话。 |
| **不支持路由或故障转移** | 每个虚拟模型映射到单一 Provider 部署。多 Provider 故障转移和负载均衡尚未实现。 |
| **无管理面板** | 配置仅通过文件进行。 |
| **无速率限制或预算管理** | 请求直接转发，无本地配额限制。 |
| **Responses API：Provider 支持因情况而异** | `/v1/responses` 对 OpenAI 和 ChatGPT 订阅原生直传。对通过 Copilot 使用的 Claude 模型使用协议桥接。无 Responses API 支持且无桥接的 Provider 将返回不支持错误。 |

---

## 使用技巧

- 本地开发时，在 `logging` 中使用 `format: pretty` 以获得更易读的日志输出。
- 设置 `level: debug` 可查看请求/响应详情。
- 运行 `curl http://localhost:4000/health` 验证 Rausu 是否已启动。
- 运行 `curl http://localhost:4000/v1/models` 验证你的模型名称是否已注册。
