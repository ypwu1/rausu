# GitHub Copilot 提供商

> **英文版本：** [GITHUB_COPILOT_PROVIDER.md](GITHUB_COPILOT_PROVIDER.md)

## 概览

`github-copilot` 提供商允许你通过 GitHub Copilot 订阅转发请求，无需 API 密钥。
Rausu 会自动将你的 GitHub OAuth 令牌换取短期有效的 Copilot API 令牌，并将兼容
OpenAI 格式的聊天补全请求代理到 `https://api.githubcopilot.com`。

## 支持矩阵

| 接口 | 支持状态 |
|---|---|
| `POST /v1/chat/completions` | ✅（流式 + 非流式） |
| `GET /v1/models` | ✅ 列出配置的模型名称 |
| `POST /v1/messages` | ❌ Copilot 不支持 Anthropic Messages API |
| `POST /v1/responses` | ❌ Copilot 不支持 OpenAI Responses API |

## 前置条件

你需要一个拥有有效 Copilot 订阅（个人版、商业版或企业版）的 GitHub 账号。

以下任意一项即可：

| 方式 | 来源 |
|---|---|
| **`GH_TOKEN` / `GITHUB_TOKEN` 环境变量** | 任意具有 `read:user` 权限的 GitHub OAuth 令牌 |
| **`~/.config/github-copilot/hosts.json`** | 由 `gh auth login` 或 VS Code Copilot 插件写入 |

`hosts.json` 文件格式如下：

```json
{
  "github.com": {
    "user": "your-username",
    "oauth_token": "ghu_..."
  }
}
```

## 快速开始

### 1. 认证（如尚未完成）

```bash
# 使用 GitHub CLI（推荐）：
gh auth login --scopes read:user

# 或直接设置环境变量：
export GH_TOKEN=ghp_yourPersonalAccessToken
```

### 2. 在 config.yaml 中添加配置

```yaml
models:
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o
        token_source: auto   # 默认值，可省略

  - name: copilot-claude-sonnet
    providers:
      - provider: github-copilot
        model: claude-3.5-sonnet
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
    "model": "copilot-gpt-4o",
    "messages": [{"role": "user", "content": "你好！"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: github-copilot
      model: <上游 Copilot 模型名>     # 必填
      token_source: auto               # 可选，默认：auto
      credentials_path: /path/to/hosts.json  # 可选，默认：~/.config/github-copilot/hosts.json
```

### `token_source` 取值

| 值 | 行为 |
|---|---|
| `auto`（默认） | 先尝试 `GH_TOKEN` / `GITHUB_TOKEN` 环境变量，再尝试 `hosts.json` |
| `env` | 仅使用 `GH_TOKEN` 或 `GITHUB_TOKEN` 环境变量 |
| `hosts_file` | 仅使用 `hosts.json`（路径由 `credentials_path` 指定，或使用默认路径） |

### `credentials_path`

覆盖默认的 `~/.config/github-copilot/hosts.json` 路径。仅在 `token_source` 为
`hosts_file` 或 `auto`（回退）时使用。

## 上游模型名称

模型可用性取决于你的 Copilot 订阅套餐。以下为 2025 年 Q1 确认可用的模型 ID：

| 模型 ID | 描述 |
|---|---|
| `gpt-4o` | OpenAI GPT-4o（经 Copilot） |
| `gpt-4o-mini` | OpenAI GPT-4o Mini |
| `claude-3.5-sonnet` | Anthropic Claude 3.5 Sonnet（经 Copilot） |
| `o1-mini` | OpenAI o1-mini 推理模型 |
| `o3-mini` | OpenAI o3-mini（按订阅套餐决定） |

若你的套餐未启用某模型，Copilot 会返回 `404` 或 `400`。

## 认证内部机制

令牌交换完全自动化：

```
GitHub OAuth 令牌  →  GET /copilot_internal/v2/token  →  Copilot API 令牌（有效期约 30 分钟）
```

Rausu 会缓存 Copilot API 令牌，并在到期前 5 分钟自动重新交换。
令牌**不会被记录到日志中**。

## 已知限制

- **不支持 Messages API 直通**（`/v1/messages`）。如需 Anthropic 原生路由，请使用
  `provider: anthropic` 或 `provider: claude-subscription`。
- **不支持 Responses API 直通**（`/v1/responses`）。
- Copilot 的速率限制和模型可用性由 GitHub 控制，Rausu 会原样传递上游 HTTP 状态码。
- 工具/函数调用支持取决于上游 Copilot 模型。
- 该提供商忽略配置中的 `base_url` 字段；接口端点由令牌交换响应决定（默认为
  `https://api.githubcopilot.com`）。
