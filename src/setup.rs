//! `rausu setup` — interactive config editor.
//!
//! A model-centric interactive editor that can create new configs from scratch
//! or load and edit existing ones.  Uses `inquire` for terminal prompts.

use anyhow::Result;
use inquire::{Confirm, InquireError, MultiSelect, Password, PasswordDisplayMode, Select, Text};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use rausu::config::paths::{default_config_path, resolve_config_path};
use rausu::config::schema::{
    AppConfig, AuthConfig, AuthKey, LoggingConfig, ModelConfig, ProviderDeployment, ServerConfig,
    TlsConfig,
};
use rausu::validation::{self, Severity};

// ── Provider catalogue ───────────────────────────────────────────────────────

const PROVIDER_TYPES: &[&str] = &[
    "github-copilot",
    "chatgpt-subscription",
    "claude-subscription",
    "openai",
    "openrouter",
    "anthropic",
    "azure-openai",
    "vertex-ai",
];

fn provider_display(p: &str) -> &str {
    match p {
        "github-copilot" => "GitHub Copilot (no API key needed)",
        "chatgpt-subscription" => "ChatGPT Subscription (no API key needed)",
        "claude-subscription" => "Claude Subscription (no API key needed)",
        "openai" => "OpenAI API (requires API key)",
        "openrouter" => "OpenRouter (requires API key)",
        "anthropic" => "Anthropic API (requires API key)",
        "azure-openai" => "Azure OpenAI (requires API key + Azure endpoint)",
        "vertex-ai" => "Vertex AI (requires GCP project)",
        other => other,
    }
}

fn display_to_provider(display: &str) -> &str {
    for &p in PROVIDER_TYPES {
        if provider_display(p) == display {
            return p;
        }
    }
    display
}

/// Well-known model lists per provider for quick selection.
fn provider_model_suggestions(provider: &str) -> Vec<&'static str> {
    match provider {
        "github-copilot" => vec![
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "gpt-4o",
            "o3",
            "o4-mini",
        ],
        "chatgpt-subscription" => vec!["gpt-5", "gpt-4o", "o3", "o4-mini"],
        "claude-subscription" => vec!["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"],
        "openai" => vec!["gpt-4o", "gpt-4o-mini", "o3", "o4-mini"],
        "openrouter" => vec![
            "openai/gpt-4o",
            "anthropic/claude-sonnet-4",
            "google/gemini-2.5-pro",
            "meta-llama/llama-4-maverick",
        ],
        "anthropic" => vec!["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"],
        "azure-openai" => vec!["gpt-4o", "gpt-4o-mini", "o3"],
        "vertex-ai" => vec!["gemini-2.5-pro", "gemini-2.5-flash"],
        _ => vec![],
    }
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Run the interactive setup editor.
pub fn run_setup(path: Option<&str>, _force: bool) -> Result<()> {
    println!();
    println!("  Rausu Config Editor");
    println!("  ====================");
    println!();

    // Resolve target path
    let target: PathBuf = path.map(PathBuf::from).unwrap_or_else(default_config_path);

    // Try to load existing config
    let mut config = load_or_create(&target)?;

    // Main editor loop
    match editor_loop(&mut config, &target) {
        Ok(()) => Ok(()),
        Err(e) => handle_inquire_error(e),
    }
}

fn load_or_create(target: &Path) -> Result<AppConfig> {
    // Check if there's an existing config we can load
    let existing = if target.exists() {
        resolve_config_path(Some(target.to_str().unwrap_or("")))
    } else {
        resolve_config_path(None)
    };

    if let Some(existing_path) = existing {
        if existing_path.exists() {
            match AppConfig::load_raw(existing_path.to_str().unwrap_or("")) {
                Ok(cfg) => {
                    println!("  Loaded existing config from: {}", existing_path.display());
                    println!(
                        "  ({} models, {} auth keys)",
                        cfg.models.len(),
                        cfg.auth.keys.len()
                    );
                    println!();
                    return Ok(cfg);
                }
                Err(e) => {
                    println!("  Warning: could not load {}: {e}", existing_path.display());
                    println!("  Starting with empty config.");
                    println!();
                }
            }
        }
    }

    println!("  No existing config found. Starting from scratch.");
    println!();

    Ok(AppConfig {
        server: ServerConfig::default(),
        logging: LoggingConfig::default(),
        auth: AuthConfig::default(),
        models: Vec::new(),
    })
}

// ── Main editor loop ─────────────────────────────────────────────────────────

/// Outcome of a save attempt, used to decide whether the editor loop continues.
#[derive(Debug, PartialEq)]
enum SaveOutcome {
    /// Config was written to disk — exit the editor.
    SavedAndExit,
    /// Save was blocked (validation errors) — stay in the editor.
    BlockedStayInEditor,
    /// User cancelled the confirmation prompt — stay in the editor.
    CancelledStayInEditor,
}

const TOP_MENU: &[&str] = &[
    "Models",
    "Auth",
    "Server",
    "TLS",
    "Logging",
    "Validate",
    "Save and Exit",
    "Exit without Saving",
];

