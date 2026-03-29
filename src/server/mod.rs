//! HTTP server setup and route registration.

use std::sync::Arc;

use anyhow::Result;
use axum::{
    http::Method,
    routing::{get, post},
    Router,
};
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use std::path::PathBuf;

use crate::auth::oauth::{OAuthTokenManager, TokenSource};
use crate::config::AppConfig;
use crate::providers::{
    anthropic::AnthropicProvider,
    claude_subscription::ClaudeSubscriptionProvider,
    openai::OpenAiProvider,
    Provider,
};
use crate::schema::chat::ModelInfo;

pub mod routes;

use routes::{chat::chat_completions, health::health_check, models::list_models};

/// Shared application state passed to all route handlers.
#[derive(Clone)]
pub struct AppState {
    /// Registered provider instances.
    pub providers: Arc<Vec<Box<dyn Provider>>>,
    /// All known model configurations (name → provider name mapping).
    pub model_registry: Arc<Vec<(String, String, String)>>, // (virtual_name, provider_name, provider_model)
}

/// The HTTP server.
pub struct Server {
    config: AppConfig,
}

impl Server {
    /// Create a new server from config.
    pub fn new(config: AppConfig) -> Result<Self> {
        Ok(Self { config })
    }

    /// Build the Axum router and run until shutdown.
    pub async fn run(self) -> Result<()> {
        let (providers, model_registry) = build_providers(&self.config);

        let state = AppState {
            providers: Arc::new(providers),
            model_registry: Arc::new(model_registry),
        };

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers(Any);

        let app = Router::new()
            .route("/health", get(health_check))
            .route("/v1/models", get(list_models))
            .route("/v1/chat/completions", post(chat_completions))
            .layer(cors)
            .with_state(state);

        let bind_addr = format!("{}:{}", self.config.server.host, self.config.server.port);
        let listener = TcpListener::bind(&bind_addr).await?;

        info!(address = %bind_addr, "Server listening");

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        info!("Server shutdown complete");
        Ok(())
    }
}

/// (virtual_name, provider_name, provider_model) model registry entry.
type ModelRegistryEntry = (String, String, String);

/// Build provider instances from configuration.
///
/// Returns (providers, model_registry) where model_registry is a list of
/// (virtual_name, provider_name, provider_model).
fn build_providers(config: &AppConfig) -> (Vec<Box<dyn Provider>>, Vec<ModelRegistryEntry>) {
    let mut providers: Vec<Box<dyn Provider>> = Vec::new();
    let mut model_registry: Vec<(String, String, String)> = Vec::new();

    // Collect model names per provider type
    let mut openai_models: Vec<(String, String, String)> = Vec::new(); // (virtual, api_key, model)
    let mut anthropic_models: Vec<(String, String, String)> = Vec::new();
    // (virtual_name, provider_model, token_source_str, credentials_path)
    let mut claude_sub_models: Vec<(String, String, String, Option<String>)> = Vec::new();

    for model_cfg in &config.models {
        for deployment in &model_cfg.providers {
            let api_key = deployment.api_key.clone().unwrap_or_default();
            match deployment.provider.as_str() {
                "openai" => {
                    openai_models.push((model_cfg.name.clone(), api_key, deployment.model.clone()));
                    model_registry.push((
                        model_cfg.name.clone(),
                        "openai".to_string(),
                        deployment.model.clone(),
                    ));
                }
                "anthropic" => {
                    anthropic_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                    ));
                    model_registry.push((
                        model_cfg.name.clone(),
                        "anthropic".to_string(),
                        deployment.model.clone(),
                    ));
                }
                "claude-subscription" => {
                    claude_sub_models.push((
                        model_cfg.name.clone(),
                        deployment.model.clone(),
                        deployment.token_source.clone().unwrap_or_default(),
                        deployment.credentials_path.clone(),
                    ));
                    model_registry.push((
                        model_cfg.name.clone(),
                        "claude-subscription".to_string(),
                        deployment.model.clone(),
                    ));
                }
                other => {
                    tracing::warn!(provider = %other, "Unknown provider type; skipping");
                }
            }
        }
    }

    // Create one OpenAI provider (reuse client, first api_key wins per model)
    if !openai_models.is_empty() {
        // Group by api_key — for MVP, create one provider per unique api_key
        let mut by_key: std::collections::HashMap<String, (Vec<String>, Option<String>)> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model) in &openai_models {
            let entry = by_key.entry(api_key.clone()).or_insert((Vec::new(), None));
            entry.0.push(virtual_name.clone());
        }

        for (api_key, (model_names, base_url)) in by_key {
            providers.push(Box::new(OpenAiProvider::new(
                api_key,
                base_url,
                model_names,
            )));
        }
    }

    // Create one Anthropic provider per unique api_key
    if !anthropic_models.is_empty() {
        let mut by_key: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model) in &anthropic_models {
            by_key
                .entry(api_key.clone())
                .or_default()
                .push(virtual_name.clone());
        }

        for (api_key, model_names) in by_key {
            providers.push(Box::new(AnthropicProvider::new(api_key, model_names)));
        }
    }

    // Create one ClaudeSubscriptionProvider per unique (token_source, credentials_path) pair.
    if !claude_sub_models.is_empty() {
        // Key: (token_source_str, credentials_path_str)
        let mut by_source: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, _model, token_source_str, credentials_path) in &claude_sub_models {
            let path_key = credentials_path.clone().unwrap_or_default();
            by_source
                .entry((token_source_str.clone(), path_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((token_source_str, credentials_path_str), model_names) in by_source {
            let token_source = match token_source_str.as_str() {
                "env" => TokenSource::Env,
                "credentials_file" => TokenSource::CredentialsFile,
                _ => TokenSource::Auto,
            };
            let credentials_path = if credentials_path_str.is_empty() {
                None
            } else {
                Some(PathBuf::from(&credentials_path_str))
            };
            let token_manager = OAuthTokenManager::new(token_source, credentials_path);
            providers.push(Box::new(ClaudeSubscriptionProvider::new(
                token_manager,
                model_names,
            )));
        }
    }

    (providers, model_registry)
}

/// Graceful shutdown signal handler (SIGTERM or Ctrl-C).
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received");
}

/// Collect all model infos from all providers.
#[allow(dead_code)]
pub fn collect_model_infos(providers: &[Box<dyn Provider>]) -> Vec<ModelInfo> {
    providers.iter().flat_map(|p| p.models()).collect()
}
