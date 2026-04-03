# ChatGPT Subscription Provider

> **English version:** [CHATGPT_SUBSCRIPTION_PROVIDER.md](CHATGPT_SUBSCRIPTION_PROVIDER.md)

## 概述

`chatgpt-subscription` provider 允许你通过 ChatGPT Plus、Pro 或 Max 订阅路由请求，无需 API Key。
Rausu 从 `~/.config/rausu/chatgpt-auth.json` 读取 OAuth access token，
并将 OpenAI Chat Completions 请求桥接到 ChatGPT Responses API（`https://chatgpt.com/backend-api/codex/responses`）。
Responses API 也可通过 `/v1/responses` 直接访问。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | ✅（流式 + 非流式，桥接至 Responses API） |
| `POST /v1/responses` | ✅ 原生 Responses API 直传 |
| `GET /v1/models` | ✅ 列出已配置的模型名 |
| `POST /v1/messages` | ✅ GPT 模型：Messages→Responses 桥接；Claude 模型：不支持 |

## 前提条件

你需要一个 ChatGPT Plus、Pro 或 Max 订阅。Codex 模型（`gpt-5.3-codex` 等）需要包含 Codex 访问权限的套餐。

OAuth token 从 `~/.config/rausu/chatgpt-auth.json` 读取，该文件需手动创建（参见下方[认证](#认证)部分）。

## 认证

### 凭证文件

创建 `~/.config/rausu/chatgpt-auth.json`，填入你的 ChatGPT OAuth token：

```json
{
  "access_token": "eyJ...",
  "refresh_token": "...",
  "expires_at": 1900000000000,
  "account_id": "user-..."
}
```

字段说明：

| 字段 | 是否必填 | 说明 |
|---|---|---|
| `access_token` | 是 | 来自 ChatGPT OAuth 流程的 Bearer access token |
| `refresh_token` | 推荐 | 用于到期前自动刷新 token |
| `expires_at` | 推荐 | 过期时间（Unix 毫秒时间戳） |
| `account_id` | 可选 | ChatGPT 账号 ID；省略时自动从 JWT 提取 |

如果省略 `account_id`，Rausu 会自动解码 JWT 载荷并查找 `["https://api.openai.com/auth"]["chatgpt_account_id"]`。

当存在 `refresh_token` 时，Rausu 会在 token 距过期不足 5 分钟时自动刷新。

### 获取 Token

Access token 可以从已登录的 ChatGPT 浏览器会话中提取（在 `chatgpt.com` 的浏览器开发者工具中查看 `Authorization` 请求头），
或从 Codex CLI 认证流程的 `~/.codex/auth.json` 中获取。
将相关字段复制到 `~/.config/rausu/chatgpt-auth.json` 即可。

### 环境变量

也可以通过环境变量提供 token（无需凭证文件）：

```bash
export CHATGPT_ACCESS_TOKEN="eyJ..."
export CHATGPT_REFRESH_TOKEN="..."          # 可选：启用自动刷新
export CHATGPT_ACCOUNT_ID="user-..."        # 可选：跳过 JWT 解码
```

然后在 Rausu 配置中设置 `token_source: env`。

### `auto` 的 token 来源解析顺序

1. `CHATGPT_ACCESS_TOKEN` 环境变量（以及可选的 `CHATGPT_REFRESH_TOKEN` / `CHATGPT_ACCOUNT_ID`）
2. `~/.config/rausu/chatgpt-auth.json`

## 快速开始

### 1. 创建 `~/.config/rausu/chatgpt-auth.json`

参见上方[认证](#认证)部分。

### 2. 添加到 config.yaml

```yaml
models:
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto

  - name: gpt-5-codex
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.3-codex
        token_source: auto
```

### 3. 启动 Rausu

```bash
rausu --config config.yaml
```

### 4. 发送请求

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gpt-5",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 与 Codex CLI 配合使用

`chatgpt-subscription` provider 专为 [Codex CLI](https://github.com/openai/codex) 设计，后者使用 OpenAI 兼容 API。

### 逐步操作

**1. 创建 `config.yaml`**

```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: info
  format: pretty

models:
  - name: gpt-5.3-codex
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.3-codex
        token_source: auto

  - name: gpt-5.4
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto
```

> **提示：** 将上游模型 ID 直接用作虚拟模型名，这样 Codex CLI 无需 `--model` 参数即可使用。

**2. 启动 Rausu**

```bash
./rausu --config config.yaml
```

**3. 将 Codex CLI 指向 Rausu**

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu 会忽略此值，但 Codex 需要设置非空 key
codex --model gpt-5.3-codex
```

### 直接使用 Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gpt-5.3-codex",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: chatgpt-subscription
      model: <上游 ChatGPT 模型 ID>             # 必填
      token_source: auto                        # 必填：auto | env | credentials_file
      credentials_path: /path/to/chatgpt-auth.json  # 可选；默认：~/.config/rausu/chatgpt-auth.json
```

### `token_source`

| 值 | 行为 |
|---|---|
| `auto` | 先尝试环境变量（`CHATGPT_ACCESS_TOKEN`），再使用凭证文件 |
| `env` | 仅使用 `CHATGPT_ACCESS_TOKEN` 环境变量 |
| `credentials_file` | 仅使用凭证文件 |

### `credentials_path`

覆盖默认的 `~/.config/rausu/chatgpt-auth.json` 路径。

## 上游模型名

模型可用性取决于你的订阅计划。2026 Q1 确认可用的模型 ID：

| 模型 ID | 说明 |
|---|---|
| `gpt-5.4` | GPT-5.4（旗舰版） |
| `gpt-5.4-pro` | GPT-5.4 Pro |
| `gpt-5.3-codex` | GPT-5.3 Codex（供 Codex CLI 使用） |
| `gpt-5.3-codex-spark` | GPT-5.3 Codex Spark（轻量版） |
| `gpt-5.3-instant` | GPT-5.3 Instant |
| `gpt-5.3-chat-latest` | GPT-5.3 Chat（最新版） |

如果你的计划不包含相应模型，ChatGPT 会返回错误。

## 请求桥接机制

Chat Completions 请求被转换为 ChatGPT Responses API 格式：

| Chat Completions 字段 | Responses API 字段 |
|---|---|
| `messages[role=system]` | `instructions` |
| `messages`（user/assistant） | `input` 数组 |
| `model` | `model` |
| `stream` | 上游始终使用流式，非流式调用者会收到聚合结果 |

Rausu 始终以流式方式请求上游，并为非流式调用者聚合响应块。发送到 ChatGPT 端点的 header：

- `Authorization: Bearer <access_token>`
- `chatgpt-account-id: <account_id>`（有时可用）
- `OpenAI-Beta: responses=experimental`
- `originator: pi`

Token **永远不会被记录到日志**。

## Docker 部署

```bash
docker run \
  -v ~/.config/rausu/chatgpt-auth.json:/app/chatgpt-auth.json \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

在 `config.yaml` 中添加 `credentials_path: /app/chatgpt-auth.json`，或使用 `CHATGPT_ACCESS_TOKEN` 环境变量方式。

## 与 Claude Code 配合使用（通过协议桥接使用 GPT 模型）

Claude Code 向 `/v1/messages` 发送请求。当配置的模型为 GPT 模型时，Rausu 自动进行
Messages API → Responses API 桥接，转发到 ChatGPT Responses 端点。

```yaml
models:
  # Claude Code 可通过 /v1/messages 使用此 GPT 模型
  - name: gpt-5.4
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: auto
```

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="local-proxy"
# 在 Claude Code 设置中选择 gpt-5.4 作为模型
```

Rausu 将 Messages API 请求转换为 Responses 格式，代理到 ChatGPT，再将响应转换回
Messages 格式——包括零缓冲的 SSE 流式传输。完整支持工具调用（`tool_use` ↔
`function_call`）。

## 已知限制

- **Messages API 仅支持 GPT 模型。** Claude 模型无法通过 `chatgpt-subscription` 使用；
  Claude 请使用 `provider: anthropic`、`provider: claude-subscription` 或
  `provider: github-copilot`。
- **订阅速率限制**和模型可用性由 OpenAI 控制 — Rausu 原样传递上游 HTTP 状态码。
- **Token 获取需手动操作。** 与 GitHub Copilot 不同，此 provider 没有自动的 device-flow 登录流程，需自行获取并放置 token。
- `base_url` 配置字段对此 provider 无效；端点始终为 `https://chatgpt.com/backend-api/codex/responses`。
