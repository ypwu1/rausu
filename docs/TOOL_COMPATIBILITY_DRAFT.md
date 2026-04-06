# Tool Compatibility & Provider Capability Checking — Design Draft

> [中文版](TOOL_COMPATIBILITY_DRAFT_CN.md)

> **Status: Draft** — This document captures the agreed design direction. No implementation exists yet. Details may change as work progresses.

## Context

Rausu is a local-first LLM gateway. Clients like Claude Code and Codex CLI increasingly rely on tool calling — sending `tools` arrays, receiving `tool_use` / `function_call` blocks, and managing multi-turn tool loops. Different providers and endpoints support different subsets of these capabilities.

Today, Rausu's priority failover routing selects providers based on availability (retryable errors like 429, 5xx, transport failures). It does **not** consider whether a provider actually supports the features a request requires. A request with `tools` could be routed to a provider that silently ignores the tools array, or a streaming request could hit an endpoint that only supports synchronous responses.

This draft defines a **minimum tool compatibility layer** — not a tool execution runtime, but the awareness needed to route requests correctly and fail explicitly when capabilities are missing.

## Design Principles

1. **Tool-aware passthrough, not tool execution.** Rausu forwards tool definitions and tool call results between client and provider. It does not execute tools, host MCP servers, approve shell commands, or run agent loops.
2. **Explicit failure over silent degradation.** If a provider cannot satisfy a request's requirements, Rausu must return a clear error or fail over to another provider. It must **never** silently strip `tools`, `tool_choice`, `parallel_tool_calls`, or related fields from a request.
3. **Capability checking at routing time.** Before forwarding a request, Rausu evaluates whether the selected provider can handle it. This integrates with the existing priority failover loop.
4. **Translate when needed, not when convenient.** Protocol translation (e.g., `function_call` ↔ `tool_use`) happens at protocol boundaries. If client and provider speak the same protocol, passthrough is preferred.

## What Tool-Aware Passthrough Means

Tool-aware passthrough means Rausu understands the **structure** of tool-related fields well enough to:

- **Detect** that a request requires tool calling capabilities (presence of `tools`, `tool_choice`, `tool_results`, etc.)
- **Evaluate** whether the target provider supports those capabilities
- **Route** the request to a capable provider, or fail explicitly
- **Translate** tool-related fields when the request crosses a protocol boundary (e.g., OpenAI `function_call` ↔ Anthropic `tool_use`)
- **Preserve** all tool-related fields intact when no translation is needed

What it does **not** mean:

- Rausu does not call tools on behalf of the client
- Rausu does not validate tool argument schemas
- Rausu does not maintain tool execution state between requests
- Rausu does not implement MCP host capabilities

## Capability Model

### Capability Dimensions

Each provider-endpoint combination declares a set of capabilities. The minimum dimensions are:

| Dimension | Description | Example values |
|-----------|-------------|----------------|
| `messages_endpoint` | Supports Anthropic Messages API (`/v1/messages`) | `true` / `false` |
| `responses_endpoint` | Supports OpenAI Responses API (`/v1/responses`) | `true` / `false` |
| `chat_completions_endpoint` | Supports OpenAI Chat Completions API (`/v1/chat/completions`) | `true` / `false` |
| `streaming` | Supports SSE streaming responses | `true` / `false` |
| `tools` | Supports tool definitions and tool call responses | `true` / `false` |
| `parallel_tools` | Supports `parallel_tool_calls` (multiple tool calls in one turn) | `true` / `false` |
| `bridge_support` | Can be reached via protocol translation (e.g., Messages→Responses) | `true` / `false` |

These dimensions are **statically declared** per provider implementation, not discovered at runtime. Each provider module knows its own capabilities.

### Request Requirements

When a request arrives, Rausu extracts the **requirements** it implies:

| Request signal | Implies requirement |
|----------------|---------------------|
| `tools` array present and non-empty | `tools = true` |
| `parallel_tool_calls: true` | `parallel_tools = true` |
| `stream: true` | `streaming = true` |
| Arrived on `/v1/messages` | `messages_endpoint = true` (or `bridge_support = true` on target) |
| Arrived on `/v1/responses` | `responses_endpoint = true` (or `bridge_support = true` on target) |
| Arrived on `/v1/chat/completions` | `chat_completions_endpoint = true` (or `bridge_support = true` on target) |

### Evaluation Outcomes

When comparing request requirements against provider capabilities, the result is one of:

| Outcome | Meaning | Action |
|---------|---------|--------|
| **Pass** | Provider satisfies all requirements | Forward the request |
| **Soft fail** | Provider cannot satisfy this request, but another provider in the failover list might | Skip this provider, continue failover loop |
| **Hard fail** | No provider in the model's provider list can satisfy this request | Return `422 Unprocessable Entity` with a clear error: `{"error": {"type": "unsupported_capability", "message": "...", "missing_capabilities": [...]}}` |

### The No-Silent-Degradation Rule

