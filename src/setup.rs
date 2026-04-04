//! `rausu setup` — interactive setup wizard.
//!
//! Walks the user through provider selection, server settings, authentication,
//! TLS, and logging, then generates a commented YAML config file.

use anyhow::Result;
use inquire::{Confirm, InquireError, MultiSelect, Password, PasswordDisplayMode, Select, Text};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::paths::default_config_path;

// ── Data structures for wizard state ─────────────────────────────────────────

/// A single model entry for the generated config.
#[derive(Debug, Clone)]
pub struct SetupModel {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub token_source: Option<String>,
    pub project_id: Option<String>,
    pub location: Option<String>,
    pub credentials_path: Option<String>,
}

/// An auth key entry.
#[derive(Debug, Clone)]
pub struct SetupAuthKey {
    pub name: String,
    pub key: String,
}

/// TLS settings.
#[derive(Debug, Clone)]
pub struct SetupTls {
    pub cert_file: String,
    pub key_file: String,
    pub client_ca_file: Option<String>,
}

/// Complete wizard result.
#[derive(Debug, Clone)]
pub struct SetupConfig {
    pub host: String,
    pub port: u16,
    pub log_level: String,
    pub log_format: String,
    pub auth_mode: String,
    pub auth_keys: Vec<SetupAuthKey>,
    pub tls: Option<SetupTls>,
    pub models: Vec<SetupModel>,
}

// ── Provider catalogue ───────────────────────────────────────────────────────

const PROVIDER_CHOICES: &[&str] = &[
    "GitHub Copilot (no API key needed)",
    "ChatGPT Subscription (no API key needed)",
    "Claude Subscription (no API key needed)",
    "OpenAI API (requires API key)",
    "Anthropic API (requires API key)",
    "Vertex AI (requires GCP project)",
    "DeepSeek (requires API key)",
    "Ollama (local, no key)",
    "Custom OpenAI-compatible provider",
];

// ── Interactive wizard ───────────────────────────────────────────────────────

/// Run the interactive setup wizard.
pub fn run_setup(path: Option<&str>, force: bool) -> Result<()> {
    println!();
    println!("  Rausu Setup Wizard");
    println!("  ==================");
    println!();

    let config = match run_wizard() {
        Ok(c) => c,
        Err(e) => return handle_inquire_error(e),
    };

    // Resolve target path
    let target: PathBuf = path.map(PathBuf::from).unwrap_or_else(default_config_path);

    if target.exists() && !force {
        let overwrite = Confirm::new(&format!(
            "Config file already exists at {}. Overwrite?",
            target.display()
        ))
        .with_default(false)
        .prompt();
        match overwrite {
            Ok(true) => {}
            Ok(false) => {
                println!("Aborted. Use --force or choose a different --path.");
                return Ok(());
            }
            Err(e) => return handle_inquire_error(e),
        }
    }

    // Print summary
    print_summary(&config);

    // Confirm write
    let confirm = Confirm::new("Write this configuration?")
        .with_default(true)
        .prompt();
    match confirm {
        Ok(true) => {}
        Ok(false) => {
            println!("Setup cancelled.");
            return Ok(());
        }
        Err(e) => return handle_inquire_error(e),
    }

    // Write config
    let yaml = generate_yaml(&config);
    write_config(&target, &yaml)?;

    println!();
    println!("  Config written to: {}", target.display());
    println!();
    println!("  Next steps:");
    println!("    1. Review the file: less {}", target.display());
    println!("    2. Start the server: rausu");
    println!("    3. Validate config:  rausu check");
    println!();

    Ok(())
}

