//! HTTP server setup and route registration.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    http::Method,
    middleware,
    routing::{get, post},
    Router,
};
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

use std::path::PathBuf;

pub mod tls;

use crate::auth::chatgpt_oauth::{
    ensure_chatgpt_credentials, ChatGptOAuthTokenManager, ChatGptTokenSource,
};
use crate::auth::copilot::{ensure_copilot_credentials, CopilotTokenManager};
use crate::auth::middleware::{auth_middleware, AuthState};
use crate::auth::oauth::{OAuthTokenManager, TokenSource};
use crate::auth::vertex::VertexTokenManager;
use crate::config::AppConfig;
use crate::providers::{
    anthropic::AnthropicProvider, azure_openai::AzureOpenAiProvider,
    chatgpt_subscription::ChatGptSubscriptionProvider,
    claude_subscription::ClaudeSubscriptionProvider, deepseek::DeepSeekProvider,
    github_copilot::GitHubCopilotProvider, google_ai_studio::GoogleAiStudioProvider,
    minimax::MiniMaxProvider, moonshot::MoonshotProvider, openai::OpenAiProvider,
    openrouter::OpenRouterProvider, vertex_ai::VertexAiProvider, zai::ZaiProvider, Provider,
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
    /// Maps every known name/alias to an ordered list of `(provider_name, provider_model)`.
    /// The order is the priority order for failover (first = highest priority).
    pub model_registry: Arc<HashMap<String, Vec<(String, String)>>>,
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

        // Build auth state from config
        let auth_state = if self.config.auth.mode == "static" {
            let key_count = self.config.auth.keys.len();
            info!(
                auth_mode = "static",
                key_count = key_count,
                "API key authentication enabled"
            );
            AuthState::from_keys(self.config.auth.keys.iter().map(|k| k.key.clone()))
        } else {
            warn!(
                auth_mode = "disabled",
                "No authentication configured — proxy is open"
            );
            AuthState::disabled()
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
            .route_layer(middleware::from_fn_with_state(
                auth_state.clone(),
                auth_middleware,
            ))
            .layer(cors)
            .with_state(state);

        let bind_addr = format!("{}:{}", self.config.server.host, self.config.server.port);
        let listener = TcpListener::bind(&bind_addr).await?;

        if let Some(tls_config) = &self.config.server.tls {
            // ── TLS / mTLS path ────────────────────────────────────────────
            let mtls = tls_config.client_ca_file.is_some();
            let rustls_config = tls::build_rustls_server_config(tls_config)?;
            let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(rustls_config));

            info!(address = %bind_addr, tls = true, mtls = mtls, "Server listening");

            // Graceful shutdown coordination
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            let shutdown_handle = tokio::spawn(async move {
                shutdown_signal().await;
                let _ = shutdown_tx.send(true);
            });

            loop {
                let mut rx = shutdown_rx.clone();
                let accept = tokio::select! {
                    result = listener.accept() => result,
                    _ = async { while !*rx.borrow_and_update() { rx.changed().await.ok(); } } => break,
                };

                let (tcp_stream, _remote_addr) = match accept {
                    Ok(conn) => conn,
                    Err(e) => {
                        warn!(error = %e, "TCP accept error");
                        continue;
                    }
                };

                let acceptor = tls_acceptor.clone();
                let app = app.clone();
                let mut rx = shutdown_rx.clone();

                tokio::spawn(async move {
                    let tls_stream = match acceptor.accept(tcp_stream).await {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::debug!(error = %e, "TLS handshake failed");
                            return;
                        }
                    };

                    let io = hyper_util::rt::TokioIo::new(tls_stream);
                    let service = hyper_util::service::TowerToHyperService::new(app);

                    let builder = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    );
                    let conn = builder.serve_connection_with_upgrades(io, service);

                    tokio::pin!(conn);
                    tokio::select! {
                        result = &mut conn => {
                            if let Err(e) = result {
                                tracing::debug!(error = %e, "Connection error");
                            }
                        }
                        _ = async { while !*rx.borrow_and_update() { rx.changed().await.ok(); } } => {
                            conn.as_mut().graceful_shutdown();
                            if let Err(e) = conn.await {
                                tracing::debug!(error = %e, "Connection error during shutdown");
                            }
                        }
                    }
                });
            }

            shutdown_handle.abort();
        } else {
            // ── Plain HTTP path ────────────────────────────────────────────
            info!(address = %bind_addr, tls = false, "Server listening");

            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
        }

        info!("Server shutdown complete");
        Ok(())
    }
}

