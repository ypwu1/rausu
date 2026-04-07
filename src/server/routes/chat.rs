//! POST /v1/chat/completions endpoint.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response, Sse},
    Json,
};
use futures::StreamExt;
use tracing::{error, info, instrument, warn};

use crate::providers::{Capability, ProviderError};
use crate::schema::chat::ChatCompletionRequest;
use crate::schema::error::ErrorResponse;
use crate::server::AppState;

/// Determine which capabilities a chat completion request requires beyond basic
/// chat completions (which is always required).
fn required_capabilities(req: &ChatCompletionRequest) -> Vec<Capability> {
    let mut caps = Vec::new();
    if req.tools.is_some() {
        caps.push(Capability::Tools);
    }
    if req.response_format.is_some() {
        caps.push(Capability::ResponseFormat);
    }
    caps
}

/// POST /v1/chat/completions — proxy chat completion requests.
///
/// When multiple providers are configured for a model, they are tried in
/// priority order. Retryable errors (429, 5xx, transport failures) trigger
/// failover to the next provider.
#[instrument(skip(state, req), fields(model = %req.model, stream = req.stream.unwrap_or(false), provider = tracing::field::Empty))]
pub async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionRequest>,
) -> Response {
    let model_name = req.model.clone();
    let is_stream = req.stream.unwrap_or(false);

    // Find the provider list for the requested model
    let provider_list = match state.model_registry.get(&model_name) {
        Some(list) => list.clone(),
        None => {
            // Fall back: try to find any provider that lists this model
            let found = state
                .providers
                .iter()
                .find(|p| p.models().iter().any(|m| m.id == model_name));
            match found {
                Some(p) => vec![(p.name().to_string(), model_name.clone())],
                None => {
                    warn!(model = %model_name, "No provider found for model");
                    return error_response(
                        StatusCode::NOT_FOUND,
                        ErrorResponse::invalid_request(format!(
                            "Model '{}' not found. Check your configuration.",
                            model_name
                        )),
                    );
                }
            }
        }
    };

    let total_providers = provider_list.len();
    let mut providers_tried: Vec<String> = Vec::new();
    let extra_caps = required_capabilities(&req);
    let mut capability_skipped: usize = 0;

    for (attempt, (provider_name, provider_model)) in provider_list.iter().enumerate() {
        tracing::Span::current().record("provider", provider_name.as_str());

        // Resolve the upstream provider
        let provider = match state.providers.iter().find(|p| p.name() == *provider_name) {
            Some(p) => p,
            None => {
                error!(provider = %provider_name, "Provider not found in registry");
                continue;
            }
        };

        // Capability pre-check: skip providers that lack required capabilities
        let missing: Vec<&Capability> = extra_caps
            .iter()
            .filter(|c| !provider.has_capability(**c))
            .collect();
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|c| c.as_str()).collect();
            warn!(
                model = %model_name,
                provider = %provider_name,
                missing_capabilities = ?names,
                "Provider lacks required capabilities, skipping"
            );
            capability_skipped += 1;
            continue;
        }

        info!(model = %model_name, provider = %provider_name, attempt = attempt + 1, "Trying provider");
        providers_tried.push(provider_name.clone());

        // Replace the virtual model name with the upstream model name
        let mut upstream_req = req.clone();
        upstream_req.model = provider_model.clone();

        let result = if is_stream {
            handle_streaming(provider.as_ref(), upstream_req).await
        } else {
            handle_non_streaming(provider.as_ref(), upstream_req).await
        };

        match result {
            ProviderAttempt::Success(response) => {
                info!(model = %model_name, provider = %provider_name, "Request served by provider");
                return response;
            }
            ProviderAttempt::NonRetryableError(response) => {
                return response;
            }
            ProviderAttempt::RetryableError { status } => {
                if attempt + 1 < total_providers {
                    warn!(model = %model_name, provider = %provider_name, status, "Provider failed, falling back");
                    continue;
                }
                // Last provider — return the error
                return error_response(
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    ErrorResponse::internal(format!(
                        "All providers failed for model '{}'. Tried: {}",
                        model_name,
                        providers_tried.join(", ")
                    )),
                );
            }
        }
    }

    // All providers exhausted.
    if capability_skipped > 0 && providers_tried.is_empty() {
        // Every provider was skipped due to missing capabilities.
        let cap_names: Vec<&str> = extra_caps.iter().map(|c| c.as_str()).collect();
        warn!(model = %model_name, missing = ?cap_names, "No provider supports required capabilities");
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorResponse::unsupported_capability(format!(
                "No provider for model '{}' supports the required capabilities: {}",
                model_name,
                cap_names.join(", ")
            )),
        );
    }

    error!(model = %model_name, providers_tried = ?providers_tried, "All providers failed");
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorResponse::internal(format!(
            "All providers failed for model '{}'. Tried: {}",
            model_name,
            providers_tried.join(", ")
        )),
    )
}