fn run_wizard() -> std::result::Result<SetupConfig, InquireError> {
    // Step 1: Provider selection
    let selected = MultiSelect::new(
        "Which providers do you want to configure?",
        PROVIDER_CHOICES.to_vec(),
    )
    .with_help_message("Use ↑↓ to move, Space to select, Enter to confirm")
    .prompt()?;

    if selected.is_empty() {
        println!("No providers selected — generating a minimal config.");
    }

    // Step 2: Per-provider configuration
    let mut models: Vec<SetupModel> = Vec::new();
    for choice in &selected {
        let mut provider_models = configure_provider(choice)?;
        models.append(&mut provider_models);
    }

    // Step 3: Server configuration
    let host = Text::new("Bind host:").with_default("127.0.0.1").prompt()?;

    let port: u16 = loop {
        let port_str = Text::new("Port:").with_default("4000").prompt()?;
        match port_str.parse::<u16>() {
            Ok(p) => break p,
            Err(_) => println!("  Invalid port number. Please enter a value between 1 and 65535."),
        }
    };

    // Step 4: Authentication
    let enable_auth = Confirm::new("Enable API key authentication?")
        .with_default(false)
        .prompt()?;

    let (auth_mode, auth_keys) = if enable_auth {
        let mut keys = Vec::new();
        loop {
            let key_name = Text::new("Key name:").with_default("default").prompt()?;
            let auto_key = generate_auth_key();
            let key_value = Text::new("Key value (Enter for auto-generated):")
                .with_default(&auto_key)
                .prompt()?;
            keys.push(SetupAuthKey {
                name: key_name,
                key: key_value,
            });
            let more = Confirm::new("Add another key?")
                .with_default(false)
                .prompt()?;
            if !more {
                break;
            }
        }
        ("static".to_string(), keys)
    } else {
        ("disabled".to_string(), Vec::new())
    };

    // Step 5: TLS
    let enable_tls = Confirm::new("Enable TLS?").with_default(false).prompt()?;

    let tls = if enable_tls {
        let cert_file = Text::new("Path to TLS certificate (PEM):").prompt()?;
        let key_file = Text::new("Path to TLS private key (PEM):").prompt()?;
        let enable_mtls = Confirm::new("Enable mutual TLS (mTLS)?")
            .with_default(false)
            .prompt()?;
        let client_ca_file = if enable_mtls {
            Some(Text::new("Path to client CA certificate (PEM):").prompt()?)
        } else {
            None
        };
        Some(SetupTls {
            cert_file,
            key_file,
            client_ca_file,
        })
    } else {
        None
    };

    // Step 6: Logging
    let log_level = Select::new(
        "Log level:",
        vec!["trace", "debug", "info", "warn", "error"],
    )
    .with_starting_cursor(2) // "info"
    .prompt()?
    .to_string();

    let log_format = Select::new("Log format:", vec!["json", "pretty"])
        .with_starting_cursor(1) // "pretty"
        .prompt()?
        .to_string();

    Ok(SetupConfig {
        host,
        port,
        log_level,
        log_format,
        auth_mode,
        auth_keys,
        tls,
        models,
    })
}