fn editor_loop(config: &mut AppConfig, target: &Path) -> std::result::Result<(), InquireError> {
    loop {
        let choice = Select::new("Configuration section:", TOP_MENU.to_vec())
            .with_help_message(&format!("Target: {}", target.display()))
            .prompt()?;

        match choice {
            "Models" => edit_models(config)?,
            "Auth" => edit_auth(config)?,
            "Server" => edit_server(config)?,
            "TLS" => edit_tls(config)?,
            "Logging" => edit_logging(config)?,
            "Validate" => run_validation(config),
            "Save and Exit" => {
                if save_and_exit(config, target)? == SaveOutcome::SavedAndExit {
                    return Ok(());
                }
            }
            "Exit without Saving" => {
                let confirm = Confirm::new("Discard all changes?")
                    .with_default(false)
                    .prompt()?;
                if confirm {
                    println!("  Changes discarded.");
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}

// ── Models section ───────────────────────────────────────────────────────────

fn edit_models(config: &mut AppConfig) -> std::result::Result<(), InquireError> {
    loop {
        let mut choices: Vec<String> = config
            .models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let providers: Vec<&str> =
                    m.providers.iter().map(|p| p.provider.as_str()).collect();
                format!("{}. {} [{}]", i + 1, m.name, providers.join(", "))
            })
            .collect();
        choices.push("+ Add model".to_string());
        choices.push("< Back".to_string());

        let choice = Select::new("Models:", choices.clone()).prompt()?;

        if choice == "< Back" {
            return Ok(());
        } else if choice == "+ Add model" {
            if let Some(model) = create_model()? {
                config.models.push(model);
            }
        } else {
            // Parse index from "N. name [providers]"
            let idx = choice
                .split('.')
                .next()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .map(|n| n - 1);
            if let Some(idx) = idx {
                if idx < config.models.len() {
                    edit_single_model(config, idx)?;
                }
            }
        }
    }
}

fn create_model() -> std::result::Result<Option<ModelConfig>, InquireError> {
    let name = Text::new("Virtual model name (exposed to clients):").prompt()?;
    if name.trim().is_empty() {
        println!("  Model name cannot be empty.");
        return Ok(None);
    }

    let aliases_str = Text::new("Aliases (comma-separated, or Enter to skip):")
        .with_default("")
        .prompt()?;
    let aliases: Option<Vec<String>> = if aliases_str.trim().is_empty() {
        None
    } else {
        Some(
            aliases_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        )
    };

    println!();
    println!("  Select providers for '{name}':");
    println!("  (You can add more providers later for failover)");
    println!();

    let display_options: Vec<&str> = PROVIDER_TYPES.iter().map(|p| provider_display(p)).collect();
    let selected = MultiSelect::new("Providers for this model:", display_options)
        .with_help_message("Space to select, Enter to confirm. Order = failover priority")
        .prompt()?;

    if selected.is_empty() {
        println!("  No providers selected. Model not created.");
        return Ok(None);
    }

    let mut providers = Vec::new();
    for display in &selected {
        let provider_type = display_to_provider(display);
        println!();
        println!("  Configuring {display} for '{name}':");
        if let Some(deployment) = prompt_provider_deployment(provider_type, &name)? {
            providers.push(deployment);
        }
    }

    if providers.is_empty() {
        println!("  No deployments configured. Model not created.");
        return Ok(None);
    }

    Ok(Some(ModelConfig {
        name,
        aliases,
        providers,
    }))
}

fn edit_single_model(config: &mut AppConfig, idx: usize) -> std::result::Result<(), InquireError> {
    loop {
        let model = &config.models[idx];
        let mut choices = vec![
            format!("Edit name (current: {})", model.name),
            "Edit aliases".to_string(),
        ];

        // List providers
        for (i, p) in model.providers.iter().enumerate() {
            let detail = provider_detail(p);
            choices.push(format!("  {}. {} {}", i + 1, p.provider, detail));
        }
        choices.push("+ Add provider deployment".to_string());
        if model.providers.len() > 1 {
            choices.push("Reorder providers".to_string());
        }
        choices.push("Delete this model".to_string());
        choices.push("< Back".to_string());

        let choice = Select::new(
            &format!("Model '{}':", config.models[idx].name),
            choices.clone(),
        )
        .prompt()?;

        if choice == "< Back" {
            return Ok(());
        } else if choice.starts_with("Edit name") {
            let new_name = Text::new("New model name:")
                .with_default(&config.models[idx].name)
                .prompt()?;
            if !new_name.trim().is_empty() {
                config.models[idx].name = new_name;
            }
        } else if choice == "Edit aliases" {
            let current = config.models[idx]
                .aliases
                .as_ref()
                .map(|a| a.join(", "))
                .unwrap_or_default();
            let new_aliases = Text::new("Aliases (comma-separated, or empty to clear):")
                .with_default(&current)
                .prompt()?;
            if new_aliases.trim().is_empty() {
                config.models[idx].aliases = None;
            } else {
                config.models[idx].aliases = Some(
                    new_aliases
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                );
            }
        } else if choice == "+ Add provider deployment" {
            let display_options: Vec<&str> =
                PROVIDER_TYPES.iter().map(|p| provider_display(p)).collect();
            let provider_display_choice =
                Select::new("Provider type:", display_options).prompt()?;
            let provider_type = display_to_provider(provider_display_choice);
            let model_name = config.models[idx].name.clone();
            if let Some(deployment) = prompt_provider_deployment(provider_type, &model_name)? {
                config.models[idx].providers.push(deployment);
            }
        } else if choice == "Reorder providers" {
            reorder_providers(&mut config.models[idx])?;
        } else if choice == "Delete this model" {
            let confirm = Confirm::new(&format!("Delete model '{}'?", config.models[idx].name))
                .with_default(false)
                .prompt()?;
            if confirm {
                config.models.remove(idx);
                return Ok(());
            }
        } else if choice.starts_with("  ") {
            // Provider line: "  N. provider detail"
            let trimmed = choice.trim();
            let pidx = trimmed
                .split('.')
                .next()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .map(|n| n - 1);
            if let Some(pidx) = pidx {
                if pidx < config.models[idx].providers.len() {
                    edit_provider_deployment(config, idx, pidx)?;
                }
            }
        }
    }
}

fn edit_provider_deployment(
    config: &mut AppConfig,
    model_idx: usize,
    provider_idx: usize,
) -> std::result::Result<(), InquireError> {
    let choices = vec!["Edit", "Delete", "< Back"];
    let choice = Select::new(
        &format!(
            "Provider {}/{}:",
            config.models[model_idx].providers[provider_idx].provider,
            config.models[model_idx].providers[provider_idx].model
        ),
        choices,
    )
    .prompt()?;

    match choice {
        "Edit" => {
            let provider_type = config.models[model_idx].providers[provider_idx]
                .provider
                .clone();
            let model_name = config.models[model_idx].name.clone();
            if let Some(new_deployment) = prompt_provider_deployment(&provider_type, &model_name)? {
                config.models[model_idx].providers[provider_idx] = new_deployment;
            }
        }
        "Delete" => {
            let confirm = Confirm::new("Delete this provider deployment?")
                .with_default(false)
                .prompt()?;
            if confirm {
                config.models[model_idx].providers.remove(provider_idx);
            }
        }
        _ => {}
    }

    Ok(())
}

fn reorder_providers(model: &mut ModelConfig) -> std::result::Result<(), InquireError> {
    if model.providers.len() < 2 {
        return Ok(());
    }

    println!("  Current provider order (first = highest priority):");
    for (i, p) in model.providers.iter().enumerate() {
        println!("    {}. {} ({})", i + 1, p.provider, p.model);
    }
    println!();

    let items: Vec<String> = model
        .providers
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}. {} ({})", i + 1, p.provider, p.model))
        .collect();

    let to_move = Select::new("Select provider to move:", items).prompt()?;
    let from_idx = to_move
        .split('.')
        .next()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|n| n - 1)
        .unwrap_or(0);

    let directions: Vec<&str> = if from_idx == 0 {
        vec!["Move down"]
    } else if from_idx == model.providers.len() - 1 {
        vec!["Move up"]
    } else {
        vec!["Move up", "Move down"]
    };

    let direction = Select::new("Direction:", directions).prompt()?;
    match direction {
        "Move up" if from_idx > 0 => {
            model.providers.swap(from_idx, from_idx - 1);
        }
        "Move down" if from_idx < model.providers.len() - 1 => {
            model.providers.swap(from_idx, from_idx + 1);
        }
        _ => {}
    }

    println!("  Updated order:");
    for (i, p) in model.providers.iter().enumerate() {
        println!("    {}. {} ({})", i + 1, p.provider, p.model);
    }

    Ok(())
}