/// Result of attempting a single provider for chat completions.
enum ProviderAttempt {
    Success(Response),
    RetryableError { status: u16 },
    NonRetryableError(Response),
}

/// Handle a non-streaming completion request.
async fn handle_non_streaming(
    provider: &dyn crate::providers::Provider,
    req: ChatCompletionRequest,
) -> ProviderAttempt {
    match provider.chat_completions(req).await {
        Ok(resp) => {
            info!(
                provider = provider.name(),
                model = %resp.model,
                prompt_tokens = resp.usage.prompt_tokens,
                completion_tokens = resp.usage.completion_tokens,
                "Non-streaming completion successful"
            );
            ProviderAttempt::Success(Json(resp).into_response())
        }
        Err(e) => {
            let status = e.status_code();
            if e.is_retryable() {
                error!(provider = provider.name(), error = %e, "Provider error (retryable)");
                ProviderAttempt::RetryableError { status }
            } else {
                error!(provider = provider.name(), error = %e, "Provider error");
                ProviderAttempt::NonRetryableError(map_provider_error(e))
            }
        }
    }
}

/// Handle a streaming completion request.
async fn handle_streaming(
    provider: &dyn crate::providers::Provider,
    req: ChatCompletionRequest,
) -> ProviderAttempt {
    let stream_result = provider.chat_completions_stream(req).await;

    match stream_result {
        Err(e) => {
            let status = e.status_code();
            if e.is_retryable() {
                error!(provider = provider.name(), error = %e, "Provider streaming error (retryable)");
                ProviderAttempt::RetryableError { status }
            } else {
                error!(provider = provider.name(), error = %e, "Provider streaming error");
                ProviderAttempt::NonRetryableError(map_provider_error(e))
            }
        }
        Ok(chunk_stream) => {
            // Convert chunk stream to SSE event stream
            let sse_stream = chunk_stream.map(|result| match result {
                Ok(chunk) => {
                    let data = serde_json::to_string(&chunk).unwrap_or_else(|_| "{}".to_string());
                    Ok::<axum::response::sse::Event, std::convert::Infallible>(
                        axum::response::sse::Event::default().data(data),
                    )
                }
                Err(e) => {
                    error!(error = %e, "Error in chunk stream");
                    let error_data = serde_json::to_string(&ErrorResponse::internal(e.to_string()))
                        .unwrap_or_else(|_| "{}".to_string());
                    Ok(axum::response::sse::Event::default().data(error_data))
                }
            });

            // Append [DONE] sentinel
            let done_event = futures::stream::once(async {
                Ok::<axum::response::sse::Event, std::convert::Infallible>(
                    axum::response::sse::Event::default().data("[DONE]"),
                )
            });

            let combined = sse_stream.chain(done_event);

            ProviderAttempt::Success(
                Sse::new(combined)
                    .keep_alive(axum::response::sse::KeepAlive::default())
                    .into_response(),
            )
        }
    }
}

/// Map a ProviderError to an HTTP response with an OpenAI-compatible error body.
fn map_provider_error(e: ProviderError) -> Response {
    let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = match &e {
        ProviderError::ProviderResponse { message, .. } => {
            ErrorResponse::new(message.clone(), "provider_error")
        }
        ProviderError::Http(he) => ErrorResponse::internal(he.to_string()),
        ProviderError::Serialisation(se) => ErrorResponse::internal(se.to_string()),
        ProviderError::Unsupported(msg) => ErrorResponse::invalid_request(msg.clone()),
        ProviderError::Internal(msg) => ErrorResponse::internal(msg.clone()),
    };
    error_response(status, body)
}

/// Build an error HTTP response.
fn error_response(status: StatusCode, body: ErrorResponse) -> Response {
    (status, Json(body)).into_response()
}
