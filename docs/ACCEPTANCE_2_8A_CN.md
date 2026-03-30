# 验收报告 — 阶段 2.8A：GitHub Copilot 提供商

> **英文版本：** [ACCEPTANCE_2_8A.md](ACCEPTANCE_2_8A.md)

**状态：部分完成 — 实现已完成，本地端到端因凭证问题受阻**

## 摘要

GitHub Copilot 提供商已实现并集成到 Rausu 中。所有单元测试通过（共 83 个）。
本地端到端验收部分完成：服务器启动及模型列表功能验证成功；由于本地凭证不兼容
`/copilot_internal/v2/token` 接口（详见§阻塞项），无法验证上游 Copilot API 调用。

---

## 支持的接口

| 路由 | 支持状态 |
|---|---|
| `POST /v1/chat/completions` | ✅（流式 + 非流式） |
| `GET /v1/models` | ✅ 列出已配置的模型名称 |
| `POST /v1/messages` | ❌ Copilot 不支持 Anthropic Messages API |
| `POST /v1/responses` | ❌ Copilot 不支持 OpenAI Responses API |

---

## 已实现的认证模式

两步令牌交换：

1. 从以下来源加载 **GitHub OAuth 令牌**（`ghu_...` 设备流令牌）：
   - `GH_TOKEN` 或 `GITHUB_TOKEN` 环境变量（`token_source: env`）
   - `~/.config/github-copilot/hosts.json` → `github.com.oauth_token`
     （`token_source: hosts_file` 或 `auto`）
2. 通过 `GET https://api.github.com/copilot_internal/v2/token` 换取**短期有效的
   Copilot API 令牌**，使用 `Authorization: token {github_oauth_token}`。
3. 缓存 Copilot API 令牌；在到期前 5 分钟自动重新交换。

Copilot API 令牌随后以 `Authorization: Bearer {api_token}` 的形式用于向
`https://api.githubcopilot.com/chat/completions` 发送请求。

---

## 配置格式

```yaml
models:
  - name: copilot-gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o
        token_source: auto        # auto | env | hosts_file（默认：auto）
        # credentials_path: /custom/path/to/hosts.json  # 可选
```

**`token_source` 取值：**

| 值 | 行为 |
|---|---|
| `auto`（默认） | 先尝试环境变量，再尝试 hosts.json |
| `env` | 仅使用 `GH_TOKEN` 或 `GITHUB_TOKEN` 环境变量 |
| `hosts_file` | 仅使用 `~/.config/github-copilot/hosts.json`（或 `credentials_path`） |

---

## 修改的文件

| 文件 | 变更说明 |
|---|---|
| `src/auth/copilot.rs` | **新增** — CopilotTokenManager |
| `src/auth/mod.rs` | 添加 `pub mod copilot;` |
| `src/providers/github_copilot.rs` | **新增** — GitHubCopilotProvider |
| `src/providers/mod.rs` | 添加 `pub mod github_copilot;` |
| `src/server/mod.rs` | 在 `build_providers()` 中添加 `"github-copilot"` 分支 |
| `config.example.yaml` | 添加 GitHub Copilot 配置示例 |
| `docs/GITHUB_COPILOT_PROVIDER.md` | **新增** — 提供商文档（英文） |
| `docs/GITHUB_COPILOT_PROVIDER_CN.md` | **新增** — 提供商文档（中文） |
| `docs/ACCEPTANCE_2_8A.md` | **新增** — 验收报告（英文） |
| `docs/ACCEPTANCE_2_8A_CN.md` | **新增** — 本文件 |

---

## 运行的测试

```
cargo test
running 83 tests
test result: ok. 83 passed; 0 failed
```

新增测试：

**`auth::copilot::tests`**（13 个测试）：令牌过期逻辑、刷新边界、从环境变量加载、
从 hosts.json 加载、auto 优先级等。

**`providers::github_copilot::tests`**（3 个测试）：提供商名称、模型列表、空模型列表。

---

## 本地端到端验收

| 步骤 | 结果 |
|---|---|
| `cargo build` | ✅ 编译通过 |
| `cargo test` | ✅ 83/83 通过 |
| 使用 Copilot 配置启动服务器 | ✅ `Server listening, address: 127.0.0.1:14321` |
| `GET /v1/models` | ✅ 返回 `copilot-gpt-4o` 和 `copilot-claude-sonnet` |
| `POST /v1/chat/completions` | ⚠️ 认证受阻（见下文） |

---

## 阻塞项 / 限制

**本地端到端因凭证兼容性问题受阻。**

GitHub 的 `/copilot_internal/v2/token` 接口要求 GitHub OAuth 设备流令牌（`ghu_...`）。
本机现有凭证：

- `~/.config/github-copilot/hosts.json` 中的 `ghu_...` 令牌 → **已过期**
  （返回 HTTP 401 Bad credentials）
- `GH_TOKEN` 环境变量中的细粒度 PAT（`github_pat_...`）→ **不兼容**
  （返回 HTTP 403 "Resource not accessible by personal access token"）

**这是凭证可用性问题，而非实现 bug。**

**获取有效令牌的方法：**

```bash
# 方式 A — GitHub CLI（创建 ghu_... OAuth 令牌）：
gh auth login --scopes read:user

# 方式 B — 设置具有 `read:user` 权限的经典 PAT（ghp_...）：
export GH_TOKEN=ghp_yourClassicPAT
```

---

## 已知限制

1. 配置字段 `base_url` 对该提供商无效；接口端点由令牌交换响应决定（默认为
   `https://api.githubcopilot.com`）。
2. 工具/函数调用支持取决于上游 Copilot 模型。
3. Copilot 速率限制和模型可用性由 GitHub 控制；Rausu 原样传递上游 HTTP 状态码。
4. 不支持 `/v1/messages` 和 `/v1/responses` 的直通功能——Copilot 不实现这些 API。
