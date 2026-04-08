# MiniMax 提供商

> **English version:** [MINIMAX_PROVIDER.md](MINIMAX_PROVIDER.md)

## 概述

`minimax` 提供商将请求路由至 [MiniMax](https://www.minimax.io)。MiniMax 在 `api.minimax.io` 同时提供 **Anthropic 兼容** 和 **OpenAI 兼容** 两套 API。Rausu 在单个 `provider: minimax` 配置项内部完成协议选择，无需分别配置 `minimax-openai` 和 `minimax-anthropic` 两个提供商。

**内部协议路由规则：**

| 下游请求 | MiniMax 上游端点 |
|---|---|
| `POST /v1/chat/completions` | `https://api.minimax.io/v1/chat/completions`（OpenAI 兼容） |
| `POST /v1/messages` | `https://api.minimax.io/anthropic/v1/messages`（Anthropic 兼容） |
| `POST /v1/responses` | 桥接：Responses → Chat Completions → Responses |

## 支持矩阵

| 端点 | 支持状态 |
|---|---|
| `POST /v1/chat/completions` | ✅ 流式 + 非流式 |
| `POST /v1/messages` | ✅ 文本与工具调用（流式 + 非流式） |
| `POST /v1/responses` | ✅ 通过 Chat Completions 桥接转换 |
| `GET /v1/models` | ✅ 列出已配置的模型名称 |
| 图像 / 文档输入 | ❌ 不支持（返回明确错误） |

## 前置条件

需要 MiniMax API 密钥。在 [minimax.io](https://www.minimax.io) 注册账号并在控制台生成密钥。

## 认证方式

在 `config.yaml` 中配置密钥，或通过环境变量传入：

```yaml
api_key: "${MINIMAX_API_KEY}"
```

```bash
export MINIMAX_API_KEY="eyJ..."
```

Rausu 在 OpenAI 兼容路径和 Anthropic 兼容路径均使用 `Authorization: Bearer <key>` 头部发送密钥。

## 快速开始

### 1. 在 config.yaml 中添加配置

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: minimax-text-01
    providers:
      - provider: minimax
        model: minimax-text-01
        api_key: "${MINIMAX_API_KEY}"

  - name: abab6.5s-chat
    providers:
      - provider: minimax
        model: abab6.5s-chat
        api_key: "${MINIMAX_API_KEY}"
```

### 2. 启动 Rausu

```bash
rausu --config config.yaml
```

### 3. 发送请求

**Chat Completions：**

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-text-01",
    "messages": [{"role": "user", "content": "你好！"}]
  }'
```

**Messages API：**

```bash
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "minimax-text-01",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "你好！"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名称>
  providers:
    - provider: minimax
      model: <minimax-模型ID>         # 必填（如 "minimax-text-01"）
      api_key: <你的API密钥>          # 必填
      base_url: <url>                 # 可选；默认：https://api.minimax.io
```

### `model`

使用 MiniMax 模型 ID，示例：

| 模型 ID | 描述 |
|---|---|
| `minimax-text-01` | MiniMax Text-01 旗舰模型 |
| `abab6.5s-chat` | MiniMax ABAB 6.5S 对话模型 |
| `abab6.5g-chat` | MiniMax ABAB 6.5G 对话模型 |
| `abab5.5s-chat` | MiniMax ABAB 5.5S 对话模型 |

完整模型列表请参阅 [MiniMax 模型目录](https://www.minimax.io/platform/document/model-list)。

### `base_url`

覆盖默认根地址 `https://api.minimax.io`。Rausu 会自动在 OpenAI 兼容请求后拼接 `/v1`，在 Anthropic 兼容请求后拼接 `/anthropic/v1`。

```yaml
base_url: "https://api.minimax.io"   # 默认值
```

此选项适用于通过本地代理或自定义 MiniMax 端点路由流量的场景。

## 与 Codex CLI 配合使用

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu 忽略此值；真实密钥已在 config.yaml 中配置
codex --model minimax-text-01
```

## 与 Claude Code 配合使用

```bash
export ANTHROPIC_BASE_URL="http://localhost:4000"
claude --model minimax-text-01
```

## 工具调用

Messages API 和 Chat Completions 路径均支持工具调用。

**Chat Completions（OpenAI 工具格式）：**

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-text-01",
    "messages": [{"role": "user", "content": "东京现在天气怎么样？"}],
    "tools": [{
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "获取城市当前天气",
        "parameters": {
          "type": "object",
          "properties": {
            "city": {"type": "string"}
          },
          "required": ["city"]
        }
      }
    }]
  }'
```

**Messages API（Anthropic 工具格式）：**

```bash
curl -s http://localhost:4000/v1/messages \
  -H "Content-Type: application/json" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "minimax-text-01",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "东京现在天气怎么样？"}],
    "tools": [{
      "name": "get_weather",
      "description": "获取城市当前天气",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": {"type": "string"}
        },
        "required": ["city"]
      }
    }]
  }'
```

## 使用 Responses API

Rausu 自动将 Responses API 请求通过 MiniMax 的 OpenAI 兼容端点进行桥接转换：

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "minimax-text-01",
    "input": [{"role": "user", "content": "你好！"}]
  }'
```

## 多提供商故障转移

MiniMax 模型可参与 Rausu 的优先级故障转移机制：

```yaml
- name: my-llm
  providers:
    - provider: anthropic          # 优先使用 Anthropic
      model: claude-sonnet-4-5
      api_key: "${ANTHROPIC_API_KEY}"
    - provider: minimax            # 降级使用 MiniMax
      model: minimax-text-01
      api_key: "${MINIMAX_API_KEY}"
```

## 能力感知路由

MiniMax 提供商向 Rausu 路由器声明的能力如下：

| 能力 | 是否声明 |
|---|---|
| `chat_completions` | 是 |
| `streaming` | 是（两条路径均支持 SSE） |
| `responses_api` | 是（Responses → Chat Completions 桥接） |
| `tools` | 是（两条路径均支持） |
| `messages_api` | 是（Anthropic 兼容直传） |
| `response_format` | 否 |

### `unsupported_capability` 错误

当模型的所有提供商因缺少所需能力而被跳过时，Rausu 返回：

- **HTTP 状态码：** `422 Unprocessable Entity`
- **`error.type`：** `unsupported_capability`
- **`error.code`：** `unsupported_capability`

示例响应体：

```json
{
  "error": {
    "message": "No provider for model 'minimax-text-01' supports the required capabilities: response_format",
    "type": "unsupported_capability",
    "code": "unsupported_capability"
  }
}
```

## 已知限制

- **不支持图像或文档输入。** MiniMax 的 Anthropic 兼容端点不支持 `image` 或 `document` 内容块。包含此类内容块的请求在到达 MiniMax 之前即会被拒绝，返回 `405 Unsupported`，符合 Rausu 的"不静默降级"原则。
- **不原生支持 Responses API。** Rausu 自动进行 Responses → Chat Completions 桥接。
- **未声明 `response_format` 能力。** 如需结构化输出，请使用声明了 `response_format` 的提供商。
- 速率限制和模型可用性由 MiniMax 控制。Rausu 原样透传上游 HTTP 状态码。

## 故障排查

| 现象 | 原因 | 解决方法 |
|---|---|---|
| `401 Unauthorized` | API 密钥无效或缺失 | 检查 `MINIMAX_API_KEY` 是否已设置且有效 |
| `429 Too Many Requests` | 超出速率限制 | 降低请求频率或添加备用提供商 |
| `404 Not Found` | 模型 ID 无效 | 对照 MiniMax 模型目录检查模型 ID |
| Messages API 返回 `405` | 请求包含图像/文档内容块 | 移除不支持的内容块类型 |
| Rausu 找不到模型 | 配置中 `name` 与客户端请求的模型名不一致 | 确保客户端发送的模型名与配置中的虚拟 `name` 完全一致 |
