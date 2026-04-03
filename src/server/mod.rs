//! HTTP server setup and route registration.

use std::collections::HashMap;
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
use tracing::{info, warn};

use std::path::PathBuf;

use crate::auth::chatgpt_oauth::{ensure_chatgpt_credentials, ChatGptOAuthTokenManager, ChatGptTokenSource};
use crate::auth::copilot::{ensure_copilot_credentials, CopilotTokenManager};
use crate::auth::oauth::{OAuthTokenManager, TokenSource};
use crate::auth::vertex::VertexTokenManager;
use crate::config::AppConfig;
use crate::providers::{
    anthropic::AnthropicProvider, chatgpt_subscription::ChatGptSubscriptionProvider,
    claude_subscription::ClaudeSubscriptionProvider, github_copilot::GitHubCopilotProvider,
    openai::OpenAiProvider, vertex_ai::VertexAiProvider, Provider,
};
use crate::schema::chat::ModelInfo;

pub mod routes;

use routes::{
    chat::chat_completions,
    health::health_check,
    messages::messages,
    models::list_models,
    responses::{responses, responses_compact},
};

/// Shared application state passed to all route handlers.
#[derive(Clone)]
pub struct AppState {
    /// Registered provider instances.
    pub providers: Arc<Vec<Box<dyn Provider>>>,
    /// Maps every known name/alias to `(provider_name, provider_model)`.
    pub model_registry: Arc<HashMap<String, (String, String)>>,
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
        let (providers, model_registry) = build_providers(&self.config).await;

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
            // Canonical /v1/ routes
            .route("/v1/models", get(list_models))
            .route("/v1/chat/completions", post(chat_completions))
            .route("/v1/responses", post(responses))
            .route("/v1/responses/compact", post(responses_compact))
            .route("/v1/messages", post(messages))
            // Compatibility routes without /v1/ prefix (e.g. Codex CLI)
            .route("/models", get(list_models))
            .route("/chat/completions", post(chat_completions))
            .route("/responses", post(responses))
            .route("/responses/compact", post(responses_compact))
            .route("/messages", post(messages))
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

/// Build provider instances from configuration.
///
/// Returns `(providers, model_registry)` where `model_registry` maps every
/// known name or alias to `(provider_name, provider_model)`.  When two
/// entries claim the same key the first one wins and a warning is logged.
async fn build_providers(
    config: &AppConfig,
) -> (Vec<Box<dyn Provider>>, HashMap<String, (String, String)>) {
    let mut providers: Vec<Box<dyn Provider>> = Vec::new();
    let mut model_registry: HashMap<String, (String, String)> = HashMap::new();

    // Collect model names per provider type
    let mut openai_models: Vec<(String, String, String)> = Vec::new(); // (virtual, api_key, model)
    let mut anthropic_models: Vec<(String, String, String)> = Vec::new();
    // (virtual_name, provider_model, token_source_str, credentials_path)
    let mut claude_sub_models: Vec<(String, String, String, Option<String>)> = Vec::new();
    // (virtual_name, provider_model, token_source_str, credentials_path)
    let mut chatgpt_sub_models: Vec<(String, String, String, Option<String>)> = Vec::new();
    // (virtual_name, provider_model, token_source_str, credentials_path)
    let mut copilot_models: Vec<(String, String, String, Option<String>)> = Vec::new();
    // (virtual_name, provider_model, project_id, location, credentials_path)
    let mut vertex_models: Vec<(String, String, String, String, Option<String>)> = Vec::new();

    for model_cfg in &config.models {
        for deployment in &model_cfg.providers {
            let api_key = deployment.api_key.clone().unwrap_or_default();
            let registry_entry: Option<(String, String)> = match deployment.provider.as_str() {
                "openai" => {
                    openai_models.push((model_cfg.name.clone(), api_key, deployment.model.clone()));
                    Some(("openai".to_string(), deployment.model.clone()))
                }
                "anthropic" => {
                    anthropic_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                    ));
                    Some(("anthropic".to_string(), deployment.model.clone()))
                }
                "claude-subscription" => {
                    claude_sub_models.push((
                        model_cfg.name.clone(),
                        deployment.model.clone(),
                        deployment.token_source.clone().unwrap_or_default(),
                        deployment.credentials_path.clone(),
                    ));
                    Some(("claude-subscription".to_string(), deployment.model.clone()))
                }
                "chatgpt-subscription" => {
                    chatgpt_sub_models.push((
                        model_cfg.name.clone(),
                        deployment.model.clone(),
                        deployment.token_source.clone().unwrap_or_default(),
                        deployment.credentials_path.clone(),
                    ));
                    Some(("chatgpt-subscription".to_string(), deployment.model.clone()))
                }
                "github-copilot" => {
                    copilot_models.push((
                        model_cfg.name.clone(),
                        deployment.model.clone(),
                        deployment.token_source.clone().unwrap_or_default(),
                        deployment.credentials_path.clone(),
                    ));
                    Some(("github-copilot".to_string(), deployment.model.clone()))
                }
                "vertex-ai" => {
                    let project_id = deployment.project_id.clone().unwrap_or_default();
                    let location = deployment
                        .location
                        .clone()
                        .unwrap_or_else(|| "us-central1".to_string());
                    vertex_models.push((
                        model_cfg.name.clone(),
                        deployment.model.clone(),
                        project_id,
                        location,
                        deployment.credentials_path.clone(),
                    ));
                    Some(("vertex-ai".to_string(), deployment.model.clone()))
                }
                other => {
                    tracing::warn!(provider = %other, "Unknown provider type; skipping");
                    None
                }
            };

            if let Some(entry) = registry_entry {
                if model_registry.contains_key(&model_cfg.name) {
                    tracing::warn!(
                        name = %model_cfg.name,
                        "Duplicate model name in registry; first entry wins"
                    );
                } else {
                    // Also register any declared aliases so they resolve to the same entry.
                    if let Some(aliases) = &model_cfg.aliases {
                        for alias in aliases {
                            if model_registry.contains_key(alias) {
                                tracing::warn!(
                                    alias = %alias,
                                    model = %model_cfg.name,
                                    "Duplicate alias in registry; skipping"
                                );
                            } else {
                                model_registry.insert(alias.clone(), entry.clone());
                            }
                        }
                    }
                    model_registry.insert(model_cfg.name.clone(), entry);
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

    // Create one ChatGptSubscriptionProvider per unique (token_source, credentials_path) pair.
    if !chatgpt_sub_models.is_empty() {
        let mut by_source: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, _model, token_source_str, credentials_path) in &chatgpt_sub_models {
            let path_key = credentials_path.clone().unwrap_or_default();
            by_source
                .entry((token_source_str.clone(), path_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((token_source_str, credentials_path_str), model_names) in by_source {
            let token_source = match token_source_str.as_str() {
                "env" => ChatGptTokenSource::Env,
                "credentials_file" => ChatGptTokenSource::CredentialsFile,
                "codex" => ChatGptTokenSource::Codex,
                "device_flow" => ChatGptTokenSource::DeviceFlow,
                _ => ChatGptTokenSource::Auto,
            };
            let credentials_path = if credentials_path_str.is_empty() {
                None
            } else {
                Some(PathBuf::from(&credentials_path_str))
            };
            let token_manager = ChatGptOAuthTokenManager::new(token_source, credentials_path);
            if let Err(e) = ensure_chatgpt_credentials(&token_manager).await {
                warn!("ChatGPT login failed: {e:#}");
            }
            providers.push(Box::new(ChatGptSubscriptionProvider::new(
                token_manager,
                model_names,
            )));
        }
    }

    // Create one GitHubCopilotProvider per unique (token_source, credentials_path) pair.
    if !copilot_models.is_empty() {
        let mut by_source: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, _model, token_source_str, credentials_path) in &copilot_models {
            let path_key = credentials_path.clone().unwrap_or_default();
            by_source
                .entry((token_source_str.clone(), path_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((_token_source_str, credentials_path_str), model_names) in by_source {
            let hosts_path = if credentials_path_str.is_empty() {
                None
            } else {
                Some(PathBuf::from(&credentials_path_str))
            };
            let token_manager = CopilotTokenManager::new(hosts_path);
            if let Err(e) = ensure_copilot_credentials(&token_manager).await {
                warn!("GitHub Copilot login failed: {e:#}");
            }
            providers.push(Box::new(GitHubCopilotProvider::new(
                token_manager,
                model_names,
            )));
        }
    }

    // Create one VertexAiProvider per unique (project_id, location, credentials_path) tuple.
    if !vertex_models.is_empty() {
        // Key: (project_id, location, credentials_path)
        let mut by_config: std::collections::HashMap<(String, String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, _model, project_id, location, credentials_path) in &vertex_models {
            let path_key = credentials_path.clone().unwrap_or_default();
            by_config
                .entry((project_id.clone(), location.clone(), path_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((project_id, location, credentials_path_str), model_names) in by_config {
            let credentials_path = if credentials_path_str.is_empty() {
                None
            } else {
                Some(PathBuf::from(&credentials_path_str))
            };
            let token_manager = VertexTokenManager::new(credentials_path);
            providers.push(Box::new(VertexAiProvider::new(
                token_manager,
                project_id,
                location,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        AppConfig, LoggingConfig, ModelConfig, ProviderDeployment, ServerConfig,
    };

    fn stub_deployment(provider: &str, model: &str) -> ProviderDeployment {
        ProviderDeployment {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: Some("test".to_string()),
            base_url: None,
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        }
    }

    fn minimal_config(models: Vec<ModelConfig>) -> AppConfig {
        AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            models,
        }
    }

    #[tokio::test]
    async fn test_alias_lookup_resolves_to_same_entry() {
        let config = minimal_config(vec![ModelConfig {
            name: "claude-haiku-4-5".to_string(),
            aliases: Some(vec!["claude-haiku-4-5-20251001".to_string()]),
            providers: vec![stub_deployment("anthropic", "claude-haiku-4-5-20251001")],
        }]);

        let (_, registry) = build_providers(&config).await;

        let primary = registry
            .get("claude-haiku-4-5")
            .expect("primary name missing");
        let alias = registry
            .get("claude-haiku-4-5-20251001")
            .expect("alias missing");
        assert_eq!(primary, alias);
        assert_eq!(primary.0, "anthropic");
        assert_eq!(primary.1, "claude-haiku-4-5-20251001");
    }

    #[tokio::test]
    async fn test_duplicate_alias_is_skipped_first_wins() {
        // Two models both declare the same alias — the first model's alias wins.
        let config = minimal_config(vec![
            ModelConfig {
                name: "model-a".to_string(),
                aliases: Some(vec!["shared-alias".to_string()]),
                providers: vec![stub_deployment("anthropic", "model-a-upstream")],
            },
            ModelConfig {
                name: "model-b".to_string(),
                aliases: Some(vec!["shared-alias".to_string()]),
                providers: vec![stub_deployment("anthropic", "model-b-upstream")],
            },
        ]);

        let (_, registry) = build_providers(&config).await;

        // Both primary names present
        assert!(registry.contains_key("model-a"));
        assert!(registry.contains_key("model-b"));

        // shared-alias resolves to model-a (first wins)
        let entry = registry.get("shared-alias").expect("shared-alias missing");
        assert_eq!(entry.1, "model-a-upstream");
    }
}