fn configure_provider(choice: &str) -> std::result::Result<Vec<SetupModel>, InquireError> {
    if choice.starts_with("GitHub Copilot") {
        let model_options = vec![
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "gpt-4o",
            "o3",
            "o4-mini",
        ];
        let selected = MultiSelect::new("GitHub Copilot — select models:", model_options)
            .with_help_message("Space to select, Enter to confirm")
            .prompt()?;
        Ok(selected
            .into_iter()
            .map(|m| SetupModel {
                name: m.to_string(),
                provider: "github-copilot".to_string(),
                model: m.to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                project_id: None,
                location: None,
                credentials_path: None,
            })
            .collect())
    } else if choice.starts_with("ChatGPT Subscription") {
        let model_options = vec!["gpt-5", "gpt-4o", "o3", "o4-mini"];
        let selected = MultiSelect::new("ChatGPT Subscription — select models:", model_options)
            .with_help_message("Space to select, Enter to confirm")
            .prompt()?;
        let token_source =
            Select::new("Token source:", vec!["auto", "env", "codex", "device_flow"]).prompt()?;
        Ok(selected
            .into_iter()
            .map(|m| SetupModel {
                name: m.to_string(),
                provider: "chatgpt-subscription".to_string(),
                model: m.to_string(),
                api_key: None,
                base_url: None,
                token_source: Some(token_source.to_string()),
                project_id: None,
                location: None,
                credentials_path: None,
            })
            .collect())
    } else if choice.starts_with("Claude Subscription") {
        let model_options = vec!["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"];
        let selected = MultiSelect::new("Claude Subscription — select models:", model_options)
            .with_help_message("Space to select, Enter to confirm")
            .prompt()?;
        let token_source =
            Select::new("Token source:", vec!["auto", "env", "credentials_file"]).prompt()?;
        Ok(selected
            .into_iter()
            .map(|m| SetupModel {
                name: m.to_string(),
                provider: "claude-subscription".to_string(),
                model: m.to_string(),
                api_key: None,
                base_url: None,
                token_source: Some(token_source.to_string()),
                project_id: None,
                location: None,
                credentials_path: None,
            })
            .collect())
    } else if choice.starts_with("OpenAI API") {
        let api_key = Password::new("OpenAI API key:")
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()?;
        let model = Text::new("Model(s) — comma-separated:")
            .with_default("gpt-4o")
            .prompt()?;
        Ok(model
            .split(',')
            .map(|m| m.trim())
            .filter(|m| !m.is_empty())
            .map(|m| SetupModel {
                name: m.to_string(),
                provider: "openai".to_string(),
                model: m.to_string(),
                api_key: Some(api_key.clone()),
                base_url: None,
                token_source: None,
                project_id: None,
                location: None,
                credentials_path: None,
            })
            .collect())
    } else if choice.starts_with("Anthropic API") {
        let api_key = Password::new("Anthropic API key:")
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()?;
        let model = Text::new("Model(s) — comma-separated:")
            .with_default("claude-sonnet-4-6")
            .prompt()?;
        Ok(model
            .split(',')
            .map(|m| m.trim())
            .filter(|m| !m.is_empty())
            .map(|m| SetupModel {
                name: m.to_string(),
                provider: "anthropic".to_string(),
                model: m.to_string(),
                api_key: Some(api_key.clone()),
                base_url: None,
                token_source: None,
                project_id: None,
                location: None,
                credentials_path: None,
            })
            .collect())
    } else if choice.starts_with("Vertex AI") {
        let project_id = Text::new("GCP project ID:").prompt()?;
        let location = Text::new("GCP location:")
            .with_default("us-central1")
            .prompt()?;
        let creds = Text::new("Credentials file path (Enter to skip):")
            .with_default("")
            .prompt()?;
        let credentials_path = if creds.is_empty() { None } else { Some(creds) };
        let model = Text::new("Model(s) — comma-separated:")
            .with_default("gemini-2.5-pro")
            .prompt()?;
        Ok(model
            .split(',')
            .map(|m| m.trim())
            .filter(|m| !m.is_empty())
            .map(|m| SetupModel {
                name: m.to_string(),
                provider: "vertex-ai".to_string(),
                model: m.to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                project_id: Some(project_id.clone()),
                location: Some(location.clone()),
                credentials_path: credentials_path.clone(),
            })
            .collect())
    } else if choice.starts_with("DeepSeek") {
        let api_key = Password::new("DeepSeek API key:")
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()?;
        let model = Text::new("Model:").with_default("deepseek-chat").prompt()?;
        Ok(vec![SetupModel {
            name: model.clone(),
            provider: "openai".to_string(),
            model,
            api_key: Some(api_key),
            base_url: Some("https://api.deepseek.com/v1".to_string()),
            token_source: None,
            project_id: None,
            location: None,
            credentials_path: None,
        }])
    } else if choice.starts_with("Ollama") {
        let base_url = Text::new("Ollama base URL:")
            .with_default("http://localhost:11434/v1")
            .prompt()?;
        let model = Text::new("Model:").with_default("llama3").prompt()?;
        Ok(vec![SetupModel {
            name: model.clone(),
            provider: "openai".to_string(),
            model,
            api_key: Some("ollama".to_string()),
            base_url: Some(base_url),
            token_source: None,
            project_id: None,
            location: None,
            credentials_path: None,
        }])
    } else if choice.starts_with("Custom") {
        let name = Text::new("Provider display name:").prompt()?;
        let base_url = Text::new("Base URL (e.g. https://api.example.com/v1):").prompt()?;
        let api_key = Password::new("API key:")
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()?;
        let model = Text::new("Model name:").prompt()?;
        let _ = name; // used only for display during wizard
        Ok(vec![SetupModel {
            name: model.clone(),
            provider: "openai".to_string(),
            model,
            api_key: Some(api_key),
            base_url: Some(base_url),
            token_source: None,
            project_id: None,
            location: None,
            credentials_path: None,
        }])
    } else {
        Ok(Vec::new())
    }
}

