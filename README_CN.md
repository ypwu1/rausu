<p align="center">
  <img src="assets/icon.jpg" width="160" alt="Rausu Icon" />
</p>

<h1 align="center">Rausu</h1>
<p align="center"><em>ラウス</em></p>

<p align="center">
  <a href="./README.md">English Version</a>
</p>

一个用 Rust 编写的高性能 LLM API 网关。单一二进制文件，零运行时依赖，协议感知的多 Provider 路由。

## 特性

- **OpenAI 兼容 API** — 适配任何 OpenAI SDK 客户端
- **多 Provider 支持** — 支持 OpenAI、Anthropic（API Key）、Claude 订阅（OAuth）、GitHub Copilot、ChatGPT 订阅（OAuth），以及任意 OpenAI 兼容服务（DeepSeek、Qwen、Ollama、GLM、Moonshot 等）
- **协议桥接** — OpenAI Responses API 与 Anthropic Messages API 双向转换；Codex CLI 可使用 Claude 模型或任意 OpenAI 兼容服务，Claude Code 可使用 GPT 模型或任意 OpenAI 兼容服务
- **真正的 SSE 流式传输** — 包括协议桥接路径在内的所有路径均实现零缓冲逐事件流式传输（首 token 延迟与直传路径一致）
- **流式传输** — 完整的 SSE 流式支持
- **单一二进制** — 零运行时依赖
- **YAML 配置** — 支持环境变量插值
- **API Key 认证** — 可选的静态 Key 认证，保护远程暴露的代理
- **结构化日志** — 带请求追踪的 JSON 日志

## 快速开始

### 方式一：从源码构建

```bash
cargo build --release

# 生成模板配置文件（写入 ~/.config/rausu/config.yaml）
./target/release/rausu init
# 编辑该文件后启动：
./target/release/rausu
```

或指定配置文件路径：

```bash
./target/release/rausu --config config.yaml
```

### 方式二：Docker（GHCR）

```bash
docker pull ghcr.io/ypwu1/rausu:latest
docker run -p 4000:4000 -v $(pwd)/config.yaml:/app/config.yaml ghcr.io/ypwu1/rausu:latest
```

多架构镜像（linux/amd64、linux/arm64）在每个版本标签时发布到 `ghcr.io/ypwu1/rausu`。可用标签：`latest`、`vX.Y.Z`、`vX.Y`。

### 方式三：Docker（从源码构建）

```bash
docker build -t rausu .
docker run -p 4000:4000 -v $(pwd)/config.yaml:/app/config.yaml rausu
```

## 配置

### 自动发现

运行 `rausu` 时若未指定 `--config`，会按优先级搜索以下位置：

1. `RAUSU_CONFIG` 环境变量
2. `./config.yaml`
3. `./rausu-config.yaml`
4. `${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml`
5. `${XDG_CONFIG_HOME:-~/.config}/rausu/rausu-config.yaml`
6. `~/.rausu/config.yaml`
7. `~/rausu-config.yaml`

若均未找到，将在 `${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml` 生成注释版模板，并提示你先编辑后再启动。

### `rausu init`

```bash
rausu init                    # 写入模板到 ~/.config/rausu/config.yaml
rausu init --path ./my.yaml   # 写入到自定义路径
rausu init --force            # 覆盖已有文件
```

### `rausu setup`

交互式配置编辑器 — 无需手写 YAML 即可创建或编辑配置：

```bash
rausu setup                    # 在默认位置创建或编辑
rausu setup --path ./my.yaml   # 指定文件
```

编辑器以模型为中心：先创建虚拟模型，然后为其添加带故障转移排序的供应商部署。支持添加、编辑、删除和重新排序模型及供应商。已有配置会自动加载。

保存前验证会检查错误（未知供应商、缺少字段、重复项）和警告（缺少凭据、不可达端点）。详见 [docs/SETUP_EDITOR_CN.md](docs/SETUP_EDITOR_CN.md)。

