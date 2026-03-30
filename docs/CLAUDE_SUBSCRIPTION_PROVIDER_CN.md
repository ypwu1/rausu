# Claude Subscription Provider

> **English version:** [CLAUDE_SUBSCRIPTION_PROVIDER.md](CLAUDE_SUBSCRIPTION_PROVIDER.md)

## 概述

`claude-subscription` provider 允许你通过 Claude Pro 或 Max 订阅路由请求，无需 API Key。
Rausu 从 `~/.claude/.credentials.json`（由 Claude CLI 写入）读取 OAuth token，
并使用订阅访问所需的 Claude Code 身份 header 将请求转发到 `https://api.anthropic.com/v1/messages`。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/messages` | ✅（流式 + 非流式） |
| `POST /v1/chat/completions` | ❌ API Key 方式请使用 `provider: anthropic` |
| `GET /v1/models` | ✅ 列出已配置的模型名 |
| `POST /v1/responses` | ❌ Anthropic Messages API 不支持此端点 |

## 前提条件

你需要一个 Claude Pro 或 Max 订阅，以及已安装并完成认证的 Claude CLI。

OAuth token 从 `~/.claude/.credentials.json` 读取，该文件在你通过 `claude`（Claude CLI）登录时写入。

```json
{
  "claudeAiOauth": {
    "accessToken": "...",
    "refreshToken": "...",
    "expiresAt": 1743000000000
  }
}
```

当 token 距过期时间不足 5 分钟时，Rausu 会使用 refresh token 自动刷新 access token。

## 认证

### 方式 A：凭证文件（推荐）

使用 Claude CLI 登录一次：

```bash
claude
# 按照浏览器 OAuth 流程完成认证
```

这会写入 `~/.claude/.credentials.json`。在 Rausu 配置中设置 `token_source: auto` 或 `token_source: credentials_file`。

### 方式 B：环境变量

设置静态 access token（不自动刷新）：

```bash
export CLAUDE_OAUTH_TOKEN="your-oauth-access-token"
```

然后在 Rausu 配置中设置 `token_source: env`。适用于在外部管理 token 轮换的 CI 环境。

### `auto` 的 token 来源解析顺序

1. `CLAUDE_OAUTH_TOKEN` 环境变量（不刷新）
2. `~/.claude/.credentials.json`（自动刷新）

## 快速开始

### 1. 使用 Claude CLI 认证（如尚未认证）

```bash
claude
```

### 2. 添加到 config.yaml

```yaml
models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto
```

> **提示：** 将上游模型 ID 直接用作虚拟模型名，这样 Claude Code 无需额外配置即可使用。

### 3. 启动 Rausu

```bash
rausu --config config.yaml
```

### 4. 使用 curl 测试

```bash
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 与 Claude Code CLI 配合使用

这是主要使用场景：将 Claude Code 指向 Rausu，通过 Claude 订阅而非直接的 API Key 进行访问。

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

**2. 启动 Rausu**

```bash
./rausu --config config.yaml
```

**3. 将 Claude Code 指向 Rausu**

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="fake-key"   # Rausu 会忽略此值，但 Claude Code 需要设置它
claude -p "Hello via subscription"
```

Claude Code 发送请求到 `/v1/messages`，正好是此 provider 处理的端点。

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: claude-subscription
      model: <上游 Claude 模型 ID>              # 必填
      token_source: auto                        # 必填：auto | env | credentials_file
      credentials_path: /path/to/credentials.json  # 可选；默认：~/.claude/.credentials.json
```

### `token_source`

| 值 | 行为 |
|---|---|
| `auto` | 先尝试 `CLAUDE_OAUTH_TOKEN` 环境变量，再使用 `~/.claude/.credentials.json` |
| `env` | 仅使用 `CLAUDE_OAUTH_TOKEN`（不刷新） |
| `credentials_file` | 仅使用凭证文件（自动刷新） |

### `credentials_path`

覆盖默认的 `~/.claude/.credentials.json` 路径。适用于以不同用户身份运行 Rausu 或在容器中使用的场景。

## 上游模型名

可用性取决于你的订阅级别（Pro 或 Max）。2026 Q1 确认可用的模型 ID：

| 模型 ID | 说明 |
|---|---|
| `claude-opus-4-6` | Claude Opus 4.6（能力最强） |
| `claude-sonnet-4-6` | Claude Sonnet 4.6 |
| `claude-opus-4-20250514` | Claude Opus 4 |
| `claude-sonnet-4-20250514` | Claude Sonnet 4 |
| `claude-haiku-4-20250514` | Claude Haiku 4（最快） |
| `claude-sonnet-4-5-20251001` | Claude Sonnet 4.5 |
| `claude-haiku-3-20240307` | Claude 3 Haiku（旧版） |

最新模型 ID 请参阅 [Anthropic 模型文档](https://docs.anthropic.com/en/docs/about-claude/models)。Rausu 会将你配置的模型名直接传递给 API。

## 认证机制

Rausu 使用 Claude Code OAuth 身份访问订阅端点：

- **Beta headers：** `claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14`
- **User-Agent：** `claude-cli/2.1.75`
- **系统提示前缀：** 自动在系统提示前添加 `You are Claude Code, Anthropic's official CLI for Claude.`

Token 刷新使用 `https://claude.ai/oauth/claude-code-client-metadata` 的 OAuth 元数据发现端点动态定位 token 端点。Token **永远不会被记录到日志**。

## Docker 部署

```bash
docker run \
  -v ~/.claude/.credentials.json:/app/credentials.json \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

在 `config.yaml` 中添加 `credentials_path: /app/credentials.json`，或使用 `CLAUDE_OAUTH_TOKEN` 环境变量方式。

## 已知限制

- **不支持 `/v1/chat/completions`。** OpenAI 兼容格式请使用带 API Key 的 `provider: anthropic`。
- **不支持 `/v1/responses`。**
- 订阅速率限制和模型可用性由 Anthropic 控制 — Rausu 原样传递上游 HTTP 状态码。
- Token 刷新需要凭证文件中有效的 refresh token。如果仅设置了 `CLAUDE_OAUTH_TOKEN`（env 来源），则不会自动刷新，需手动轮换 token。
- 系统提示前缀（`You are Claude Code...`）始终会被注入，这是订阅端点接受请求的必要条件。