// ── Provider deployment prompts ──────────────────────────────────────────────

fn prompt_provider_deployment(
    provider: &str,
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    match provider {
        "github-copilot" => prompt_copilot_deployment(virtual_name),
        "chatgpt-subscription" => prompt_chatgpt_deployment(virtual_name),
        "claude-subscription" => prompt_claude_sub_deployment(virtual_name),
        "openai" => prompt_openai_deployment(virtual_name),
        "openrouter" => prompt_openrouter_deployment(virtual_name),
        "anthropic" => prompt_anthropic_deployment(virtual_name),
        "azure-openai" => prompt_azure_openai_deployment(virtual_name),
        "vertex-ai" => prompt_vertex_deployment(virtual_name),
        _ => Ok(None),
    }
}

fn prompt_model_name(
    provider: &str,
    virtual_name: &str,
) -> std::result::Result<String, InquireError> {
    let suggestions = provider_model_suggestions(provider);
    if suggestions.is_empty() {
        Text::new("Upstream model name:")
            .with_default(virtual_name)
            .prompt()
    } else {
        let default = if suggestions.contains(&virtual_name) {
            virtual_name
        } else {
            suggestions[0]
        };
        Text::new("Upstream model name:")
            .with_default(default)
            .with_help_message(&format!("Suggestions: {}", suggestions.join(", ")))
            .prompt()
    }
}

fn prompt_copilot_deployment(
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    let model = prompt_model_name("github-copilot", virtual_name)?;
    Ok(Some(ProviderDeployment {
        provider: "github-copilot".to_string(),
        model,
        api_key: None,
        base_url: None,
        token_source: None,
        credentials_path: None,
        api_version: None,
        project_id: None,
        location: None,
    }))
}

fn prompt_chatgpt_deployment(
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    let model = prompt_model_name("chatgpt-subscription", virtual_name)?;
    let token_source =
        Select::new("Token source:", vec!["auto", "env", "codex", "device_flow"]).prompt()?;
    Ok(Some(ProviderDeployment {
        provider: "chatgpt-subscription".to_string(),
        model,
        api_key: None,
        base_url: None,
        token_source: Some(token_source.to_string()),
        credentials_path: None,
        api_version: None,
        project_id: None,
        location: None,
    }))
}

fn prompt_claude_sub_deployment(
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    let model = prompt_model_name("claude-subscription", virtual_name)?;
    let token_source =
        Select::new("Token source:", vec!["auto", "env", "credentials_file"]).prompt()?;
    Ok(Some(ProviderDeployment {
        provider: "claude-subscription".to_string(),
        model,
        api_key: None,
        base_url: None,
        token_source: Some(token_source.to_string()),
        credentials_path: None,
        api_version: None,
        project_id: None,
        location: None,
    }))
}