This is a hard rule, not a guideline:

> **Rausu MUST NOT silently strip or modify tool-related fields to make a request fit a provider's capabilities.**

Examples of what this rule prohibits:

- Removing the `tools` array to send to a provider that doesn't support tools
- Setting `parallel_tool_calls: false` when the client sent `true`
- Dropping `tool_choice` because the target provider doesn't support it
- Silently converting tool calls into plain text messages

If a provider can't handle the request as-is (possibly after legitimate protocol translation), the request must fail over or hard fail — never silently degrade.

## Integration with Priority Failover Routing

Today's failover loop in `src/server/routes/chat.rs` works like this:

```
for each provider in priority order:
    try sending request
    if retryable error → try next provider
    if non-retryable error → return error
    if success → return response
```

Capability checking adds a **pre-flight step** before the network call:

```
for each provider in priority order:
    check provider capabilities against request requirements
    if soft fail → skip provider, log warning, try next
    try sending request
    if retryable error → try next provider
    if non-retryable error → return error
    if success → return response

if all providers exhausted → check if any were capability soft fails
    if yes → return 422 with unsupported_capability error
    if no → return 503 as today
```

This means:

- **Capability soft fails are retryable.** If provider A doesn't support tools but provider B does, the request routes to provider B. This works transparently with the existing failover model.
- **Capability checks happen before network calls.** No wasted round-trips to providers that can't handle the request.
- **Existing error classification is unchanged.** HTTP 429, 5xx, and transport errors still trigger failover as before. Capability checking is an additional, earlier gate.

### Logging

Capability-related routing decisions should be logged at the same levels as existing failover:

- `INFO`: "Checking capabilities for provider X"
- `WARN`: "Provider X does not support tools, skipping (soft fail)"
- `ERROR`: "No provider supports required capabilities: [tools, parallel_tools]"

## Why This Matters for Claude Code and Codex CLI

Claude Code sends requests via `/v1/messages` with `tools` arrays for file operations, shell commands, and other agent capabilities. If Rausu routes a Claude Code request to a provider that doesn't support tools, the agent loop breaks silently — the client expects tool call blocks in the response but gets plain text instead.

Codex CLI sends requests via `/v1/responses` with function definitions. The same problem applies: silent tool stripping means the client's agent loop stalls or produces incorrect results.

Both clients assume the gateway is transparent to tool semantics. Capability checking ensures Rausu upholds that assumption by either routing to a capable provider or failing explicitly so the client can surface a meaningful error to the user.

## Phased Implementation Plan

### Phase A: Documentation & Capability Model (this document)

- Define capability dimensions and evaluation outcomes
- Establish the no-silent-degradation rule
- Document integration points with existing routing

### Phase B: Request Requirement Extraction

- Parse incoming requests to extract capability requirements
- Detect `tools`, `parallel_tool_calls`, `stream`, and endpoint type
- Produce a `RequestRequirements` struct

### Phase C: Provider Capability Declaration

- Each provider implementation declares its capabilities via a method on the `Provider` trait (e.g., `fn capabilities(&self) -> ProviderCapabilities`)
- Capabilities are static per provider type, not per-request

### Phase D: Routing Integration

- Add capability pre-flight check to the failover loop
- Implement soft fail / hard fail logic
- Add structured error responses for unsupported capabilities
- Add capability-related log lines

### Phase E: Protocol-Specific Tool Translation (if needed)

- Extend the existing protocol bridge to handle tool-specific translation edge cases
- Ensure `function_call` ↔ `tool_use` conversion preserves all semantics
- Handle provider-specific tool calling quirks (e.g., different JSON schema formats)

## Non-Goals

The following are explicitly **not** in scope for this design:

- **Built-in tool execution** — Rausu does not run tools. It forwards tool definitions and results. Tool execution is the client's responsibility.
- **MCP host runtime** — Rausu does not host MCP servers, manage tool registries, or broker tool discovery. MCP gateway functionality is deferred to Phase 5.
- **Shell approval loop** — Rausu does not prompt users to approve or deny tool invocations. That is the client's UX responsibility.
- **Agent runtime** — Rausu does not implement multi-turn agent loops, tool result injection, or autonomous execution flows. It handles single request-response cycles.
- **Runtime capability discovery** — Capabilities are declared statically in code. Rausu does not probe providers at startup or query capability endpoints. This may change in the future but is out of scope for the initial implementation.
- **Tool argument validation** — Rausu does not validate that tool arguments match their declared JSON schemas. The provider and client handle this.

## Open Questions

- Should capability declarations be configurable per-model in YAML, or purely derived from provider type? Static-per-provider is simpler; per-model config handles edge cases where the same provider has different capability sets for different models.
- Should hard fail responses include a suggestion of which providers could satisfy the request, to aid debugging?
- What is the right HTTP status code for capability hard fails? `422 Unprocessable Entity` is semantically correct; `400 Bad Request` is more conventional for some clients.