/// Build provider instances from configuration.
///
/// Returns `(providers, model_registry)` where `model_registry` maps every
/// known name or alias to an ordered list of `(provider_name, provider_model)`.
/// The order follows the config YAML (first = highest priority for failover).
async fn build_providers(
    config: &AppConfig,
) -> (
    Vec<Box<dyn Provider>>,
    HashMap<String, Vec<(String, String)>>,
) {
    let mut providers: Vec<Box<dyn Provider>> = Vec::new();
    let mut model_registry: HashMap<String, Vec<(String, String)>> = HashMap::new();

    // Collect model names per provider type
    let mut openai_models: Vec<(String, String, String, Option<String>)> = Vec::new(); // (virtual, api_key, model, base_url)
    let mut openrouter_models: Vec<(String, String, String, Option<String>)> = Vec::new(); // (virtual, api_key, model, base_url)
    let mut minimax_models: Vec<(String, String, String, Option<String>)> = Vec::new(); // (virtual, api_key, model, base_url)
    let mut anthropic_models: Vec<(String, String, String)> = Vec::new();
    // (virtual_name, provider_model, token_source_str, credentials_path)
    let mut claude_sub_models: Vec<(String, String, String, Option<String>)> = Vec::new();
    // (virtual_name, provider_model, token_source_str, credentials_path)
    let mut chatgpt_sub_models: Vec<(String, String, String, Option<String>)> = Vec::new();
    // (virtual_name, provider_model, token_source_str, credentials_path)
    let mut copilot_models: Vec<(String, String, String, Option<String>)> = Vec::new();
    let mut zai_models: Vec<(String, String, String, Option<String>)> = Vec::new(); // (virtual, api_key, model, base_url)
    let mut moonshot_models: Vec<(String, String, String, Option<String>)> = Vec::new(); // (virtual, api_key, model, base_url)
    let mut deepseek_models: Vec<(String, String, String, Option<String>)> = Vec::new(); // (virtual, api_key, model, base_url)
    let mut google_ai_studio_models: Vec<(String, String, String, Option<String>)> = Vec::new(); // (virtual, api_key, model, base_url)
                                                                                                 // (virtual, api_key, model, base_url, api_version)
    let mut azure_openai_models: Vec<(String, String, String, Option<String>, Option<String>)> =
        Vec::new();
    // (virtual_name, provider_model, project_id, location, credentials_path)
    let mut vertex_models: Vec<(String, String, String, String, Option<String>)> = Vec::new();

    for model_cfg in &config.models {
        for deployment in &model_cfg.providers {
            let api_key = deployment.api_key.clone().unwrap_or_default();
            let registry_entry: Option<(String, String)> = match deployment.provider.as_str() {
                "openai" => {
                    openai_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                        deployment.base_url.clone(),
                    ));
                    Some(("openai".to_string(), deployment.model.clone()))
                }
                "openrouter" => {
                    openrouter_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                        deployment.base_url.clone(),
                    ));
                    Some(("openrouter".to_string(), deployment.model.clone()))
                }
                "minimax" => {
                    minimax_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                        deployment.base_url.clone(),
                    ));
                    Some(("minimax".to_string(), deployment.model.clone()))
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
                "z-ai" => {
                    zai_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                        deployment.base_url.clone(),
                    ));
                    Some(("z-ai".to_string(), deployment.model.clone()))
                }
                "moonshot" => {
                    moonshot_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                        deployment.base_url.clone(),
                    ));
                    Some(("moonshot".to_string(), deployment.model.clone()))
                }
                "deepseek" => {
                    deepseek_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                        deployment.base_url.clone(),
                    ));
                    Some(("deepseek".to_string(), deployment.model.clone()))
                }
                "google-ai-studio" => {
                    google_ai_studio_models.push((
                        model_cfg.name.clone(),
                        api_key,
                        deployment.model.clone(),
                        deployment.base_url.clone(),
                    ));
                    Some(("google-ai-studio".to_string(), deployment.model.clone()))
                }
                "azure-openai" => {
                    if deployment.base_url.as_ref().is_none_or(|u| u.is_empty()) {
                        tracing::error!(
                            model = %model_cfg.name,
                            "azure-openai requires base_url (e.g. https://<resource>.openai.azure.com/); skipping"
                        );
                        None
                    } else {
                        azure_openai_models.push((
                            model_cfg.name.clone(),
                            api_key,
                            deployment.model.clone(),
                            deployment.base_url.clone(),
                            deployment.api_version.clone(),
                        ));
                        Some(("azure-openai".to_string(), deployment.model.clone()))
                    }
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
                // Accumulate providers for this model name in priority order.
                model_registry
                    .entry(model_cfg.name.clone())
                    .or_default()
                    .push(entry);
            }
        }

        // Register aliases pointing to the same provider list.
        if let Some(aliases) = &model_cfg.aliases {
            if let Some(provider_list) = model_registry.get(&model_cfg.name) {
                let provider_list = provider_list.clone();
                for alias in aliases {
                    if model_registry.contains_key(alias) {
                        tracing::warn!(
                            alias = %alias,
                            model = %model_cfg.name,
                            "Duplicate alias in registry; skipping"
                        );
                    } else {
                        model_registry.insert(alias.clone(), provider_list.clone());
                    }
                }
            }
        }
    }

    // Create one OpenAI provider per unique (api_key, base_url) pair.
    // This ensures DeepSeek, Ollama, and custom OpenAI-compatible providers
    // each get their own correctly-configured provider instance.
    if !openai_models.is_empty() {
        let mut by_key: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model, base_url) in &openai_models {
            let url_key = base_url.clone().unwrap_or_default();
            by_key
                .entry((api_key.clone(), url_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((api_key, url_key), model_names) in by_key {
            let base_url = if url_key.is_empty() {
                None
            } else {
                Some(url_key)
            };
            providers.push(Box::new(OpenAiProvider::new(
                api_key,
                base_url,
                model_names,
            )));
        }
    }

    // Create one OpenRouter provider per unique (api_key, base_url) pair.
    if !openrouter_models.is_empty() {
        let mut by_key: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model, base_url) in &openrouter_models {
            let url_key = base_url.clone().unwrap_or_default();
            by_key
                .entry((api_key.clone(), url_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((api_key, url_key), model_names) in by_key {
            let base_url = if url_key.is_empty() {
                None
            } else {
                Some(url_key)
            };
            providers.push(Box::new(OpenRouterProvider::new(
                api_key,
                base_url,
                model_names,
            )));
        }
    }

    // Create one MiniMax provider per unique (api_key, base_url) pair.
    if !minimax_models.is_empty() {
        let mut by_key: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model, base_url) in &minimax_models {
            let url_key = base_url.clone().unwrap_or_default();
            by_key
                .entry((api_key.clone(), url_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((api_key, url_key), model_names) in by_key {
            let base_url = if url_key.is_empty() {
                None
            } else {
                Some(url_key)
            };
            providers.push(Box::new(MiniMaxProvider::new(
                api_key,
                base_url,
                model_names,
            )));
        }
    }

    // Create one Z.AI provider per unique (api_key, base_url) pair.
    if !zai_models.is_empty() {
        let mut by_key: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model, base_url) in &zai_models {
            let url_key = base_url.clone().unwrap_or_default();
            by_key
                .entry((api_key.clone(), url_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((api_key, url_key), model_names) in by_key {
            let base_url = if url_key.is_empty() {
                None
            } else {
                Some(url_key)
            };
            providers.push(Box::new(ZaiProvider::new(api_key, base_url, model_names)));
        }
    }

    // Create one Moonshot provider per unique (api_key, base_url) pair.
    if !moonshot_models.is_empty() {
        let mut by_key: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model, base_url) in &moonshot_models {
            let url_key = base_url.clone().unwrap_or_default();
            by_key
                .entry((api_key.clone(), url_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((api_key, url_key), model_names) in by_key {
            let base_url = if url_key.is_empty() {
                None
            } else {
                Some(url_key)
            };
            providers.push(Box::new(MoonshotProvider::new(
                api_key,
                base_url,
                model_names,
            )));
        }
    }

    // Create one DeepSeek provider per unique (api_key, base_url) pair.
    if !deepseek_models.is_empty() {
        let mut by_key: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model, base_url) in &deepseek_models {
            let url_key = base_url.clone().unwrap_or_default();
            by_key
                .entry((api_key.clone(), url_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((api_key, url_key), model_names) in by_key {
            let base_url = if url_key.is_empty() {
                None
            } else {
                Some(url_key)
            };
            providers.push(Box::new(DeepSeekProvider::new(
                api_key,
                base_url,
                model_names,
            )));
        }
    }

    // Create one Google AI Studio provider per unique (api_key, base_url) pair.
    if !google_ai_studio_models.is_empty() {
        let mut by_key: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for (virtual_name, api_key, _model, base_url) in &google_ai_studio_models {
            let url_key = base_url.clone().unwrap_or_default();
            by_key
                .entry((api_key.clone(), url_key))
                .or_default()
                .push(virtual_name.clone());
        }

        for ((api_key, url_key), model_names) in by_key {
            let base_url = if url_key.is_empty() {
                None
            } else {
                Some(url_key)
            };
            providers.push(Box::new(GoogleAiStudioProvider::new(
                api_key,
                base_url,
                model_names,
            )));
        }
    }

    // Create one Azure OpenAI provider per unique (api_key, base_url, deployment, api_version) tuple.
    // Each Azure deployment maps to a specific model, so we create one provider per deployment.
    if !azure_openai_models.is_empty() {
        for (virtual_name, api_key, deployment_name, base_url, api_version) in azure_openai_models {
            let base_url = base_url.expect("azure-openai base_url validated above");
            providers.push(Box::new(AzureOpenAiProvider::new(
                api_key,
                base_url,
                deployment_name,
                api_version,
                vec![virtual_name],
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
        AppConfig, AuthConfig, LoggingConfig, ModelConfig, ProviderDeployment, ServerConfig,
    };

    fn stub_deployment(provider: &str, model: &str) -> ProviderDeployment {
        ProviderDeployment {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: Some("test".to_string()),
            base_url: None,
            token_source: None,
            credentials_path: None,
            api_version: None,
            project_id: None,
            location: None,
        }
    }

    fn minimal_config(models: Vec<ModelConfig>) -> AppConfig {
        AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
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
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].0, "anthropic");
        assert_eq!(primary[0].1, "claude-haiku-4-5-20251001");
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
        assert_eq!(entry[0].1, "model-a-upstream");
    }

    #[tokio::test]
    async fn test_multiple_providers_per_model() {
        let config = minimal_config(vec![ModelConfig {
            name: "claude-sonnet".to_string(),
            aliases: None,
            providers: vec![
                stub_deployment("anthropic", "claude-sonnet-upstream-1"),
                stub_deployment("openai", "claude-sonnet-upstream-2"),
            ],
        }]);

        let (_, registry) = build_providers(&config).await;

        let entries = registry.get("claude-sonnet").expect("model name missing");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "anthropic");
        assert_eq!(entries[0].1, "claude-sonnet-upstream-1");
        assert_eq!(entries[1].0, "openai");
        assert_eq!(entries[1].1, "claude-sonnet-upstream-2");
    }

    #[tokio::test]
    async fn test_aliases_get_full_provider_list() {
        let config = minimal_config(vec![ModelConfig {
            name: "claude-sonnet".to_string(),
            aliases: Some(vec!["sonnet".to_string()]),
            providers: vec![
                stub_deployment("anthropic", "upstream-1"),
                stub_deployment("openai", "upstream-2"),
            ],
        }]);

        let (_, registry) = build_providers(&config).await;

        let primary = registry.get("claude-sonnet").expect("primary missing");
        let alias = registry.get("sonnet").expect("alias missing");
        assert_eq!(primary, alias);
        assert_eq!(primary.len(), 2);
    }

    // ── OpenAI base_url grouping tests ──────────────────────────────────────

    fn openai_deployment_with_url(
        model: &str,
        api_key: &str,
        base_url: Option<&str>,
    ) -> ProviderDeployment {
        ProviderDeployment {
            provider: "openai".to_string(),
            model: model.to_string(),
            api_key: Some(api_key.to_string()),
            base_url: base_url.map(|s| s.to_string()),
            token_source: None,
            credentials_path: None,
            api_version: None,
            project_id: None,
            location: None,
        }
    }

    #[tokio::test]
    async fn test_openai_different_base_urls_create_separate_providers() {
        // DeepSeek and default OpenAI should get separate provider instances
        let config = minimal_config(vec![
            ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![openai_deployment_with_url("gpt-4o", "sk-openai", None)],
            },
            ModelConfig {
                name: "deepseek-chat".to_string(),
                aliases: None,
                providers: vec![openai_deployment_with_url(
                    "deepseek-chat",
                    "sk-deep",
                    Some("https://api.deepseek.com/v1"),
                )],
            },
        ]);

        let (providers, registry) = build_providers(&config).await;

        // Should have 2 separate OpenAI provider instances
        let openai_providers: Vec<_> = providers.iter().filter(|p| p.name() == "openai").collect();
        assert_eq!(
            openai_providers.len(),
            2,
            "Different base_urls should create separate providers"
        );

        // Both models should be in the registry
        assert!(registry.contains_key("gpt-4o"));
        assert!(registry.contains_key("deepseek-chat"));
    }

    #[tokio::test]
    async fn test_openai_same_key_same_url_grouped() {
        // Two models with the same api_key and no base_url should share a provider
        let config = minimal_config(vec![
            ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![openai_deployment_with_url("gpt-4o", "sk-shared", None)],
            },
            ModelConfig {
                name: "gpt-4o-mini".to_string(),
                aliases: None,
                providers: vec![openai_deployment_with_url("gpt-4o-mini", "sk-shared", None)],
            },
        ]);

        let (providers, _) = build_providers(&config).await;

        let openai_providers: Vec<_> = providers.iter().filter(|p| p.name() == "openai").collect();
        assert_eq!(
            openai_providers.len(),
            1,
            "Same api_key and base_url should share a provider"
        );
    }

    #[tokio::test]
    async fn test_openai_same_key_different_url_separated() {
        // Same key but different base_urls should be separate
        let config = minimal_config(vec![
            ModelConfig {
                name: "model-a".to_string(),
                aliases: None,
                providers: vec![openai_deployment_with_url("model-a", "sk-key", None)],
            },
            ModelConfig {
                name: "model-b".to_string(),
                aliases: None,
                providers: vec![openai_deployment_with_url(
                    "model-b",
                    "sk-key",
                    Some("http://localhost:11434/v1"),
                )],
            },
        ]);

        let (providers, _) = build_providers(&config).await;

        let openai_providers: Vec<_> = providers.iter().filter(|p| p.name() == "openai").collect();
        assert_eq!(
            openai_providers.len(),
            2,
            "Same key but different base_url should create separate providers"
        );
    }
}