fn prompt_openai_deployment(
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    let model = prompt_model_name("openai", virtual_name)?;
    let api_key = Password::new("API key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation()
        .with_help_message("Supports ${ENV_VAR} syntax")
        .prompt()?;
    let base_url_str = Text::new("Base URL (Enter for default OpenAI):")
        .with_default("")
        .prompt()?;
    let base_url = if base_url_str.trim().is_empty() {
        None
    } else {
        Some(base_url_str)
    };
    Ok(Some(ProviderDeployment {
        provider: "openai".to_string(),
        model,
        api_key: Some(api_key),
        base_url,
        token_source: None,
        credentials_path: None,
        api_version: None,
        project_id: None,
        location: None,
    }))
}

fn prompt_openrouter_deployment(
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    let model = prompt_model_name("openrouter", virtual_name)?;
    let api_key = Password::new("API key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation()
        .with_help_message("Supports ${ENV_VAR} syntax — get a key at https://openrouter.ai/keys")
        .prompt()?;
    Ok(Some(ProviderDeployment {
        provider: "openrouter".to_string(),
        model,
        api_key: Some(api_key),
        base_url: None,
        token_source: None,
        credentials_path: None,
        api_version: None,
        project_id: None,
        location: None,
    }))
}

fn prompt_anthropic_deployment(
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    let model = prompt_model_name("anthropic", virtual_name)?;
    let api_key = Password::new("API key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation()
        .with_help_message("Supports ${ENV_VAR} syntax")
        .prompt()?;
    Ok(Some(ProviderDeployment {
        provider: "anthropic".to_string(),
        model,
        api_key: Some(api_key),
        base_url: None,
        token_source: None,
        credentials_path: None,
        api_version: None,
        project_id: None,
        location: None,
    }))
}

fn prompt_azure_openai_deployment(
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    let model = prompt_model_name("azure-openai", virtual_name)?;
    let api_key = Password::new("API key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation()
        .with_help_message("Supports ${ENV_VAR} syntax")
        .prompt()?;
    let base_url = Text::new("Azure resource endpoint (e.g. https://<resource>.openai.azure.com):")
        .prompt()?;
    let api_version = Text::new("API version:")
        .with_default("2024-12-01-preview")
        .prompt()?;
    let api_version = if api_version.is_empty() {
        None
    } else {
        Some(api_version)
    };
    Ok(Some(ProviderDeployment {
        provider: "azure-openai".to_string(),
        model,
        api_key: Some(api_key),
        base_url: Some(base_url),
        token_source: None,
        credentials_path: None,
        api_version,
        project_id: None,
        location: None,
    }))
}

fn prompt_vertex_deployment(
    virtual_name: &str,
) -> std::result::Result<Option<ProviderDeployment>, InquireError> {
    let model = prompt_model_name("vertex-ai", virtual_name)?;
    let project_id = Text::new("GCP project ID:").prompt()?;
    let location = Text::new("GCP location:")
        .with_default("us-central1")
        .prompt()?;
    let creds = Text::new("Credentials file path (Enter to skip):")
        .with_default("")
        .prompt()?;
    let credentials_path = if creds.is_empty() { None } else { Some(creds) };
    Ok(Some(ProviderDeployment {
        provider: "vertex-ai".to_string(),
        model,
        api_key: None,
        base_url: None,
        token_source: None,
        credentials_path,
        api_version: None,
        project_id: Some(project_id),
        location: Some(location),
    }))
}

fn provider_detail(d: &ProviderDeployment) -> String {
    let mut parts = vec![format!("model={}", d.model)];
    if let Some(url) = &d.base_url {
        parts.push(format!("url={url}"));
    }
    if let Some(ts) = &d.token_source {
        parts.push(format!("token={ts}"));
    }
    if let Some(pid) = &d.project_id {
        parts.push(format!("project={pid}"));
    }
    format!("({})", parts.join(", "))
}

// ── Auth section ─────────────────────────────────────────────────────────────

fn edit_auth(config: &mut AppConfig) -> std::result::Result<(), InquireError> {
    loop {
        let mode_label = format!("Mode: {}", config.auth.mode);
        let key_count = config.auth.keys.len();
        let keys_label = format!("Keys: {} configured", key_count);

        let mut choices = vec![mode_label.clone(), keys_label];
        if config.auth.mode == "static" {
            choices.push("+ Add key".to_string());
        }
        choices.push("< Back".to_string());

        let choice = Select::new("Auth:", choices).prompt()?;

        if choice == "< Back" {
            return Ok(());
        } else if choice == mode_label {
            let new_mode = Select::new("Auth mode:", vec!["disabled", "static"]).prompt()?;
            config.auth.mode = new_mode.to_string();
        } else if choice == "+ Add key" {
            let name = Text::new("Key name:").with_default("default").prompt()?;
            let auto_key = generate_auth_key();
            let key = Text::new("Key value (Enter for auto-generated):")
                .with_default(&auto_key)
                .prompt()?;
            config.auth.keys.push(AuthKey { name, key });
        } else if choice.starts_with("Keys:") && key_count > 0 {
            edit_auth_keys(config)?;
        }
    }
}

fn edit_auth_keys(config: &mut AppConfig) -> std::result::Result<(), InquireError> {
    loop {
        let mut choices: Vec<String> = config
            .auth
            .keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let preview = if k.key.len() > 16 {
                    format!("{}...", &k.key[..16])
                } else {
                    k.key.clone()
                };
                format!("{}. {} = {}", i + 1, k.name, preview)
            })
            .collect();
        choices.push("< Back".to_string());

        let choice = Select::new("Auth keys:", choices).prompt()?;
        if choice == "< Back" {
            return Ok(());
        }

        let idx = choice
            .split('.')
            .next()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .map(|n| n - 1);
        if let Some(idx) = idx {
            if idx < config.auth.keys.len() {
                let action = Select::new(
                    &format!("Key '{}':", config.auth.keys[idx].name),
                    vec!["Delete", "< Back"],
                )
                .prompt()?;
                if action == "Delete" {
                    config.auth.keys.remove(idx);
                }
            }
        }
    }
}

// ── Server section ───────────────────────────────────────────────────────────

fn edit_server(config: &mut AppConfig) -> std::result::Result<(), InquireError> {
    let new_host = Text::new("Bind host:")
        .with_default(&config.server.host)
        .prompt()?;
    config.server.host = new_host;

    let port: u16 = loop {
        let port_str = Text::new("Port:")
            .with_default(&config.server.port.to_string())
            .prompt()?;
        match port_str.parse::<u16>() {
            Ok(p) => break p,
            Err(_) => println!("  Invalid port number."),
        }
    };
    config.server.port = port;

    Ok(())
}

// ── TLS section ──────────────────────────────────────────────────────────────

fn edit_tls(config: &mut AppConfig) -> std::result::Result<(), InquireError> {
    let currently_enabled = config.server.tls.is_some();
    let label = if currently_enabled {
        "TLS is enabled"
    } else {
        "TLS is disabled"
    };
    println!("  {label}");

    let enable = Confirm::new("Enable TLS?")
        .with_default(currently_enabled)
        .prompt()?;

    if !enable {
        config.server.tls = None;
        return Ok(());
    }

    let current_cert = config
        .server
        .tls
        .as_ref()
        .map(|t| t.cert_file.as_str())
        .unwrap_or("");
    let current_key = config
        .server
        .tls
        .as_ref()
        .map(|t| t.key_file.as_str())
        .unwrap_or("");

    let cert_file = Text::new("Path to TLS certificate (PEM):")
        .with_default(current_cert)
        .prompt()?;
    let key_file = Text::new("Path to TLS private key (PEM):")
        .with_default(current_key)
        .prompt()?;

    let enable_mtls = Confirm::new("Enable mutual TLS (mTLS)?")
        .with_default(
            config
                .server
                .tls
                .as_ref()
                .is_some_and(|t| t.client_ca_file.is_some()),
        )
        .prompt()?;

    let client_ca_file = if enable_mtls {
        let current_ca = config
            .server
            .tls
            .as_ref()
            .and_then(|t| t.client_ca_file.as_deref())
            .unwrap_or("");
        Some(
            Text::new("Path to client CA certificate (PEM):")
                .with_default(current_ca)
                .prompt()?,
        )
    } else {
        None
    };

    config.server.tls = Some(TlsConfig {
        cert_file,
        key_file,
        client_ca_file,
    });

    Ok(())
}

// ── Logging section ──────────────────────────────────────────────────────────

fn edit_logging(config: &mut AppConfig) -> std::result::Result<(), InquireError> {
    let current_level = config.logging.level.as_deref().unwrap_or("info");
    let levels = vec!["trace", "debug", "info", "warn", "error"];
    let cursor = levels.iter().position(|&l| l == current_level).unwrap_or(2);
    let level = Select::new("Log level:", levels)
        .with_starting_cursor(cursor)
        .prompt()?;
    config.logging.level = Some(level.to_string());

    let current_format = config.logging.format.as_deref().unwrap_or("json");
    let formats = vec!["json", "pretty"];
    let cursor = formats
        .iter()
        .position(|&f| f == current_format)
        .unwrap_or(0);
    let format = Select::new("Log format:", formats)
        .with_starting_cursor(cursor)
        .prompt()?;
    config.logging.format = Some(format.to_string());

    Ok(())
}

// ── Validation ───────────────────────────────────────────────────────────────

fn run_validation(config: &AppConfig) {
    println!();
    println!("  ── Validation Results ──");
    println!();

    let result = validation::validate_config(config);

    if result.issues.is_empty() {
        println!("  All checks passed.");
    } else {
        for issue in &result.issues {
            let icon = match issue.severity {
                Severity::Error => "ERROR",
                Severity::Warning => "WARN ",
            };
            println!("  [{icon}] {}: {}", issue.context, issue.message);
        }
    }

    let error_count = result.errors().len();
    let warn_count = result.warnings().len();
    println!();
    println!("  {} error(s), {} warning(s)", error_count, warn_count);
    println!();
}

// ── Save ─────────────────────────────────────────────────────────────────────

fn save_and_exit(
    config: &mut AppConfig,
    target: &Path,
) -> std::result::Result<SaveOutcome, InquireError> {
    // Run validation before save
    let result = validation::validate_config(config);
    if result.has_errors() {
        println!();
        println!("  Cannot save — configuration has errors:");
        for issue in result.errors() {
            println!("    [ERROR] {}: {}", issue.context, issue.message);
        }
        println!();
        println!("  Fix the errors above and try again.");
        println!();
        return Ok(SaveOutcome::BlockedStayInEditor);
    } else if result.has_warnings() {
        println!();
        println!("  Warnings:");
        for issue in result.warnings() {
            println!("    [WARN] {}: {}", issue.context, issue.message);
        }
        println!();
    }

    // Print summary
    print_summary(config);

    let confirm = Confirm::new("Write this configuration?")
        .with_default(true)
        .prompt()?;
    if !confirm {
        println!("  Save cancelled.");
        return Ok(SaveOutcome::CancelledStayInEditor);
    }

    let yaml = generate_yaml(config);
    write_config(target, &yaml).map_err(|e| {
        println!("  Error writing config: {e}");
        InquireError::OperationCanceled
    })?;

    println!();
    println!("  Config written to: {}", target.display());
    println!();
    println!("  Next steps:");
    println!("    1. Review the file: less {}", target.display());
    println!("    2. Start the server: rausu");
    println!("    3. Validate config:  rausu check");
    println!();

    Ok(SaveOutcome::SavedAndExit)
}

// ── Summary ──────────────────────────────────────────────────────────────────

fn print_summary(config: &AppConfig) {
    println!();
    println!("  ── Configuration Summary ──");
    println!();
    println!("  Server:  {}:{}", config.server.host, config.server.port);
    println!(
        "  Logging: {} / {}",
        config.logging.level.as_deref().unwrap_or("info"),
        config.logging.format.as_deref().unwrap_or("json")
    );
    println!("  Auth:    {}", config.auth.mode);
    if !config.auth.keys.is_empty() {
        for k in &config.auth.keys {
            let preview = if k.key.len() > 20 {
                format!("{}...", &k.key[..20])
            } else {
                k.key.clone()
            };
            println!("           key \"{}\" = {}", k.name, preview);
        }
    }
    if let Some(tls) = &config.server.tls {
        println!("  TLS:     cert={}, key={}", tls.cert_file, tls.key_file);
        if let Some(ca) = &tls.client_ca_file {
            println!("           mTLS client CA={ca}");
        }
    } else {
        println!("  TLS:     disabled");
    }
    println!("  Models:  {} configured", config.models.len());
    for m in &config.models {
        let providers: Vec<String> = m
            .providers
            .iter()
            .map(|p| format!("{} ({})", p.provider, p.model))
            .collect();
        println!("           {} -> {}", m.name, providers.join(", "));
    }
    println!();
}

// ── YAML generation ──────────────────────────────────────────────────────────

/// Generate a commented YAML config string from an AppConfig.
pub fn generate_yaml(config: &AppConfig) -> String {
    let mut yaml = String::new();

    yaml.push_str("# rausu — LLM API Gateway configuration\n");
    yaml.push_str("#\n");
    yaml.push_str("# Generated by `rausu setup`.\n");
    yaml.push_str("#\n");
    yaml.push_str("# Environment variable interpolation: ${VAR_NAME}\n");
    yaml.push_str("# CLI value overrides:                RAUSU__SERVER__PORT=8080\n");
    yaml.push('\n');

    // Server
    yaml.push_str(
        "# ── Server ────────────────────────────────────────────────────────────────────\n",
    );
    yaml.push_str("server:\n");
    yaml.push_str(&format!("  host: {}\n", config.server.host));
    yaml.push_str(&format!("  port: {}\n", config.server.port));

    // TLS
    if let Some(tls) = &config.server.tls {
        yaml.push_str("  tls:\n");
        yaml.push_str(&format!("    cert_file: \"{}\"\n", tls.cert_file));
        yaml.push_str(&format!("    key_file: \"{}\"\n", tls.key_file));
        if let Some(ca) = &tls.client_ca_file {
            yaml.push_str(&format!("    client_ca_file: \"{ca}\"\n"));
        }
    }

    yaml.push('\n');

    // Logging
    yaml.push_str(
        "# ── Logging ───────────────────────────────────────────────────────────────────\n",
    );
    yaml.push_str("logging:\n");
    yaml.push_str(&format!(
        "  level: {}        # trace | debug | info | warn | error\n",
        config.logging.level.as_deref().unwrap_or("info")
    ));
    yaml.push_str(&format!(
        "  format: {}     # json (structured) | pretty (human-readable)\n",
        config.logging.format.as_deref().unwrap_or("json")
    ));
    yaml.push('\n');

    // Auth
    yaml.push_str(
        "# ── Authentication ────────────────────────────────────────────────────────────\n",
    );
    yaml.push_str("auth:\n");
    yaml.push_str(&format!("  mode: {}\n", config.auth.mode));
    if !config.auth.keys.is_empty() {
        yaml.push_str("  keys:\n");
        for k in &config.auth.keys {
            yaml.push_str(&format!("    - name: \"{}\"\n", k.name));
            yaml.push_str(&format!("      key: \"{}\"\n", k.key));
        }
    }
    yaml.push('\n');

    // Models
    yaml.push_str(
        "# ── Models ────────────────────────────────────────────────────────────────────\n",
    );
    yaml.push_str("models:\n");

    if config.models.is_empty() {
        yaml.push_str("  # No models configured — add entries here or re-run `rausu setup`.\n");
        yaml.push_str("  []\n");
    } else {
        for m in &config.models {
            yaml.push_str(&format!("  - name: {}\n", m.name));
            if let Some(aliases) = &m.aliases {
                if !aliases.is_empty() {
                    yaml.push_str("    aliases:\n");
                    for a in aliases {
                        yaml.push_str(&format!("      - {a}\n"));
                    }
                }
            }
            yaml.push_str("    providers:\n");
            for p in &m.providers {
                yaml.push_str(&format!("      - provider: {}\n", p.provider));
                yaml.push_str(&format!("        model: {}\n", p.model));

                if let Some(key) = &p.api_key {
                    yaml.push_str(&format!("        api_key: \"{key}\"\n"));
                }
                if let Some(url) = &p.base_url {
                    yaml.push_str(&format!("        base_url: {url}\n"));
                }
                if let Some(ts) = &p.token_source {
                    yaml.push_str(&format!("        token_source: {ts}\n"));
                }
                if let Some(av) = &p.api_version {
                    yaml.push_str(&format!("        api_version: {av}\n"));
                }
                if let Some(pid) = &p.project_id {
                    yaml.push_str(&format!("        project_id: {pid}\n"));
                }
                if let Some(loc) = &p.location {
                    yaml.push_str(&format!("        location: {loc}\n"));
                }
                if let Some(cp) = &p.credentials_path {
                    yaml.push_str(&format!("        credentials_path: \"{cp}\"\n"));
                }
            }
        }
    }

    yaml
}

// ── Auth key generation ──────────────────────────────────────────────────────

/// Generate a random API key with the `rausu-sk-` prefix.
pub fn generate_auth_key() -> String {
    format!("rausu-sk-{}", Uuid::new_v4().simple())
}

// ── File writing ─────────────────────────────────────────────────────────────

fn write_config(target: &Path, yaml: &str) -> anyhow::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, yaml)?;
    Ok(())
}

