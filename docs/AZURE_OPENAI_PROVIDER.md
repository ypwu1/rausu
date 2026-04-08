# Azure OpenAI Provider

> **中文版:** [AZURE_OPENAI_PROVIDER_CN.md](AZURE_OPENAI_PROVIDER_CN.md)

## Overview

The `azure-openai` provider routes requests to [Azure OpenAI Service](https://azure.microsoft.com/en-us/products/ai-services/openai-service). Unlike standard OpenAI, Azure OpenAI uses a different URL structure and authentication mechanism:

- **Authentication:** `api-key: <key>` header (not `Authorization: Bearer <key>`)
- **URL pattern:** `{base_url}/openai/deployments/{deployment}/chat/completions?api-version={version}`
- **`model` in config** is the Azure deployment name, used in the URL path — it is **not** sent in the request body

**Responses API bridge:** When Codex CLI or other clients send Responses API requests (`/v1/responses`) to an Azure OpenAI-backed model, Rausu automatically bridges Responses -> Chat Completions format, the same strategy used by the `openai`, `deepseek`, `openrouter`, `moonshot`, and `z-ai` providers.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | Streaming + non-streaming |
| `POST /v1/responses` | Responses->ChatCompletions bridge |
| `GET /v1/models` | Lists configured model names |
| `POST /v1/messages` | Not supported (use `provider: anthropic`) |

## Prerequisites

1. An [Azure OpenAI Service](https://portal.azure.com) resource
2. A deployed model (deployment name) within that resource
3. An API key from the Azure portal (Keys and Endpoint section)

## Authentication

Azure OpenAI uses the `api-key` header instead of the standard Bearer token. Set your API key in `config.yaml` or via an environment variable:

```yaml
api_key: "${AZURE_OPENAI_API_KEY}"
```

```bash
export AZURE_OPENAI_API_KEY="your-api-key-here"
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
      - provider: azure-openai
        model: gpt-4o                  # Azure deployment name
        api_key: "${AZURE_OPENAI_API_KEY}"
        base_url: "https://my-resource.openai.azure.com/"
```

### 2. Start Rausu

```bash
rausu --config config.yaml
```

### 3. Send a request

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="fake-key"   # Rausu ignores this; the real key is in config.yaml
codex --model gpt-4o
```

## Using the Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "input": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration reference

```yaml
- name: <virtual-model-name>
  providers:
    - provider: azure-openai
      model: <deployment-name>           # Required — Azure deployment name
      api_key: <your-api-key>            # Required
      base_url: <azure-endpoint>         # Required — e.g. https://<resource>.openai.azure.com/
      api_version: <version>             # Optional; default: 2024-12-01-preview
```

### `model`

The `model` field is the **Azure deployment name**, not the OpenAI model ID. When you create a deployment in the Azure portal, you choose a name — that name is what goes here.

Example model mappings:

| Virtual name (config `name`) | Deployment name (config `model`) | Underlying OpenAI model |
|---|---|---|
| `gpt-4o` | `gpt-4o` | GPT-4o |
| `gpt-4o-mini` | `gpt-4o-mini-deployment` | GPT-4o mini |
| `my-custom-model` | `prod-gpt4-turbo` | GPT-4 Turbo |

### `base_url`

**Required.** The Azure resource endpoint URL. Found in the Azure portal under your OpenAI resource → Keys and Endpoint.

Format: `https://<resource-name>.openai.azure.com/`

Rausu strips any trailing slash before constructing the deployment URL.

### `api_version`

The Azure OpenAI API version query parameter. Defaults to `2024-12-01-preview` when omitted.

Common values:
- `2024-12-01-preview` (default)
- `2025-01-01-preview`

See [Azure OpenAI API version documentation](https://learn.microsoft.com/en-us/azure/ai-services/openai/api-version-deprecation) for the full list.

## URL construction

Rausu constructs the upstream URL as:

```
{base_url}/openai/deployments/{deployment_name}/chat/completions?api-version={api_version}
```

For example, with:
- `base_url: https://my-resource.openai.azure.com/`
- `model: gpt-4o` (deployment name)
- `api_version: 2024-12-01-preview`

The resulting URL is:
```
https://my-resource.openai.azure.com/openai/deployments/gpt-4o/chat/completions?api-version=2024-12-01-preview
```

The `model` field is **not** included in the request body sent to Azure — Azure determines the model from the deployment name in the URL path.

## Multi-provider failover

Azure OpenAI models can participate in Rausu's priority-based failover alongside other providers:

```yaml
- name: gpt-4o
  providers:
    - provider: azure-openai        # Try Azure first
      model: gpt-4o
      api_key: "${AZURE_OPENAI_API_KEY}"
      base_url: "https://my-resource.openai.azure.com/"
    - provider: openai              # Fall back to direct OpenAI
      model: gpt-4o
      api_key: "${OPENAI_API_KEY}"
```

## Capability-aware routing

The Azure OpenAI provider declares the following capabilities to Rausu's router:

| Capability | Declared |
|---|---|
| `chat_completions` | Yes |
| `streaming` | Yes (SSE) |
| `responses_api` | Yes (Responses -> Chat Completions bridge) |
| `tools` | Yes (passed through to Azure OpenAI) |
| `response_format` | Yes (passed through to Azure OpenAI) |

**How routing works:**

1. When a request arrives the router inspects it and determines which capabilities are required. A request containing `tools` requires `tools`; a request with `response_format` requires `response_format`.
2. Providers that lack any required capability are **skipped before any upstream call** is made.
3. If another configured provider for the same virtual model supports the required capabilities, failover continues there.
4. If **no** configured provider supports all required capabilities, Rausu returns a clear client-facing error instead of silently degrading or stripping fields.

### `unsupported_capability` error

When all providers for a model are skipped due to missing capabilities, Rausu returns:

- **HTTP status:** `422 Unprocessable Entity`
- **`error.type`:** `unsupported_capability`
- **`error.code`:** `unsupported_capability`
- **`error.message`:** names the missing capability or capabilities

Example response body:

```json
{
  "error": {
    "message": "No provider for model 'my-model' supports the required capabilities: tools",
    "type": "unsupported_capability",
    "code": "unsupported_capability"
  }
}
```

### No silent downgrade policy

Rausu does **not** silently strip `tools`, `tool_choice`, or `response_format` fields from requests on the Azure OpenAI path. If the selected upstream model does not support a requested capability, the upstream error is propagated to the client unchanged.

> **Note:** The capability declarations above reflect what the Azure OpenAI provider in Rausu exposes to the router. Actual capability support can still depend on the specific upstream deployment and model version.

## Docker deployment

```bash
docker run \
  -e AZURE_OPENAI_API_KEY="your-key" \
  -v /path/to/config.yaml:/app/config.yaml \
  -p 4000:4000 \
  rausu --config /app/config.yaml
```

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `401 Unauthorized` | Invalid or missing API key | Verify `AZURE_OPENAI_API_KEY` is set and valid |
| `404 Not Found` | Wrong deployment name or base URL | Check that `model` matches the Azure deployment name and `base_url` matches your resource endpoint |
| `400 Bad Request` with "api-version" | Missing or invalid API version | Set `api_version` to a valid Azure OpenAI API version |
| Startup error: `base_url is required` | No `base_url` configured | Add `base_url` pointing to your Azure resource endpoint |
| `429 Too Many Requests` | Rate limit exceeded | Reduce request rate or add another provider for failover |
| Model not found in Rausu | Typo in config `name` vs. client request | Ensure client sends the exact virtual `name` from config |

## Known limitations

- **No `/v1/messages` support.** Azure OpenAI uses the OpenAI-compatible format. For Anthropic Messages API passthrough, use `provider: anthropic` or `provider: claude-subscription`.
- **No native Responses API.** Rausu bridges Responses -> Chat Completions automatically.
- **`base_url` is required.** Unlike other OpenAI-compatible providers, Azure OpenAI has no default endpoint — you must provide your Azure resource URL.
- Rate limits and model availability are controlled by Azure. Rausu propagates upstream HTTP status codes unchanged.
- Tool/function calling is passed through as-is; no additional translation is performed. Capability depends on the upstream deployment.
