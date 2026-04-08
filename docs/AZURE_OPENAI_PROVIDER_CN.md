# Azure OpenAI Provider

> **English version:** [AZURE_OPENAI_PROVIDER.md](AZURE_OPENAI_PROVIDER.md)

## 概述

`azure-openai` provider 将请求路由到 [Azure OpenAI Service](https://azure.microsoft.com/en-us/products/ai-services/openai-service)。与标准 OpenAI 不同，Azure OpenAI 使用不同的 URL 结构和认证机制：

- **认证方式：** `api-key: <key>` 请求头（不是 `Authorization: Bearer <key>`）
- **URL 格式：** `{base_url}/openai/deployments/{deployment}/chat/completions?api-version={version}`
- **config 中的 `model`** 是 Azure 部署名称，用于 URL 路径——**不会**作为请求体发送

**Responses API 桥接：** 当 Codex CLI 或其他客户端向 Azure OpenAI 支持的模型发送 Responses API 请求（`/v1/responses`）时，Rausu 自动进行 Responses -> Chat Completions 格式桥接，与 `openai`、`deepseek`、`openrouter`、`moonshot` 和 `z-ai` provider 使用相同策略。

## 支持矩阵

| 端点 | 支持 |
|---|---|
| `POST /v1/chat/completions` | 流式 + 非流式 |
| `POST /v1/responses` | Responses->ChatCompletions 桥接 |
| `GET /v1/models` | 列出已配置的模型名 |
| `POST /v1/messages` | 不支持（请使用 `provider: anthropic`） |

## 前提条件

1. 一个 [Azure OpenAI Service](https://portal.azure.com) 资源
2. 在该资源中部署的模型（部署名称）
3. Azure 门户中的 API Key（密钥和端点部分）

## 认证

Azure OpenAI 使用 `api-key` 请求头而非标准的 Bearer Token。在 `config.yaml` 中或通过环境变量设置 API Key：

```yaml
api_key: "${AZURE_OPENAI_API_KEY}"
```

```bash
export AZURE_OPENAI_API_KEY="your-api-key-here"
```

## 快速开始

### 1. 添加到 config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: gpt-4o
    providers:
      - provider: azure-openai
        model: gpt-4o                  # Azure 部署名称
        api_key: "${AZURE_OPENAI_API_KEY}"
        base_url: "https://my-resource.openai.azure.com/"
```

### 2. 启动 Rausu

```bash
rausu --config config.yaml
```

### 3. 发送请求

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配合 Codex CLI 使用

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu 忽略此值；真实 Key 在 config.yaml 中
codex --model gpt-4o
```

## 使用 Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置参考

```yaml
- name: <虚拟模型名>
  providers:
    - provider: azure-openai
      model: <deployment-name>           # 必填 — Azure 部署名称
      api_key: <your-api-key>            # 必填
      base_url: <azure-endpoint>         # 必填 — 如 https://<resource>.openai.azure.com/
      api_version: <version>             # 可选；默认: 2024-12-01-preview
```

### `model`

`model` 字段是 **Azure 部署名称**，不是 OpenAI 模型 ID。在 Azure 门户创建部署时，您选择的名称就是此处填写的值。

示例模型映射：

| 虚拟名称 (config `name`) | 部署名称 (config `model`) | 底层 OpenAI 模型 |
|---|---|---|
| `gpt-4o` | `gpt-4o` | GPT-4o |
| `gpt-4o-mini` | `gpt-4o-mini-deployment` | GPT-4o mini |
| `my-custom-model` | `prod-gpt4-turbo` | GPT-4 Turbo |

### `base_url`

**必填。** Azure 资源端点 URL。在 Azure 门户中您的 OpenAI 资源 → 密钥和端点 部分可以找到。

格式：`https://<resource-name>.openai.azure.com/`

Rausu 在构建部署 URL 前会去除尾部斜杠。

### `api_version`

Azure OpenAI API 版本查询参数。未指定时默认为 `2024-12-01-preview`。

常用值：
- `2024-12-01-preview`（默认）
- `2025-01-01-preview`

完整列表请参见 [Azure OpenAI API 版本文档](https://learn.microsoft.com/en-us/azure/ai-services/openai/api-version-deprecation)。

## URL 构建

Rausu 按以下方式构建上游 URL：

```
{base_url}/openai/deployments/{deployment_name}/chat/completions?api-version={api_version}
```

例如，当配置为：
- `base_url: https://my-resource.openai.azure.com/`
- `model: gpt-4o`（部署名称）
- `api_version: 2024-12-01-preview`

生成的 URL 为：
```
https://my-resource.openai.azure.com/openai/deployments/gpt-4o/chat/completions?api-version=2024-12-01-preview
```

`model` 字段**不会**包含在发送给 Azure 的请求体中——Azure 从 URL 路径中的部署名称确定模型。

## 多 Provider 故障转移

Azure OpenAI 模型可以与其他 provider 一起参与 Rausu 的优先级故障转移：

```yaml
- name: gpt-4o
  providers:
    - provider: azure-openai        # 先尝试 Azure
      model: gpt-4o
      api_key: "${AZURE_OPENAI_API_KEY}"
      base_url: "https://my-resource.openai.azure.com/"
    - provider: openai              # 失败后转到直连 OpenAI
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
```

## 能力感知路由

Azure OpenAI provider 向 Rausu 路由器声明以下能力：

| 能力 | 是否声明 |
|---|---|
| `chat_completions` | 是 |
| `streaming` | 是（SSE） |
| `responses_api` | 是（Responses → Chat Completions 桥接） |
| `tools` | 是（透传给 Azure OpenAI） |
| `response_format` | 是（透传给 Azure OpenAI） |

**路由工作方式：**

1. 请求到达时，路由器检查请求内容并确定所需能力。包含 `tools` 的请求需要 `tools` 能力；包含 `response_format` 的请求需要 `response_format` 能力。
2. 缺少任何所需能力的 provider 在**发起上游调用之前**就会被跳过。
3. 如果同一虚拟模型下还有其他已配置的 provider 支持所需能力，故障转移会继续到该 provider。
4. 如果**没有任何**已配置的 provider 支持所有所需能力，Rausu 会向客户端返回明确的错误，而不是静默降级或剥离字段。

### `unsupported_capability` 错误

当某个模型的所有 provider 因缺少所需能力而被跳过时，Rausu 返回：

- **HTTP 状态码：** `422 Unprocessable Entity`
- **`error.type`：** `unsupported_capability`
- **`error.code`：** `unsupported_capability`
- **`error.message`：** 说明缺少哪些能力

响应体示例：

```json
{
  "error": {
    "message": "No provider for model 'my-model' supports the required capabilities: tools",
    "type": "unsupported_capability",
    "code": "unsupported_capability"
  }
}
```

### 无静默降级策略

在 Azure OpenAI 路径上，Rausu **不会**静默剥离请求中的 `tools`、`tool_choice` 或 `response_format` 字段。如果所选上游模型不支持请求的能力，上游错误会原样传播给客户端。

> **注意：** 上述能力声明反映的是 Rausu 中 Azure OpenAI provider 向路由器暴露的内容。实际能力支持仍取决于具体的上游部署和模型版本。

## Docker 部署

```bash
docker run \
  -e AZURE_OPENAI_API_KEY="your-key" \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## 故障排查

| 症状 | 原因 | 解决方法 |
|---|---|---|
| `401 Unauthorized` | 无效或缺失的 API Key | 确认 `AZURE_OPENAI_API_KEY` 已设置且有效 |
| `404 Not Found` | 部署名称或 base URL 错误 | 检查 `model` 是否匹配 Azure 部署名称，`base_url` 是否匹配资源端点 |
| `400 Bad Request` 含 "api-version" | 缺失或无效的 API 版本 | 将 `api_version` 设置为有效的 Azure OpenAI API 版本 |
| 启动错误：`base_url is required` | 未配置 `base_url` | 添加指向 Azure 资源端点的 `base_url` |
| `429 Too Many Requests` | 超出速率限制 | 降低请求频率或添加其他 provider 进行故障转移 |
| Rausu 中找不到模型 | config `name` 与客户端请求不匹配 | 确保客户端发送的是 config 中准确的虚拟 `name` |

## 已知限制

- **不支持 `/v1/messages`。** Azure OpenAI 使用 OpenAI 兼容格式。如需 Anthropic Messages API 直传，请使用 `provider: anthropic` 或 `provider: claude-subscription`。
- **无原生 Responses API。** Rausu 自动进行 Responses -> Chat Completions 桥接。
- **`base_url` 为必填。** 与其他 OpenAI 兼容 provider 不同，Azure OpenAI 没有默认端点——您必须提供 Azure 资源 URL。
- 速率限制和模型可用性由 Azure 控制。Rausu 原样传播上游 HTTP 状态码。
- 工具/函数调用直接透传；不进行额外转换。能力取决于所选的上游部署。