// ── Error handling ───────────────────────────────────────────────────────────

fn handle_inquire_error<T>(err: InquireError) -> anyhow::Result<T> {
    match err {
        InquireError::OperationCanceled => {
            println!();
            println!("Setup cancelled.");
            std::process::exit(0);
        }
        InquireError::OperationInterrupted => {
            println!();
            std::process::exit(130);
        }
        other => Err(other.into()),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> AppConfig {
        AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 4000,
                tls: None,
            },
            logging: LoggingConfig {
                level: Some("info".to_string()),
                format: Some("pretty".to_string()),
            },
            auth: AuthConfig {
                mode: "disabled".to_string(),
                keys: Vec::new(),
            },
            models: vec![
                ModelConfig {
                    name: "gpt-4o".to_string(),
                    aliases: None,
                    providers: vec![ProviderDeployment {
                        provider: "openai".to_string(),
                        model: "gpt-4o".to_string(),
                        api_key: Some("sk-test123".to_string()),
                        base_url: None,
                        token_source: None,
                        credentials_path: None,
                        api_version: None,
                        project_id: None,
                        location: None,
                    }],
                },
                ModelConfig {
                    name: "claude-sonnet-4-6".to_string(),
                    aliases: None,
                    providers: vec![ProviderDeployment {
                        provider: "github-copilot".to_string(),
                        model: "claude-sonnet-4-6".to_string(),
                        api_key: None,
                        base_url: None,
                        token_source: None,
                        credentials_path: None,
                        api_version: None,
                        project_id: None,
                        location: None,
                    }],
                },
            ],
        }
    }

    #[test]
    fn test_generate_yaml_contains_server() {
        let yaml = generate_yaml(&sample_config());
        assert!(yaml.contains("server:"));
        assert!(yaml.contains("host: 127.0.0.1"));
        assert!(yaml.contains("port: 4000"));
    }

    #[test]
    fn test_generate_yaml_contains_logging() {
        let yaml = generate_yaml(&sample_config());
        assert!(yaml.contains("logging:"));
        assert!(yaml.contains("level: info"));
        assert!(yaml.contains("format: pretty"));
    }

    #[test]
    fn test_generate_yaml_contains_models() {
        let yaml = generate_yaml(&sample_config());
        assert!(yaml.contains("models:"));
        assert!(yaml.contains("name: gpt-4o"));
        assert!(yaml.contains("provider: openai"));
        assert!(yaml.contains("api_key: \"sk-test123\""));
        assert!(yaml.contains("name: claude-sonnet-4-6"));
        assert!(yaml.contains("provider: github-copilot"));
    }

    #[test]
    fn test_generate_yaml_auth_static() {
        let mut config = sample_config();
        config.auth.mode = "static".to_string();
        config.auth.keys = vec![AuthKey {
            name: "my-key".to_string(),
            key: "rausu-sk-abc123".to_string(),
        }];
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("mode: static"));
        assert!(yaml.contains("name: \"my-key\""));
        assert!(yaml.contains("key: \"rausu-sk-abc123\""));
    }

    #[test]
    fn test_generate_yaml_tls() {
        let mut config = sample_config();
        config.server.tls = Some(TlsConfig {
            cert_file: "/etc/rausu/server.crt".to_string(),
            key_file: "/etc/rausu/server.key".to_string(),
            client_ca_file: Some("/etc/rausu/ca.crt".to_string()),
        });
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("tls:"));
        assert!(yaml.contains("cert_file: \"/etc/rausu/server.crt\""));
        assert!(yaml.contains("key_file: \"/etc/rausu/server.key\""));
        assert!(yaml.contains("client_ca_file: \"/etc/rausu/ca.crt\""));
    }

    #[test]
    fn test_generate_yaml_vertex_ai() {
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![ModelConfig {
                name: "gemini-pro".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "vertex-ai".to_string(),
                    model: "gemini-pro".to_string(),
                    api_key: None,
                    base_url: None,
                    token_source: None,
                    api_version: None,
                    project_id: Some("my-gcp-project".to_string()),
                    location: Some("us-central1".to_string()),
                    credentials_path: Some("/path/to/creds.json".to_string()),
                }],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("provider: vertex-ai"));
        assert!(yaml.contains("project_id: my-gcp-project"));
        assert!(yaml.contains("location: us-central1"));
        assert!(yaml.contains("credentials_path: \"/path/to/creds.json\""));
    }

    #[test]
    fn test_generate_yaml_empty_models() {
        let config = AppConfig {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
                tls: None,
            },
            logging: LoggingConfig {
                level: Some("debug".to_string()),
                format: Some("json".to_string()),
            },
            auth: AuthConfig::default(),
            models: Vec::new(),
        };
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("models:"));
        assert!(yaml.contains("No models configured"));
    }

    #[test]
    fn test_generate_auth_key_format() {
        let key = generate_auth_key();
        assert!(
            key.starts_with("rausu-sk-"),
            "key should start with rausu-sk-"
        );
        let suffix = &key["rausu-sk-".len()..];
        assert_eq!(suffix.len(), 32, "uuid part should be 32 hex chars");
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "uuid part should be hex"
        );
    }

    #[test]
    fn test_generate_auth_key_uniqueness() {
        let k1 = generate_auth_key();
        let k2 = generate_auth_key();
        assert_ne!(k1, k2, "generated keys should be unique");
    }

    #[test]
    fn test_generate_yaml_token_source() {
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![ModelConfig {
                name: "gpt-5".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "chatgpt-subscription".to_string(),
                    model: "gpt-5".to_string(),
                    api_key: None,
                    base_url: None,
                    token_source: Some("auto".to_string()),
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("provider: chatgpt-subscription"));
        assert!(yaml.contains("token_source: auto"));
    }

    #[test]
    fn test_generate_yaml_deepseek() {
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![ModelConfig {
                name: "deepseek-chat".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "openai".to_string(),
                    model: "deepseek-chat".to_string(),
                    api_key: Some("sk-deep".to_string()),
                    base_url: Some("https://api.deepseek.com/v1".to_string()),
                    token_source: None,
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("base_url: https://api.deepseek.com/v1"));
        assert!(yaml.contains("api_key: \"sk-deep\""));
    }

    #[test]
    fn test_generate_yaml_aliases() {
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: Some(vec!["gpt-4".to_string(), "gpt4o".to_string()]),
                providers: vec![ProviderDeployment {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    api_key: Some("sk-test".to_string()),
                    base_url: None,
                    token_source: None,
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("aliases:"));
        assert!(yaml.contains("- gpt-4"));
        assert!(yaml.contains("- gpt4o"));
    }

    #[test]
    fn test_roundtrip_preserves_env_placeholder() {
        // Simulate the full setup round-trip: load_raw → generate_yaml
        // The ${ENV_VAR} placeholder must survive unchanged.
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig {
                mode: "static".to_string(),
                keys: vec![AuthKey {
                    name: "main".to_string(),
                    key: "${AUTH_TOKEN}".to_string(),
                }],
            },
            models: vec![ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    api_key: Some("${OPENAI_API_KEY}".to_string()),
                    base_url: None,
                    token_source: None,
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(
            yaml.contains("api_key: \"${OPENAI_API_KEY}\""),
            "env placeholder must survive round-trip, got:\n{yaml}"
        );
        assert!(
            yaml.contains("key: \"${AUTH_TOKEN}\""),
            "auth key placeholder must survive round-trip, got:\n{yaml}"
        );
    }

    #[test]
    fn test_roundtrip_placeholder_not_expanded() {
        // Even when the env var is set, generate_yaml must emit the raw placeholder.
        std::env::set_var("ROUNDTRIP_SECRET", "super-secret-value");
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    api_key: Some("${ROUNDTRIP_SECRET}".to_string()),
                    base_url: None,
                    token_source: None,
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(
            !yaml.contains("super-secret-value"),
            "expanded secret must NOT appear in output"
        );
        assert!(
            yaml.contains("${ROUNDTRIP_SECRET}"),
            "raw placeholder must appear in output"
        );
        std::env::remove_var("ROUNDTRIP_SECRET");
    }

    #[test]
    fn test_roundtrip_empty_string_stays_empty() {
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    api_key: Some(String::new()),
                    base_url: None,
                    token_source: None,
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(
            yaml.contains("api_key: \"\""),
            "empty string must remain as empty quoted string, got:\n{yaml}"
        );
    }

    #[test]
    fn test_roundtrip_literal_key_stays_literal() {
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    api_key: Some("sk-literal-key-abc123".to_string()),
                    base_url: None,
                    token_source: None,
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(
            yaml.contains("api_key: \"sk-literal-key-abc123\""),
            "literal key must be preserved exactly"
        );
    }

    #[test]
    fn test_hard_validation_errors_block_save() {
        // A config with hard validation errors: static auth with no keys
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig {
                mode: "static".to_string(),
                keys: Vec::new(), // Error: static mode requires keys
            },
            models: vec![ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    api_key: Some("sk-test".to_string()),
                    base_url: None,
                    token_source: None,
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };

        let result = validation::validate_config(&config);
        assert!(
            result.has_errors(),
            "static auth with no keys should produce errors"
        );
    }

    #[test]
    fn test_blocked_save_returns_stay_in_editor() {
        // When save_and_exit encounters hard validation errors it must return
        // BlockedStayInEditor so the editor loop continues instead of exiting.
        let mut config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig {
                mode: "static".to_string(),
                keys: Vec::new(), // Error: static mode requires keys
            },
            models: vec![ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    api_key: Some("sk-test".to_string()),
                    base_url: None,
                    token_source: None,
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                }],
            }],
        };

        let target =
            std::env::temp_dir().join(format!("rausu_blocked_save_{}.yaml", std::process::id()));
        let outcome = save_and_exit(&mut config, &target).expect("should not error");
        assert_eq!(
            outcome,
            SaveOutcome::BlockedStayInEditor,
            "blocked save must return BlockedStayInEditor, not SavedAndExit"
        );
        assert!(
            !target.exists(),
            "config file must NOT be written when validation has errors"
        );
    }

    #[test]
    fn test_blocked_save_does_not_exit_editor_loop_contract() {
        // Regression: previously, editor_loop used `return save_and_exit(...)`
        // which exited the loop even when save was blocked.
        // This test verifies the contract: only SavedAndExit causes exit.
        assert_ne!(
            SaveOutcome::BlockedStayInEditor,
            SaveOutcome::SavedAndExit,
            "BlockedStayInEditor must be distinguishable from SavedAndExit"
        );
        assert_ne!(
            SaveOutcome::CancelledStayInEditor,
            SaveOutcome::SavedAndExit,
            "CancelledStayInEditor must be distinguishable from SavedAndExit"
        );
    }

    #[test]
    fn test_full_file_roundtrip_with_load_raw() {
        // End-to-end: write YAML with placeholder → load_raw → generate_yaml → verify
        let dir =
            std::env::temp_dir().join(format!("rausu_setup_roundtrip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");

        let original_yaml = r#"
models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${ROUNDTRIP_SECRET}"
auth:
  mode: static
  keys:
    - name: prod
      key: "${AUTH_KEY}"
"#;
        std::fs::write(&path, original_yaml).unwrap();
        std::env::set_var("ROUNDTRIP_SECRET", "super-secret-value");
        std::env::set_var("AUTH_KEY", "auth-secret-value");

        let cfg = AppConfig::load_raw(path.to_str().unwrap()).unwrap();
        let output = generate_yaml(&cfg);

        assert!(
            output.contains("${ROUNDTRIP_SECRET}"),
            "placeholder must survive full round-trip"
        );
        assert!(
            !output.contains("super-secret-value"),
            "expanded secret must NOT appear"
        );
        assert!(
            output.contains("${AUTH_KEY}"),
            "auth placeholder must survive"
        );
        assert!(
            !output.contains("auth-secret-value"),
            "expanded auth secret must NOT appear"
        );

        std::env::remove_var("ROUNDTRIP_SECRET");
        std::env::remove_var("AUTH_KEY");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_generate_yaml_multi_provider_model() {
        let config = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![ModelConfig {
                name: "claude-sonnet".to_string(),
                aliases: None,
                providers: vec![
                    ProviderDeployment {
                        provider: "anthropic".to_string(),
                        model: "claude-sonnet-4-6".to_string(),
                        api_key: Some("sk-ant".to_string()),
                        base_url: None,
                        token_source: None,
                        credentials_path: None,
                        api_version: None,
                        project_id: None,
                        location: None,
                    },
                    ProviderDeployment {
                        provider: "github-copilot".to_string(),
                        model: "claude-sonnet-4-6".to_string(),
                        api_key: None,
                        base_url: None,
                        token_source: None,
                        credentials_path: None,
                        api_version: None,
                        project_id: None,
                        location: None,
                    },
                ],
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("provider: anthropic"));
        assert!(yaml.contains("provider: github-copilot"));
        // Both should be under the same model
        let model_count = yaml.matches("- name: claude-sonnet").count();
        assert_eq!(model_count, 1, "should have exactly one model entry");
    }

    #[test]
    fn test_write_config_creates_file() {
        let path =
            std::env::temp_dir().join(format!("rausu_setup_test_{}.yaml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let yaml = generate_yaml(&sample_config());
        write_config(&path, &yaml).expect("should write config");
        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("server:"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_provider_display_roundtrip() {
        for &p in PROVIDER_TYPES {
            let display = provider_display(p);
            let back = display_to_provider(display);
            assert_eq!(back, p, "roundtrip failed for {p}");
        }
    }
}
