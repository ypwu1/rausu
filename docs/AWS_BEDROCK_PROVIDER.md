# AWS Bedrock Provider

> **中文版:** [AWS_BEDROCK_PROVIDER_CN.md](AWS_BEDROCK_PROVIDER_CN.md)

## Overview

The `bedrock` provider routes requests to [AWS Bedrock](https://aws.amazon.com/bedrock/) through the **Converse API**, which provides a unified interface for all Bedrock-hosted models (Anthropic Claude, Amazon Nova/Titan, Meta Llama, Mistral, Cohere, etc.).

This provider translates between OpenAI Chat Completions format and the Bedrock Converse API format, including:

- **Messages:** OpenAI roles → Bedrock `User`/`Assistant` messages with separate `system` field
- **Tool calling:** OpenAI `tools` / `tool_choice` ↔ Bedrock `toolConfig` / `toolChoice`
- **Streaming:** AWS EventStream binary encoding → SSE `ChatCompletionChunk` format
- **Inference config:** `temperature`, `max_tokens`, `top_p`, `stop` → Bedrock `inferenceConfig`

**Auth:** AWS SigV4 request signing via the standard AWS SDK credential chain — no API key needed in config.

**Responses API bridge:** When clients send Responses API requests (`/v1/responses`) to a Bedrock-backed model, Rausu automatically bridges Responses → Chat Completions format.

## Support matrix

| Endpoint | Support |
|---|---|
| `POST /v1/chat/completions` | Streaming + non-streaming (via Converse API) |
| `POST /v1/responses` | Responses → ChatCompletions bridge |
| `GET /v1/models` | Lists configured model names |
| `POST /v1/messages` | Not supported (use `provider: anthropic`) |

## Prerequisites

1. An AWS account with [Bedrock model access](https://docs.aws.amazon.com/bedrock/latest/userguide/model-access.html) enabled for the desired models.
2. AWS credentials available through one of:
   - **Environment variables:** `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` (+ optional `AWS_SESSION_TOKEN`)
   - **Shared credentials file:** `~/.aws/credentials`
   - **IAM role:** Automatic on EC2, ECS, Lambda, etc.
   - **AWS SSO:** Via `aws sso login` + `AWS_PROFILE`

## Authentication

No `api_key` field is needed. The AWS SDK handles credential resolution automatically.

```bash
# Option 1: Environment variables
export AWS_ACCESS_KEY_ID="AKIA..."
export AWS_SECRET_ACCESS_KEY="..."
export AWS_REGION="us-east-1"

# Option 2: AWS CLI profile
export AWS_PROFILE="my-bedrock-profile"

# Option 3: IAM role (automatic on EC2/ECS/Lambda — no config needed)
```

## Quick start

### 1. Add to config.yaml

```yaml
server:
  host: 127.0.0.1
  port: 4000

models:
  - name: claude-3-5-sonnet
    providers:
      - provider: bedrock
        model: anthropic.claude-3-5-sonnet-20241022-v2:0
        region: us-east-1
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
    "model": "claude-3-5-sonnet",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Using with Codex CLI

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="unused"
codex --model claude-3-5-sonnet
```

## Using with Claude Code

```bash
export OPENAI_BASE_URL="http://localhost:4000/v1"
export OPENAI_API_KEY="unused"
claude --model claude-3-5-sonnet
```

## Using Responses API

```bash
curl -s http://localhost:4000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-3-5-sonnet",
    "input": "Explain AWS Bedrock in one sentence."
  }'
```

## Configuration reference

```yaml
providers:
  - provider: bedrock            # Required: provider type
    model: <bedrock-model-id>    # Required: Bedrock model ID
    region: <aws-region>         # Required: AWS region (e.g. us-east-1)
```

| Field | Required | Description |
|---|---|---|
| `provider` | Yes | Must be `"bedrock"` |
| `model` | Yes | Bedrock model ID (see below) |
| `region` | Yes | AWS region where the model is available |

## Model ID format

Bedrock model IDs follow the pattern `<vendor>.<model-name>-<version>:<revision>`:

| Vendor | Example Model IDs |
|---|---|
| Anthropic | `anthropic.claude-3-5-sonnet-20241022-v2:0`, `anthropic.claude-3-5-haiku-20241022-v1:0` |
| Amazon | `amazon.nova-pro-v1:0`, `amazon.nova-lite-v1:0`, `amazon.titan-text-premier-v1:0` |
| Meta | `meta.llama3-1-70b-instruct-v1:0`, `meta.llama3-1-8b-instruct-v1:0` |
| Mistral | `mistral.mistral-large-2407-v1:0` |

## Request/response format translation

### Messages

| OpenAI format | Bedrock Converse format |
|---|---|
| `role: "system"` messages | Separate `system` parameter with `SystemContentBlock::Text` |
| `role: "user"` messages | `Message { role: User, content: [ContentBlock::Text] }` |
| `role: "assistant"` messages | `Message { role: Assistant, content: [ContentBlock::Text] }` |
| `role: "assistant"` with `tool_calls` | `Message { role: Assistant, content: [ContentBlock::ToolUse] }` |
| `role: "tool"` messages | `Message { role: User, content: [ContentBlock::ToolResult] }` |

### Tool calling

| OpenAI format | Bedrock format |
|---|---|
| `tools[].function.{name, description, parameters}` | `toolConfig.tools[].toolSpec.{name, description, inputSchema}` |
| `tool_choice: "auto"` | `toolChoice: Auto` |
| `tool_choice: "required"` | `toolChoice: Any` |
| `tool_choice: {type: "function", function: {name}}` | `toolChoice: Tool {name}` |
| `tool_choice: "none"` | Tool config omitted |

### Stop reason mapping

| Bedrock `stopReason` | OpenAI `finish_reason` |
|---|---|
| `EndTurn` | `stop` |
| `ToolUse` | `tool_calls` |
| `MaxTokens` | `length` |
| `StopSequence` | `stop` |
| `ContentFiltered` | `content_filter` |

## Multi-provider failover

```yaml
models:
  - name: claude-sonnet
    providers:
      - provider: bedrock
        model: anthropic.claude-3-5-sonnet-20241022-v2:0
        region: us-east-1
      - provider: anthropic
        model: claude-3-5-sonnet-20241022
        api_key: "${ANTHROPIC_API_KEY}"
```

If Bedrock returns a 5xx, 429 (throttled), or connection error, Rausu automatically fails over to the next provider.

## Capability-aware routing

The `bedrock` provider declares these capabilities:

| Capability | Declared |
|---|---|
| `chat_completions` | Yes |
| `streaming` | Yes |
| `responses_api` | Yes (bridged) |
| `tools` | Yes |
| `response_format` | No |
| `messages_api` | No |

Requests requiring undeclared capabilities (e.g. `response_format`) will receive a 422 error or fail over to a provider that supports them.

## Troubleshooting

### "region is required for bedrock"
Add `region: us-east-1` (or your desired region) to the provider config.

### "access denied" / 403
- Verify your AWS credentials are valid: `aws sts get-caller-identity`
- Ensure the IAM user/role has `bedrock:InvokeModel` and `bedrock:InvokeModelWithResponseStream` permissions
- Check that the model is enabled in the Bedrock console for your region

### "not found" / 404
- The model ID may be incorrect — check the [Bedrock model IDs](https://docs.aws.amazon.com/bedrock/latest/userguide/model-ids.html)
- The model may not be available in your region

### "throttled" / 429
- You've hit Bedrock's rate limit — Rausu will automatically retry with the next provider if configured

## Known limitations

- `response_format` (structured output / JSON mode) is not supported through the Converse API translation — use a provider that natively supports it if needed
- Image/vision content in messages is not translated (text-only content parts are extracted)
- The Bedrock Converse API has different token counting than OpenAI — reported usage numbers come from Bedrock
