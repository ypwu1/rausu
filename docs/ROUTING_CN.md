# 优先级故障转移路由

Rausu 支持为每个模型配置多个提供商，并自动进行故障转移。当主要提供商返回可重试错误时，请求会自动转发到下一个优先级的提供商。

## 配置

在模型的 `providers` 字段中列出多个提供商。YAML 中的顺序定义优先级（第一个 = 最高优先级）：

```yaml
models:
  - name: claude-sonnet-4-6
    aliases: ["claude-sonnet-4-6-20250514"]
    providers:
      - provider: github-copilot        # 优先级 1：免费（Copilot）
        model: claude-sonnet-4.6
      - provider: claude-subscription    # 优先级 2：订阅
        model: claude-sonnet-4-6
      - provider: anthropic              # 优先级 3：按量付费 API
        model: claude-sonnet-4-6

  - name: gpt-4o
    providers:
      - provider: github-copilot
        model: gpt-4o
      - provider: chatgpt-subscription
        model: gpt-4o
      - provider: openai
        model: gpt-4o
```

别名会解析到与主模型名称相同的提供商列表。

## 工作原理

1. 收到对模型 `claude-sonnet-4-6` 的请求。
2. Rausu 尝试第一个提供商（`github-copilot`）。
3. 如果该提供商返回可重试错误（如 429 速率限制），Rausu 记录警告并尝试下一个提供商（`claude-subscription`）。
4. 如果也失败，Rausu 尝试最后一个提供商（`anthropic`）。
5. 如果所有提供商都失败，Rausu 返回 503 Service Unavailable。

对于不可重试的错误（如 400 Bad Request），错误会立即返回给客户端，不会尝试其他提供商。

## 可重试错误

以下状态码会触发故障转移：

| 状态码 | 含义 |
|---|---|
| 429 | 速率限制 |
| 500 | 内部服务器错误 |
| 502 | 网关错误 |
| 503 | 服务不可用 |
| 504 | 网关超时 |

此外：
- **传输层故障**（连接拒绝、DNS 错误、超时）始终可重试。
- **不支持的操作**（如提供商不支持 Messages API）会自动跳过。

不可重试的错误（400、401、403、404 等）会立即返回给客户端。

## 流式传输安全性

对于代理端点（Messages API、Responses API），故障转移决策基于上游 HTTP 状态码，在任何响应字节流式传输到客户端**之前**做出。一旦开始流式传输，就无法切换提供商。

对于 Chat Completions 流式传输，如果初始 `chat_completions_stream()` 调用返回错误，则进行故障转移。一旦 SSE 流开始产生数据块，就确定使用该提供商。

## 日志记录

Rausu 在不同级别记录故障转移活动：

- **INFO**：正在尝试的提供商（`Trying provider`）和成功的提供商（`Request served by provider`）。
- **WARN**：提供商失败并进行故障转移（`Provider failed, falling back`）。
- **ERROR**：所有提供商都已耗尽（`All providers failed`）。

日志示例：
```
INFO  Trying provider model=claude-sonnet-4-6 provider=github-copilot attempt=1
WARN  Provider failed, falling back model=claude-sonnet-4-6 provider=github-copilot status=429
INFO  Trying provider model=claude-sonnet-4-6 provider=claude-subscription attempt=2
INFO  Request served by provider model=claude-sonnet-4-6 provider=claude-subscription
```

## 与认证和 TLS 的交互

- **API 密钥认证**：每个到 Rausu 的请求仍需要有效的 API 密钥（如果配置了认证）。故障转移对客户端透明。
- **TLS/mTLS**：故障转移发生在上游提供商层面。客户端与 Rausu 的 TLS 连接不受影响。
- 提供商特定的凭据（API 密钥、OAuth 令牌、服务账户密钥）按提供商管理，在故障转移期间自动使用。

## 单提供商（无故障转移）

如果模型只配置了一个提供商，行为与之前完全相同：错误直接返回给客户端，不执行故障转移逻辑。
