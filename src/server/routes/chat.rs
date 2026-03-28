//! POST /v1/chat/completions endpoint.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response, Sse},
    Json,
};
use futures::StreamExt;
use tracing::{error, info, instrument, warn};

use crate::providers::ProviderError;
use crate::schema::chat::ChatCompletionRequest;
use crate::schema::error::ErrorResponse;
use crate::server::AppState;

/// POST /v1/chat/completions — proxy chat completion requests.
#[instrument(skip(state, req), fields(model = %req.model, stream = req.stream.unwrap_or(false)))]
pub async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionRequest>,
) -> Response {
    let model_name = req.model.clone();
    let is_stream = req.stream.unwrap_or(false);

    // Find the provider for the requested model
    let provider_info = state
        .model_registry
        .iter()
        .find(|(virtual_name, _, _)| virtual_name == &model_name);

    let (provider_name, provider_model) = match provider_info {
        Some((_, pname, pmodel)) => (pname.clone(), pmodel.clone()),
        None => {
            // Fall back: try to find any provider that lists this model
            let found = state
                .providers
                .iter()
                .find(|p| p.models().iter().any(|m| m.id == model_name));
            match found {
                Some(p) => (p.name().to_string(), model_name.clone()),
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

    // Resolve the upstream provider
    let provider = match state.providers.iter().find(|p| p.name() == provider_name) {
        Some(p) => p,
        None => {
            error!(provider = %provider_name, "Provider not found in registry");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse::internal("Provider not configured"),
            );
        }
    };

    // Replace the virtual model name with the upstream model name
    let mut upstream_req = req;
    upstream_req.model = provider_model;

    if is_stream {
        handle_streaming(provider.as_ref(), upstream_req).await
    } else {
        handle_non_streaming(provider.as_ref(), upstream_req).await
    }
}

/// Handle a non-streaming completion request.
async fn handle_non_streaming(
    provider: &dyn crate::providers::Provider,
    req: ChatCompletionRequest,
) -> Response {
    match provider.chat_completions(req).await {
        Ok(resp) => {
            info!(
                provider = provider.name(),
                model = %resp.model,
                prompt_tokens = resp.usage.prompt_tokens,
                completion_tokens = resp.usage.completion_tokens,
                "Non-streaming completion successful"
            );
            Json(resp).into_response()
        }
        Err(e) => {
            error!(provider = provider.name(), error = %e, "Provider error");
            map_provider_error(e)
        }
    }
}

/// Handle a streaming completion request.
async fn handle_streaming(
    provider: &dyn crate::providers::Provider,
    req: ChatCompletionRequest,
) -> Response {
    let stream_result = provider.chat_completions_stream(req).await;

    match stream_result {
        Err(e) => {
            error!(provider = provider.name(), error = %e, "Provider streaming error");
            map_provider_error(e)
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

            Sse::new(combined)
                .keep_alive(axum::response::sse::KeepAlive::default())
                .into_response()
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
