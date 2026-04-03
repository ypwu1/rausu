# GitHub Copilot Provider

> **English version:** [GITHUB_COPILOT_PROVIDER.md](GITHUB_COPILOT_PROVIDER.md)

## 概述

`github-copilot` provider 允许你通过 GitHub Copilot 订阅路由请求，无需 API Key。
Rausu 自动将你的 GitHub OAuth device-flow token 换取短期 Copilot API token，
然后将请求代理到 `https://api.githubcopilot.com`。

Claude 模型直接转发到 Copilot 原生 `/v1/messages` 端点（Anthropic Messages API
直传——无需协议转换）。其他模型使用 OpenAI 兼容的 `/chat/completions` 端点。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | ✅（流式 + 非流式） |
| `GET /v1/models` | ✅ 列出已配置的模型名 |
| `POST /v1/messages` | ✅ Claude：原生直传；GPT/其他：协议转换 |
| `POST /v1/responses` | ✅ Claude：Responses→Messages 桥接；GPT/其他：不支持 |

## 前提条件

你需要一个拥有 Copilot 活跃订阅的 GitHub 账号（Individual、Business 或 Enterprise）。

Token 从 `~/.config/github-copilot/hosts.json` 读取，该文件由 `gh auth login` 或
Copilot VS Code / JetBrains 扩展写入。

> **注意：** 环境变量 `GH_TOKEN` / `GITHUB_TOKEN` **不受支持**。它们通常包含 PAT
>（个人访问令牌），与 Copilot 内部 token 换取端点不兼容。仅支持来自 `hosts.json`
> 的 device-flow token（`ghu_...`）。

`hosts.json` 格式示例：

```json
{
  "github.com": {
    "user": "your-username",
    "oauth_token": "ghu_..."
  }
}
```

## 快速开始

### 1. 认证（如尚未认证）

```bash
# 使用 GitHub CLI（推荐）：
gh auth login --scopes read:user
```

### 2. 添加到 config.yaml

```yaml
models:
  # Claude 模型 — 原生 /v1/messages 直传
  - name: claude-sonnet-4-6
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.6

  # OpenAI 模型 — /chat/completions 协议转换
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o
```

### 3. 启动 Rausu

```bash
rausu --config config.yaml
```

### 4. 发送请求

```bash
# Claude 模型通过 /v1/messages（原生直传）
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: any-key" \
  -d '{
    "model": "claude-sonnet-4-6",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# OpenAI 模型通过 /v1/chat/completions
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "copilot-gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  aliases:                               # 可选；别名
    - <alias-1>
  providers:
    - provider: github-copilot
      model: <上游 Copilot 模型名>   # 必填
      credentials_path: /path/to/hosts.json  # 可选；默认：~/.config/github-copilot/hosts.json
```

### `credentials_path`

覆盖默认的 `~/.config/github-copilot/hosts.json` 路径。

## 支持的模型参考

### Claude 模型（通过 `/v1/messages` — 原生 Anthropic 直传）

| 配置 `name`（客户端发送） | 配置 `model`（Copilot ID） | 类别 | 特性 |
|---|---|---|---|
| `claude-opus-4-6` | `claude-opus-4.6` | 强力 | 视觉、思维、工具、流式 |
| `claude-sonnet-4-6` | `claude-sonnet-4.6` | 通用 | 视觉、思维、工具、流式 |
| `claude-sonnet-4-5` | `claude-sonnet-4.5` | 通用 | 视觉、思维、工具、流式 |
| `claude-opus-4-5` | `claude-opus-4.5` | 强力 | 视觉、思维、工具、流式 |
| `claude-sonnet-4` | `claude-sonnet-4` | 通用 | 视觉、思维、工具、流式 |
| `claude-haiku-4-5` | `claude-haiku-4.5` | 通用 | 视觉、思维、工具、流式 |

### OpenAI 模型（通过 `/chat/completions`）

| 配置 `name` | 配置 `model`（Copilot ID） | 类别 | 特性 |
|---|---|---|---|
| `gpt-5.4` | `gpt-5.4` | 强力 | 视觉、工具、流式、推理 |
| `gpt-5.2` | `gpt-5.2` | 通用 | 视觉、工具、流式、推理 |
| `gpt-5.1` | `gpt-5.1` | 通用 | 视觉、工具、流式、推理 |
| `gpt-5-mini` | `gpt-5-mini` | 轻量 | 视觉、工具、流式、推理 |
| `gpt-4.1` | `gpt-4.1` | 通用 | 视觉、工具、流式 |
| `gpt-4o` | `gpt-4o` | 通用 | 视觉、工具、流式 |

### Google 模型（通过 `/chat/completions`）

