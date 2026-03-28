//! GET /v1/models endpoint.

use axum::{extract::State, Json};

use crate::schema::chat::{ModelInfo, ModelListResponse};
use crate::server::AppState;

/// GET /v1/models — list all configured models.
pub async fn list_models(State(state): State<AppState>) -> Json<ModelListResponse> {
    let data: Vec<ModelInfo> = state.providers.iter().flat_map(|p| p.models()).collect();

    Json(ModelListResponse {
        object: "list".to_string(),
        data,
    })
}