// ── Summary ──────────────────────────────────────────────────────────────────

fn print_summary(config: &SetupConfig) {
    println!();
    println!("  ── Configuration Summary ──");
    println!();
    println!("  Server:  {}:{}", config.host, config.port);
    println!("  Logging: {} / {}", config.log_level, config.log_format);
    println!("  Auth:    {}", config.auth_mode);
    if !config.auth_keys.is_empty() {
        for k in &config.auth_keys {
            println!(
                "           key \"{}\" = {}...",
                k.name,
                &k.key[..20.min(k.key.len())]
            );
        }
    }
    if let Some(tls) = &config.tls {
        println!("  TLS:     cert={}, key={}", tls.cert_file, tls.key_file);
        if let Some(ca) = &tls.client_ca_file {
            println!("           mTLS client CA={}", ca);
        }
    } else {
        println!("  TLS:     disabled");
    }
    println!("  Models:  {} configured", config.models.len());
    for m in &config.models {
        println!("           {} → {} ({})", m.name, m.model, m.provider);
    }
    println!();
}

// ── YAML generation ──────────────────────────────────────────────────────────

/// Generate a commented YAML config string from the wizard result.
pub fn generate_yaml(config: &SetupConfig) -> String {
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
    yaml.push_str(&format!("  host: {}\n", config.host));
    yaml.push_str(&format!("  port: {}\n", config.port));

    // TLS
    if let Some(tls) = &config.tls {
        yaml.push_str("  tls:\n");
        yaml.push_str(&format!("    cert_file: \"{}\"\n", tls.cert_file));
        yaml.push_str(&format!("    key_file: \"{}\"\n", tls.key_file));
        if let Some(ca) = &tls.client_ca_file {
            yaml.push_str(&format!("    client_ca_file: \"{}\"\n", ca));
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
        config.log_level
    ));
    yaml.push_str(&format!(
        "  format: {}     # json (structured) | pretty (human-readable)\n",
        config.log_format
    ));
    yaml.push('\n');

    // Auth
    yaml.push_str(
        "# ── Authentication ────────────────────────────────────────────────────────────\n",
    );
    yaml.push_str("auth:\n");
    yaml.push_str(&format!("  mode: {}\n", config.auth_mode));
    if !config.auth_keys.is_empty() {
        yaml.push_str("  keys:\n");
        for k in &config.auth_keys {
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
            yaml.push_str("    providers:\n");
            yaml.push_str(&format!("      - provider: {}\n", m.provider));
            yaml.push_str(&format!("        model: {}\n", m.model));

            if let Some(key) = &m.api_key {
                yaml.push_str(&format!("        api_key: \"{}\"\n", key));
            }
            if let Some(url) = &m.base_url {
                yaml.push_str(&format!("        base_url: {}\n", url));
            }
            if let Some(ts) = &m.token_source {
                yaml.push_str(&format!("        token_source: {}\n", ts));
            }
            if let Some(pid) = &m.project_id {
                yaml.push_str(&format!("        project_id: {}\n", pid));
            }
            if let Some(loc) = &m.location {
                yaml.push_str(&format!("        location: {}\n", loc));
            }
            if let Some(cp) = &m.credentials_path {
                yaml.push_str(&format!("        credentials_path: \"{}\"\n", cp));
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

fn write_config(target: &Path, yaml: &str) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, yaml)?;
    Ok(())
}

// ── Error handling ───────────────────────────────────────────────────────────

fn handle_inquire_error<T>(err: InquireError) -> Result<T> {
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

    fn sample_config() -> SetupConfig {
        SetupConfig {
            host: "127.0.0.1".to_string(),
            port: 4000,
            log_level: "info".to_string(),
            log_format: "pretty".to_string(),
            auth_mode: "disabled".to_string(),
            auth_keys: Vec::new(),
            tls: None,
            models: vec![
                SetupModel {
                    name: "gpt-4o".to_string(),
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    api_key: Some("sk-test123".to_string()),
                    base_url: None,
                    token_source: None,
                    project_id: None,
                    location: None,
                    credentials_path: None,
                },
                SetupModel {
                    name: "claude-sonnet-4-6".to_string(),
                    provider: "github-copilot".to_string(),
                    model: "claude-sonnet-4-6".to_string(),
                    api_key: None,
                    base_url: None,
                    token_source: None,
                    project_id: None,
                    location: None,
                    credentials_path: None,
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
        config.auth_mode = "static".to_string();
        config.auth_keys = vec![SetupAuthKey {
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
        config.tls = Some(SetupTls {
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
        let config = SetupConfig {
            host: "127.0.0.1".to_string(),
            port: 4000,
            log_level: "info".to_string(),
            log_format: "pretty".to_string(),
            auth_mode: "disabled".to_string(),
            auth_keys: Vec::new(),
            tls: None,
            models: vec![SetupModel {
                name: "gemini-pro".to_string(),
                provider: "vertex-ai".to_string(),
                model: "gemini-pro".to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                project_id: Some("my-gcp-project".to_string()),
                location: Some("us-central1".to_string()),
                credentials_path: Some("/path/to/creds.json".to_string()),
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
        let config = SetupConfig {
            host: "0.0.0.0".to_string(),
            port: 8080,
            log_level: "debug".to_string(),
            log_format: "json".to_string(),
            auth_mode: "disabled".to_string(),
            auth_keys: Vec::new(),
            tls: None,
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
        // uuid v4 simple format is 32 hex chars
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
        let config = SetupConfig {
            host: "127.0.0.1".to_string(),
            port: 4000,
            log_level: "info".to_string(),
            log_format: "pretty".to_string(),
            auth_mode: "disabled".to_string(),
            auth_keys: Vec::new(),
            tls: None,
            models: vec![SetupModel {
                name: "gpt-5".to_string(),
                provider: "chatgpt-subscription".to_string(),
                model: "gpt-5".to_string(),
                api_key: None,
                base_url: None,
                token_source: Some("auto".to_string()),
                project_id: None,
                location: None,
                credentials_path: None,
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("provider: chatgpt-subscription"));
        assert!(yaml.contains("token_source: auto"));
    }

    #[test]
    fn test_generate_yaml_deepseek() {
        let config = SetupConfig {
            host: "127.0.0.1".to_string(),
            port: 4000,
            log_level: "info".to_string(),
            log_format: "pretty".to_string(),
            auth_mode: "disabled".to_string(),
            auth_keys: Vec::new(),
            tls: None,
            models: vec![SetupModel {
                name: "deepseek-chat".to_string(),
                provider: "openai".to_string(),
                model: "deepseek-chat".to_string(),
                api_key: Some("sk-deep".to_string()),
                base_url: Some("https://api.deepseek.com/v1".to_string()),
                token_source: None,
                project_id: None,
                location: None,
                credentials_path: None,
            }],
        };
        let yaml = generate_yaml(&config);
        assert!(yaml.contains("base_url: https://api.deepseek.com/v1"));
        assert!(yaml.contains("api_key: \"sk-deep\""));
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
}