| 配置 `name` | 配置 `model`（Copilot ID） | 类别 | 特性 |
|---|---|---|---|
| `gemini-2.5-pro` | `gemini-2.5-pro` | 强力 | 视觉、思维、工具、流式 |
| `gemini-3.1-pro-preview` | `gemini-3.1-pro-preview` | 强力 | 视觉、思维、工具、流式（预览） |
| `gemini-3-flash-preview` | `gemini-3-flash-preview` | 轻量 | 视觉、思维、工具、流式（预览） |

### xAI 模型（通过 `/chat/completions`）

| 配置 `name` | 配置 `model`（Copilot ID） | 类别 |
|---|---|---|
| `grok-code-fast-1` | `grok-code-fast-1` | 轻量 |

### Codex 模型（仅 `/responses` — Rausu 尚不支持）

| Copilot ID | 类别 | 备注 |
|---|---|---|
| `gpt-5.2-codex` | 强力 | 仅 /responses |
| `gpt-5.3-codex` | 强力 | 仅 /responses |
| `gpt-5.4-mini` | 轻量 | 仅 /responses |

### 说明

- **Claude 模型使用原生 `/v1/messages` 直传** — 无协议转换，完整特性支持
  （思维、工具、视觉、流式）。
- **通过 `/v1/responses` 访问 Claude 模型时使用协议桥接** — Rausu 将 Responses API
  请求转换为 Messages API 格式，调用 Copilot 原生 `/v1/messages`，再将响应转换回
  Responses API 格式。这使得 Codex CLI 可以通过 Copilot 使用 Claude 模型，完整支持
  工具调用和流式传输。
- **Copilot 模型 ID 使用点号**（`claude-opus-4.6`），而 **Claude Code 使用连字符**
  （`claude-opus-4-6`）。在配置中使用 `aliases` 字段来兼容两种命名方式。
- 模型可用性取决于你的 Copilot 订阅级别。Copilot 可能对未在你计划中启用的模型返回
  `404` 或 `400`。

### 完整配置示例

```yaml
models:
  # ── Claude 模型（原生 /v1/messages 直传）─────────────────────────────────
  - name: claude-opus-4-6
    providers:
      - provider: github-copilot
        model: claude-opus-4.6

  - name: claude-sonnet-4-6
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.6

  - name: claude-sonnet-4-5
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.5

  - name: claude-opus-4-5
    providers:
      - provider: github-copilot
        model: claude-opus-4.5

  - name: claude-sonnet-4
    providers:
      - provider: github-copilot
        model: claude-sonnet-4

  - name: claude-haiku-4-5
    providers:
      - provider: github-copilot
        model: claude-haiku-4.5

  # ── OpenAI 模型（/chat/completions 协议转换）─────────────────────────────
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o

  - name: copilot-gpt-5.4
    providers:
      - provider: github-copilot
        model: gpt-5.4

  # ── Google 模型（/chat/completions）──────────────────────────────────────
  - name: copilot-gemini-2.5-pro
    providers:
      - provider: github-copilot
        model: gemini-2.5-pro
```

## 认证机制

Token 换取完全自动：

```
hosts.json (ghu_...)  →  GET /copilot_internal/v2/token  →  Copilot API token（TTL ~30 分钟）
```

Rausu 缓存 Copilot API token 并在过期前 5 分钟重新换取。Token **永远不会被记录到日志**。

## 与 Codex CLI 配合使用（通过协议桥接使用 Claude 模型）

Codex CLI 向 `/v1/responses` 发送请求。当配置的模型为 Claude 模型时，Rausu 自动进行
Responses API → Messages API 桥接，转发到 Copilot 原生 `/v1/messages` 端点。

```yaml
models:
  # Codex CLI 可通过 /v1/responses 使用此 Claude 模型
  - name: claude-sonnet-4-6
    providers:
      - provider: github-copilot
        model: claude-sonnet-4.6
    aliases:
      - claude-sonnet-4.6   # 匹配 Codex CLI 可能请求的名称
```

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="local-proxy"
codex --model claude-sonnet-4-6
```

Rausu 将 Responses API 请求转换为 Messages 格式，代理到 Copilot，再将响应转换回
Responses 格式——包括零缓冲的 SSE 流式传输。

## 已知限制

- **Responses API 仅支持 Claude 模型。** GPT 及其他非 Claude 模型没有 `/v1/responses`
  桥接；这些模型请使用 `/v1/chat/completions`。
- Copilot 的速率限制和模型可用性由 GitHub 控制 — Rausu 原样传递上游 HTTP 状态码。
- 工具/函数调用支持取决于上游 Copilot 模型。
- `base_url` 配置字段对此 provider 无效；端点由 token 换取响应决定（默认为
  `https://api.githubcopilot.com`）。
