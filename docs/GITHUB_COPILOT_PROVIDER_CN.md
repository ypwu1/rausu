# GitHub Copilot Provider

> **English version:** [GITHUB_COPILOT_PROVIDER.md](GITHUB_COPILOT_PROVIDER.md)

## 概述

`github-copilot` provider 允许你通过 GitHub Copilot 订阅路由请求，无需 API Key。
Rausu 自动将你的 GitHub OAuth device-flow token 换取短期 Copilot API token，
然后将 OpenAI 兼容的 chat completions 请求代理到 `https://api.githubcopilot.com`。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | ✅（流式 + 非流式） |
| `GET /v1/models` | ✅ 列出已配置的模型名 |
| `POST /v1/messages` | ❌ Copilot 不暴露 Anthropic Messages API |
| `POST /v1/responses` | ❌ Copilot 不暴露 OpenAI Responses API |

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
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o

  - name: copilot-claude-sonnet
    providers:
      - provider: github-copilot
        model: claude-3.5-sonnet
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
    "model": "copilot-gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: github-copilot
      model: <上游 Copilot 模型名>   # 必填
      credentials_path: /path/to/hosts.json  # 可选；默认：~/.config/github-copilot/hosts.json
```

### `credentials_path`

覆盖默认的 `~/.config/github-copilot/hosts.json` 路径。

## 上游模型名

模型可用性取决于你的 Copilot 订阅级别。2025 Q1 确认可用的模型名：

| 模型 ID | 说明 |
|---|---|
| `gpt-4o` | OpenAI GPT-4o（通过 Copilot） |
| `gpt-4o-mini` | OpenAI GPT-4o Mini |
| `claude-3.5-sonnet` | Anthropic Claude 3.5 Sonnet（通过 Copilot） |
| `o1-mini` | OpenAI o1-mini 推理模型 |
| `o3-mini` | OpenAI o3-mini（部分计划可用） |

Copilot 可能对未在你计划中启用的模型返回 `404` 或 `400`。

## 认证机制

Token 换取完全自动：

```
hosts.json (ghu_...)  →  GET /copilot_internal/v2/token  →  Copilot API token（TTL ~30 分钟）
```

Rausu 缓存 Copilot API token 并在过期前 5 分钟重新换取。Token **永远不会被记录到日志**。

## 已知限制

- **不支持 Messages API 直传**（`/v1/messages`）。Anthropic 原生路由请使用 `provider: anthropic` 或 `provider: claude-subscription`。
- **不支持 Responses API 直传**（`/v1/responses`）。
- Copilot 的速率限制和模型可用性由 GitHub 控制 — Rausu 原样传递上游 HTTP 状态码。
- 工具/函数调用支持取决于上游 Copilot 模型。
- `base_url` 配置字段对此 provider 无效；端点由 token 换取响应决定（默认为 `https://api.githubcopilot.com`）。
