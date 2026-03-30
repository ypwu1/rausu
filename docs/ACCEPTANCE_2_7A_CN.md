# 阶段 2.7A — 真实本地端到端验收报告

> [English version](ACCEPTANCE_2_7A.md)

**日期：** 2026-03-30
**测试执行：** 自动化验收运行（Claude Code agent）
**仓库状态：** `main` 分支，阶段 2.6A/B/C 完成后

---

## 总体状态

| 验收路径 | 状态 |
|---------|------|
| Claude Code → Rausu | **通过（DONE）** |
| Codex CLI → Rausu | **阻塞（BLOCKED）**，凭据缺失 |

---

## 测试环境

| 项目 | 值 |
|------|-----|
| 平台 | Linux（Ubuntu，x86-64）|
| Rust | stable（release 构建）|
| Rausu 版本 | 0.1.0 |
| 二进制文件 | `./target/release/rausu` |
| 使用配置 | `config-test.yaml` |
| Claude Code 版本 | 2.1.87 |
| Codex CLI 版本 | codex-cli 0.117.0（通过 `npx`）|
| Claude OAuth token | 有效（`~/.claude/.credentials.json`，测试时剩余约 7 小时）|
| OpenAI API key | **不可用** |
| ChatGPT 订阅凭据 | **不可用** |

---

## 路径一：Claude Code → Rausu

### 配置

`config-test.yaml`：
```yaml
server:
  host: 127.0.0.1
  port: 4000

logging:
  level: debug
  format: pretty

models:
  - name: claude-sonnet-4-20250514
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: auto
```

### 测试命令

```bash
# 启动 Rausu
./target/release/rausu --config config-test.yaml

# 验证服务器运行状态
curl http://localhost:4000/health
# → {"status":"ok"}

curl http://localhost:4000/v1/models
# → {"object":"list","data":[{"id":"claude-sonnet-4-20250514",...}]}

# 通过 Rausu 运行 Claude Code
ANTHROPIC_BASE_URL="http://localhost:4000" \
ANTHROPIC_API_KEY="local-proxy" \
  claude -p "Reply with exactly: e2e-pass" --model claude-sonnet-4-20250514
# → e2e-pass
```

### 服务器日志（关键行）

```
INFO  rausu: Rausu starting, version: "0.1.0", config: config-test.yaml
INFO  rausu::server: Server listening, address: 127.0.0.1:4000
INFO  rausu::server::routes::messages: Messages proxy succeeded,
        model: claude-sonnet-4-20250514, provider: claude-subscription,
        status: 200, stream: true
```

### 结果

**通过。** Claude Code 2.1.87 成功将 `POST /v1/messages` 请求通过 Rausu 路由至 Claude 订阅端点，使用本地 OAuth token。非流式和流式请求均正常工作。

---

## 路径二：Codex CLI → Rausu

### 测试内容

```bash
# 验证 Codex CLI 可用
npx codex --version
# → codex-cli 0.117.0

# 测试 /v1/responses 端点行为（Codex 的主要路径）
curl -s -X POST http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer local-proxy" \
  -d '{"model":"claude-sonnet-4-20250514","input":"test"}'
# → {"error":{"message":"Unsupported operation: Provider 'claude-subscription'
#             does not support the Responses API","type":"internal_server_error"}}
```

### 结果

**阻塞。** Codex CLI 需要 OpenAI Responses API（`/v1/responses`）。该端点仅支持 `openai` 和 `chatgpt-subscription` 两种 provider。当前机器上均无相应凭据：

- `OPENAI_API_KEY` — 未设置
- `~/.config/rausu/chatgpt-auth.json` — 不存在
- `CHATGPT_ACCESS_TOKEN` — 未设置

端点本身已正确实现，对不支持的 provider 返回清晰的错误信息。**这是凭据缺失导致的阻塞，不是代码缺陷。**

解除 Codex CLI 验收阻塞的方法：
1. 添加 OpenAI API key：设置 `OPENAI_API_KEY` 并在 `config.yaml` 中添加 `openai` provider 条目
2. **或** 将 ChatGPT 订阅凭据写入 `~/.config/rausu/chatgpt-auth.json` 并添加 `chatgpt-subscription` provider 条目

---

## 发现并修复的缺陷

### `context_management` beta 头转发缺失

**症状：** Claude Code 2.1.87 返回：
```
API Error: 400 {"type":"error","error":{
  "type":"invalid_request_error",
  "message":"context_management: Extra inputs are not permitted"}}
```

**根本原因：** Rausu 的 `proxy_messages` 将 `anthropic-beta` 请求头硬编码为固定列表。Claude Code 2.1.87 在请求体中发送 `context_management` 字段，该字段需要对应的 beta 头才能被 Anthropic API 接受。由于 Rausu 覆盖了客户端的 beta 头而非合并，导致所需 beta 缺失，API 拒绝了该请求。

**修复方案：** `proxy_messages` 方法新增 `client_betas: Option<String>` 参数，接收下游客户端的 `anthropic-beta` 头值。`claude-subscription` provider 在转发前将客户端 betas 与必需的 OAuth betas 合并；`anthropic` provider 直接转发客户端 betas。

**修改文件：**
- `src/providers/mod.rs` — 为 `proxy_messages` trait 默认方法添加 `client_betas` 参数；将 `Provider` trait 移至测试模块之前（修复已有 clippy 警告）
- `src/server/routes/messages.rs` — 从请求头中提取 `anthropic-beta`；传递给 `proxy_messages`
- `src/providers/claude_subscription.rs` — 添加 `merge_betas()` 辅助函数；转发时使用合并后的 betas
- `src/providers/anthropic.rs` — 有客户端 betas 时进行转发

**测试：** 66 个单元测试 + 2 个集成测试全部通过。`cargo clippy` 和 `cargo fmt` 均无问题。

---

## 复现说明

### Claude Code 通过 Rausu（通过）

**前提条件：** 已登录 Claude Code（`~/.claude/.credentials.json` 存在且有效）

```bash
cd /path/to/rausu

# 构建
cargo build --release

# 创建配置（或使用 config-test.yaml）
cat > config.yaml <<'EOF'
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
EOF

# 启动 Rausu
./target/release/rausu --config config.yaml &

# 将 Claude Code 指向 Rausu
export ANTHROPIC_BASE_URL="http://localhost:4000"
export ANTHROPIC_API_KEY="local-proxy"
claude -p "Hello from Rausu"
```

### Codex CLI 通过 Rausu（阻塞，需要凭据）

获取 OpenAI API key 或 ChatGPT 订阅凭据后：

```bash
# 使用 OpenAI API key
export OPENAI_API_KEY="sk-..."

# 在 config.yaml 中添加：
# models:
#   - name: gpt-4o
#     providers:
#       - provider: openai
#         model: gpt-4o
#         api_key: "${OPENAI_API_KEY}"

./target/release/rausu --config config.yaml &

export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="local-proxy"
codex --model gpt-4o
```

---

## 剩余阻塞项

| 阻塞项 | 类型 | 所需操作 |
|--------|------|---------|
| Codex CLI：无 OpenAI 或 ChatGPT 凭据 | 环境/凭据 | 提供 `OPENAI_API_KEY` 或 ChatGPT 订阅凭据 |

Claude Code 验收路径无残余代码缺陷。Codex 阻塞纯属环境限制。
