# Interactive Config Editor (`rausu setup`)

`rausu setup` is an interactive, model-centric configuration editor that runs in your terminal. It can create new configs from scratch or load and edit existing ones.

## Quick Start

```bash
# Create or edit config at the default location
rausu setup

# Edit a specific config file
rausu setup --path /path/to/config.yaml
```

## Features

- **Load existing configs** — if a config file already exists, it is loaded into the editor automatically
- **Model-centric flow** — create a virtual model first, then attach one or more provider deployments
- **Multi-provider failover** — each model can have multiple providers; order determines failover priority
- **Full CRUD** — add, edit, delete, and reorder models and their provider deployments
- **Pre-save validation** — the shared validation engine checks for errors before writing
- **All config sections** — models, auth, server, TLS, and logging are all editable

## Top-Level Menu

When you launch `rausu setup`, you see:

```
Configuration section:
> Models
  Auth
  Server
  TLS
  Logging
  Validate
  Save and Exit
  Exit without Saving
```

## Models Workflow

### Creating a Model

1. Choose **Models** from the top menu
2. Select **+ Add model**
3. Enter the virtual model name (e.g. `gpt-4o`)
4. Optionally add aliases (e.g. `gpt-4, gpt4o`)
5. Select one or more providers (order = failover priority)
6. Configure each provider with its required fields

### Editing a Model

Select an existing model to:
- **Edit name** — change the virtual model name
- **Edit aliases** — modify or clear alias list
- **View/edit provider deployments** — select any provider to edit or delete it
- **Add provider deployment** — attach another provider for failover
- **Reorder providers** — move a provider up or down in priority
- **Delete model** — remove the model entirely

## Supported Providers

| Provider | Required Fields |
|---|---|
| GitHub Copilot | upstream model name |
| ChatGPT Subscription | upstream model name, token source |
| Claude Subscription | upstream model name, token source |
| OpenAI API | upstream model name, API key, optional base URL |
| Anthropic API | upstream model name, API key |
| Vertex AI | upstream model name, GCP project ID, location |

Custom OpenAI-compatible providers (DeepSeek, Ollama, etc.) use the OpenAI provider type with a custom `base_url`.

## Validation

Select **Validate** from the top menu to run the shared validation engine. It reports:

- **Errors** (block startup): unknown provider types, missing required fields, empty model names, duplicate names/aliases, invalid token sources
- **Warnings** (informational): missing API keys, missing credential files, no models configured

The same validation runs automatically when you choose **Save and Exit**.

## Example: Multi-Provider Model with Failover

```
Models > + Add model
  Virtual model name: claude-sonnet
  Aliases: sonnet
  Providers: Anthropic API, GitHub Copilot

  Configuring Anthropic API for 'claude-sonnet':
    Upstream model: claude-sonnet-4-6
    API key: ****

  Configuring GitHub Copilot for 'claude-sonnet':
    Upstream model: claude-sonnet-4-6
```

This creates a model `claude-sonnet` (alias `sonnet`) that tries Anthropic first, then falls back to GitHub Copilot.