### `rausu check`

验证配置文件并测试提供商连通性：

```bash
rausu check                    # 使用自动发现的配置
rausu check --config my.yaml   # 使用指定配置文件
```

示例输出：

```
📋 Config: ~/.config/rausu/config.yaml
   Server: 127.0.0.1:4000
   Auth: static (2 keys)

📦 Models (3):
   ✓ gpt-5.4 → chatgpt-subscription
   ✓ claude-opus-4.6 → github-copilot
   ✓ deepseek-chat → openai (https://api.deepseek.com/v1)

🔌 Connectivity:
   ✓ chatgpt-subscription: token available (codex auth)
   ✓ github-copilot: hosts.json found (~/.config/github-copilot/hosts.json)
   ✓ openai (https://api.deepseek.com/v1): reachable (HTTP 200)
   ✗ openai (http://localhost:11434/v1): connection refused

✅ 3/4 providers OK
```

检查按四个步骤执行：加载配置、模型验证（必填字段、有效的 Provider 类型）、Provider 连通性（HTTP 可达性或凭证文件是否存在）、以及认证验证。

> **启动验证**：相同的验证逻辑在 `rausu` 以服务器模式启动时会自动运行。硬错误（未知供应商、缺少必填字段、重复名称）会阻止启动。警告（缺少凭据、不可达端点）会记录日志但允许服务器继续启动。

### 手动设置

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

### 认证

Rausu 支持可选的 API Key 认证，用于保护远程暴露的代理。提供两种模式：

- **`disabled`**（默认）— 无认证，所有请求直接转发。
- **`static`** — 请求必须携带有效的 `Authorization: Bearer <key>` 头，且 Key 在配置列表中。

```yaml
auth:
  mode: static
  keys:
    - name: "my-laptop"
      key: "rausu-sk-abc123"
    - name: "remote-client"
      key: "${RAUSU_API_KEY}"    # 支持环境变量插值
```

Key 值支持 `${ENV_VAR}` 插值。推荐的 Key 前缀约定为 `rausu-sk-`。

`/health` 端点始终免于认证。

如果完全省略 `auth` 配置段，认证默认为 `disabled`。

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

## 客户端 × 模型矩阵

所有客户端与模型的组合均支持，通过直传或协议桥接实现：

| 客户端 | 协议 | 目标 | 路径 |
|--------|------|------|------|
| Claude Code | `/v1/messages` | Claude（Copilot） | 直传 |
| Claude Code | `/v1/messages` | Claude（Anthropic） | 直传 |
| Claude Code | `/v1/messages` | GPT（ChatGPT 订阅） | Messages→Responses 桥接 |
| Claude Code | `/v1/messages` | 任意 OpenAI 兼容服务 | Messages→Responses→ChatCompletions |
| Codex CLI | `/v1/responses` | GPT（ChatGPT 订阅） | 直传 |
| Codex CLI | `/v1/responses` | GPT（Copilot） | 直传 |
| Codex CLI | `/v1/responses` | Claude（Copilot） | Responses→Messages 桥接 |
| Codex CLI | `/v1/responses` | 任意 OpenAI 兼容服务 | Responses→ChatCompletions 桥接 |

详细协议转换说明见 [docs/PROTOCOL_BRIDGE_PLAN_CN.md](docs/PROTOCOL_BRIDGE_PLAN_CN.md)。

## API 端点

| 端点 | 方法 | 描述 |
|------|------|------|
| `/health` | GET | 健康检查 |
| `/v1/models` | GET | 列出已配置的模型 |
| `/v1/chat/completions` | POST | 聊天补全 — 路由 + 格式转换 |
| `/v1/responses` | POST | OpenAI Responses API — 直传或 Responses→Messages 桥接 |
| `/v1/responses/compact` | POST | OpenAI Responses API 紧凑变体 — 透明直传 |
| `/v1/messages` | POST | Anthropic Messages API — 直传或 Messages→Responses 桥接 |

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
