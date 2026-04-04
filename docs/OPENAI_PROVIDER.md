# OpenAI Provider

> **中文版:** [OPENAI_PROVIDER_CN.md](OPENAI_PROVIDER_CN.md)

## Overview

The `openai` provider routes requests to the OpenAI API or any **OpenAI-compatible** endpoint using an API key. It supports Chat Completions, the Responses API, and works with any provider that implements the OpenAI Chat Completions format — DeepSeek, Qwen (Aliyun DashScope), Ollama, GLM, Moonshot, Baichuan, Yi, MiniMax, and more.

**Phase 3 protocol bridge:** When Codex CLI sends Responses API requests (`/v1/responses`) to a generic OpenAI-compatible provider, Rausu automatically bridges Responses → Chat Completions format. This means Codex CLI works with any provider backed by `provider: openai` + `base_url` without any client-side configuration.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | ✅ (streaming + non-streaming) |
| `POST /v1/responses` | ✅ native passthrough (OpenAI); Responses→ChatCompletions bridge (generic providers) |
| `GET /v1/models` | ✅ lists configured model names |
| `POST /v1/messages` | ❌ Use `provider: anthropic` for Anthropic Messages API |

## Prerequisites

An [OpenAI API key](https://platform.openai.com/api-keys) with access to the models you want to use.

## Authentication

Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${OPENAI_API_KEY}"
```

```bash
export OPENAI_API_KEY="sk-..."
```

## Quick start

### 1. Add to config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  - name: o3
    providers:
      - provider: openai
        model: o3
        api_key: "${OPENAI_API_KEY}"
```

### 2. Start Rausu

```bash
rausu --config config.yaml
```

### 3. Send a request

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
codex --model o3
```

## Using the Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer fake-key" \
  -d '{
    "model": "gpt-4o",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: openai
      model: <openai-model-id>    # Required
      api_key: <your-api-key>     # Required
      base_url: <url>             # Optional; default: https://api.openai.com/v1
```

### `base_url`

Overrides the default `https://api.openai.com/v1` endpoint. Use this to point at an OpenAI-compatible API (Azure OpenAI, a local proxy, etc.).

```yaml
base_url: "https://your-azure-endpoint.openai.azure.com/openai/deployments/gpt-4o"
```

## OpenAI-compatible Providers (Phase 3)

Any provider with an OpenAI-compatible Chat Completions endpoint works by setting `base_url`. When Codex CLI uses the Responses API against these providers, Rausu bridges Responses → Chat Completions automatically.

### DeepSeek

```yaml
models:
  - name: deepseek-chat
    providers:
      - provider: openai
        model: deepseek-chat
        base_url: https://api.deepseek.com/v1
        api_key: sk-xxx
  - name: deepseek-reasoner
    providers:
      - provider: openai
        model: deepseek-reasoner
        base_url: https://api.deepseek.com/v1
        api_key: sk-xxx
```

```bash
export OPENAI_BASE_URL=http://localhost:4000
codex --model deepseek-chat
```

### Qwen (Aliyun DashScope)

```yaml
models:
  - name: qwen-max
    providers:
      - provider: openai
        model: qwen-max
        base_url: https://dashscope.aliyuncs.com/compatible-mode/v1
        api_key: sk-xxx
  - name: qwen-plus
    providers:
      - provider: openai
        model: qwen-plus
        base_url: https://dashscope.aliyuncs.com/compatible-mode/v1
        api_key: sk-xxx
```

### Ollama (local)

```yaml
models:
  - name: llama3
    providers:
      - provider: openai
        model: llama3
        base_url: http://localhost:11434/v1
        api_key: ollama   # Ollama ignores this; any non-empty value works
  - name: qwen2.5-coder
    providers:
      - provider: openai
        model: qwen2.5-coder:7b
        base_url: http://localhost:11434/v1
        api_key: ollama
```

```bash
export OPENAI_BASE_URL=http://localhost:4000
codex --model llama3
```

### Other compatible providers

The same pattern works for any OpenAI-compatible endpoint:

| Provider | `base_url` |
|---|---|
| Moonshot (Kimi) | `https://api.moonshot.cn/v1` |
| GLM (Zhipu AI) | `https://open.bigmodel.cn/api/paas/v4` |
| Yi (01.AI) | `https://api.lingyiwanwu.com/v1` |
| MiniMax | `https://api.minimax.chat/v1` |
| Baichuan | `https://api.baichuan-ai.com/v1` |
| Groq | `https://api.groq.com/openai/v1` |
| Together AI | `https://api.together.xyz/v1` |

## Upstream model names

Any model available on your OpenAI account can be used. Common examples:

| Model ID | Description |
|---|---|
| `gpt-4o` | GPT-4o (multimodal) |
| `gpt-4o-mini` | GPT-4o Mini (fast, cost-effective) |
| `o3` | o3 reasoning model |
| `o4-mini` | o4-mini reasoning model |
| `gpt-4-turbo` | GPT-4 Turbo |

Check the [OpenAI model documentation](https://platform.openai.com/docs/models) for the full list of available model IDs.

## Docker deployment

```bash
docker run \
  -e OPENAI_API_KEY="sk-..." \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Known limitations

- **No `/v1/messages` support.** Use `provider: anthropic` for Anthropic-native routing.
- Rate limits and model availability are controlled by the upstream provider — Rausu propagates the upstream HTTP status code unchanged.
- Tool/function calling is passed through as-is in the Chat Completions format; no additional translation is performed.
- Generic providers via `base_url` must support the OpenAI Chat Completions API format. Providers with non-standard formats are not supported.
