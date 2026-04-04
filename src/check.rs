//! `rausu check` subcommand — config validation and provider connectivity testing.

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::PathBuf;

use crate::config::{
    paths::resolve_config_path,
    schema::{AppConfig, ModelConfig, ProviderDeployment},
};

/// Known provider types.
const VALID_PROVIDERS: &[&str] = &[
    "openai",
    "anthropic",
    "claude-subscription",
    "chatgpt-subscription",
    "github-copilot",
    "vertex-ai",
];

/// ANSI color helpers — return empty strings when not a TTY.
struct Colors {
    green: &'static str,
    red: &'static str,
    yellow: &'static str,
    bold: &'static str,
    reset: &'static str,
}

impl Colors {
    fn new() -> Self {
        if std::io::stdout().is_terminal() {
            Self {
                green: "\x1b[32m",
                red: "\x1b[31m",
                yellow: "\x1b[33m",
                bold: "\x1b[1m",
                reset: "\x1b[0m",
            }
        } else {
            Self {
                green: "",
                red: "",
                yellow: "",
                bold: "",
                reset: "",
            }
        }
    }
}

/// Execute the `rausu check` subcommand.
pub async fn run_check(cli_config: Option<&str>) -> Result<()> {
    let c = Colors::new();

    // ── Step 1: Config Loading ──────────────────────────────────────────────
    let config_path = resolve_config_path(cli_config);
    let config_path = match config_path {
        Some(p) => p,
        None => {
            println!(
                "{red}\u{2717} No configuration file found.{reset}",
                red = c.red,
                reset = c.reset
            );
            println!("  Run `rausu init` to generate a template.");
            anyhow::bail!("No configuration file found");
        }
    };

    let config_str = config_path
        .to_str()
        .context("Config path contains non-UTF-8 characters")?;

    let app_config = match AppConfig::load(config_str) {
        Ok(cfg) => {
            println!(
                "\u{1f4cb} {bold}Config:{reset} {}",
                config_path.display(),
                bold = c.bold,
                reset = c.reset
            );
            println!(
                "   Server: {}:{}",
                app_config_server_display(&cfg),
                cfg.server.port
            );
            println!("   Auth: {}", auth_display(&cfg, &c));
            println!();
            cfg
        }
        Err(e) => {
            println!(
                "{red}\u{2717} Failed to load config:{reset} {e}",
                red = c.red,
                reset = c.reset
            );
            anyhow::bail!("Config loading failed");
        }
    };

    // ── Step 2: Model Validation ────────────────────────────────────────────
    let model_count = app_config.models.len();
    println!(
        "\u{1f4e6} {bold}Models ({model_count}):{reset}",
        bold = c.bold,
        reset = c.reset
    );

    let mut all_ok = true;
    let mut provider_endpoints: Vec<ProviderEndpoint> = Vec::new();

    if model_count == 0 {
        println!(
            "   {yellow}\u{26a0} No models configured{reset}",
            yellow = c.yellow,
            reset = c.reset
        );
        all_ok = false;
    }

    for model in &app_config.models {
        let (ok, endpoints) = validate_model(model, &c);
        if !ok {
            all_ok = false;
        }
        for ep in endpoints {
            if !provider_endpoints.iter().any(|e| e.key == ep.key) {
                provider_endpoints.push(ep);
            }
        }
    }
    println!();

    // ── Step 3: Provider Connectivity ───────────────────────────────────────
    println!(
        "\u{1f50c} {bold}Connectivity:{reset}",
        bold = c.bold,
        reset = c.reset
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .context("Failed to create HTTP client")?;

    let mut reachable_count = 0u32;
    let total_count = provider_endpoints.len() as u32;

    for ep in &provider_endpoints {
        let (status, detail) = test_connectivity(&client, ep).await;
        let (icon, color) = match status {
            ConnStatus::Ok => {
                reachable_count += 1;
                ("\u{2713}", c.green)
            }
            ConnStatus::Warn => ("\u{26a0}", c.yellow),
            ConnStatus::Fail => {
                all_ok = false;
                ("\u{2717}", c.red)
            }
        };
        println!(
            "   {color}{icon}{reset} {label}: {detail}",
            color = color,
            reset = c.reset,
            label = ep.label,
        );
    }
    println!();

    // ── Step 4: Auth Check ──────────────────────────────────────────────────
    if app_config.auth.mode == "static" && app_config.auth.keys.is_empty() {
        println!(
            "{red}\u{2717} Auth mode is 'static' but no keys are configured{reset}",
            red = c.red,
            reset = c.reset
        );
        all_ok = false;
    }

    // ── Summary ─────────────────────────────────────────────────────────────
    if total_count > 0 {
        if all_ok {
            println!(
                "{green}\u{2705} {reachable_count}/{total_count} providers OK{reset}",
                green = c.green,
                reset = c.reset
            );
        } else {
            println!(
                "{yellow}\u{26a0} {reachable_count}/{total_count} providers OK{reset}",
                yellow = c.yellow,
                reset = c.reset
            );
        }
    }

    Ok(())
}

fn app_config_server_display(cfg: &AppConfig) -> &str {
    &cfg.server.host
}

fn auth_display(cfg: &AppConfig, c: &Colors) -> String {
    match cfg.auth.mode.as_str() {
        "disabled" => format!(
            "{yellow}disabled{reset}",
            yellow = c.yellow,
            reset = c.reset
        ),
        "static" => {
            let n = cfg.auth.keys.len();
            let suffix = if n == 1 { "key" } else { "keys" };
            format!("static ({n} {suffix})")
        }
        other => format!("{other} (unknown)"),
    }
}

/// Intermediate type to de-duplicate connectivity checks.
struct ProviderEndpoint {
    key: String,
    label: String,
    kind: EndpointKind,
}

enum EndpointKind {
    /// GET {base_url}/models to test connectivity.
    HttpGet { url: String },
    /// Check if a file exists on disk.
    FileExists {
        path: PathBuf,
        description: &'static str,
    },
    /// Check for an env var or file (ChatGPT subscription).
    ChatGptToken,
}

#[derive(Debug)]
enum ConnStatus {
    Ok,
    Warn,
    Fail,
}

/// Validate a single model entry, returning (ok, endpoints_to_test).
fn validate_model(model: &ModelConfig, c: &Colors) -> (bool, Vec<ProviderEndpoint>) {
    let mut ok = true;
    let mut endpoints = Vec::new();

    let aliases = model
        .aliases
        .as_ref()
        .map(|a| a.join(", "))
        .unwrap_or_default();
    let alias_str = if aliases.is_empty() {
        String::new()
    } else {
        format!(" (aliases: {aliases})")
    };

    let provider_names: Vec<String> = model.providers.iter().map(provider_short_label).collect();
    let providers_str = provider_names.join(", ");

    for deployment in &model.providers {
        let (dep_ok, ep) = validate_deployment(deployment, c, &model.name);
        if !dep_ok {
            ok = false;
        }
        if let Some(ep) = ep {
            endpoints.push(ep);
        }
    }

    let icon = if ok {
        format!("{green}\u{2713}{reset}", green = c.green, reset = c.reset)
    } else {
        format!(
            "{yellow}\u{26a0}{reset}",
            yellow = c.yellow,
            reset = c.reset
        )
    };
    println!(
        "   {icon} {name}{alias_str} \u{2192} {providers_str}",
        name = model.name
    );

    (ok, endpoints)
}

fn provider_short_label(d: &ProviderDeployment) -> String {
    match d.provider.as_str() {
        "openai" => {
            if let Some(url) = &d.base_url {
                format!("openai ({url})")
            } else {
                "openai".to_string()
            }
        }
        other => other.to_string(),
    }
}

fn validate_deployment(
    d: &ProviderDeployment,
    c: &Colors,
    model_name: &str,
) -> (bool, Option<ProviderEndpoint>) {
    let mut ok = true;

    // Check provider type is valid
    if !VALID_PROVIDERS.contains(&d.provider.as_str()) {
        println!(
            "      {red}\u{2717} {model_name}: unknown provider type '{}'  {reset}",
            d.provider,
            red = c.red,
            reset = c.reset
        );
        ok = false;
        return (ok, None);
    }

    // Check model name is present
    if d.model.is_empty() {
        println!(
            "      {red}\u{2717} {model_name}/{}: model name is empty{reset}",
            d.provider,
            red = c.red,
            reset = c.reset
        );
        ok = false;
    }

    // Provider-specific required field checks
    match d.provider.as_str() {
        "openai" | "anthropic" => {
            if d.api_key.as_ref().is_none_or(|k| k.is_empty()) {
                println!(
                    "      {yellow}\u{26a0} {model_name}/{}: no api_key configured{reset}",
                    d.provider,
                    yellow = c.yellow,
                    reset = c.reset
                );
            }
        }
        "vertex-ai" => {
            if d.project_id.is_none() {
                println!(
                    "      {red}\u{2717} {model_name}/vertex-ai: project_id is required{reset}",
                    red = c.red,
                    reset = c.reset
                );
                ok = false;
            }
        }
        _ => {}
    }

    // Build endpoint for connectivity testing
    let endpoint = build_endpoint(d);

    (ok, endpoint)
}

fn build_endpoint(d: &ProviderDeployment) -> Option<ProviderEndpoint> {
    match d.provider.as_str() {
        "openai" => {
            let base = d.base_url.as_deref().unwrap_or("https://api.openai.com/v1");
            let base = base.trim_end_matches('/');
            let url = format!("{base}/models");
            let label = if d.base_url.is_some() {
                format!("openai ({base})")
            } else {
                "openai (https://api.openai.com/v1)".to_string()
            };
            Some(ProviderEndpoint {
                key: format!("openai:{base}"),
                label,
                kind: EndpointKind::HttpGet { url },
            })
        }
        "anthropic" => {
            let base = d.base_url.as_deref().unwrap_or("https://api.anthropic.com");
            Some(ProviderEndpoint {
                key: format!("anthropic:{base}"),
                label: format!("anthropic ({base})"),
                kind: EndpointKind::HttpGet {
                    url: base.to_string(),
                },
            })
        }
        "github-copilot" => {
            let path = copilot_hosts_path();
            Some(ProviderEndpoint {
                key: "github-copilot".to_string(),
                label: "github-copilot".to_string(),
                kind: EndpointKind::FileExists {
                    path,
                    description: "hosts.json",
                },
            })
        }
        "chatgpt-subscription" => Some(ProviderEndpoint {
            key: "chatgpt-subscription".to_string(),
            label: "chatgpt-subscription".to_string(),
            kind: EndpointKind::ChatGptToken,
        }),
        "claude-subscription" => {
            let path = d
                .credentials_path
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(claude_credentials_path);
            Some(ProviderEndpoint {
                key: "claude-subscription".to_string(),
                label: "claude-subscription".to_string(),
                kind: EndpointKind::FileExists {
                    path,
                    description: "credentials file",
                },
            })
        }
        "vertex-ai" => {
            let path = d
                .credentials_path
                .as_ref()
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
                        .ok()
                        .map(PathBuf::from)
                })
                .unwrap_or_else(gcloud_adc_path);
            Some(ProviderEndpoint {
                key: format!("vertex-ai:{}", path.display()),
                label: "vertex-ai".to_string(),
                kind: EndpointKind::FileExists {
                    path,
                    description: "credentials file",
                },
            })
        }
        _ => None,
    }
}

async fn test_connectivity(
    client: &reqwest::Client,
    ep: &ProviderEndpoint,
) -> (ConnStatus, String) {
    match &ep.kind {
        EndpointKind::HttpGet { url } => match client.get(url).send().await {
            Ok(resp) => (
                ConnStatus::Ok,
                format!("reachable (HTTP {})", resp.status().as_u16()),
            ),
            Err(e) => {
                let msg = if e.is_timeout() {
                    "connection timed out".to_string()
                } else if e.is_connect() {
                    "connection refused".to_string()
                } else {
                    format!("{e}")
                };
                (ConnStatus::Fail, msg)
            }
        },
        EndpointKind::FileExists { path, description } => {
            if path.exists() {
                (
                    ConnStatus::Ok,
                    format!("{description} found ({path})", path = path.display()),
                )
            } else {
                (
                    ConnStatus::Warn,
                    format!("{description} not found ({path})", path = path.display()),
                )
            }
        }
        EndpointKind::ChatGptToken => {
            // Check env var first
            if std::env::var("CHATGPT_ACCESS_TOKEN")
                .ok()
                .is_some_and(|v| !v.is_empty())
            {
                return (
                    ConnStatus::Ok,
                    "token available (CHATGPT_ACCESS_TOKEN env)".to_string(),
                );
            }
            // Check codex auth file
            let codex_path = codex_auth_path();
            if codex_path.exists() {
                return (
                    ConnStatus::Ok,
                    format!("token available ({})", codex_path.display()),
                );
            }
            // Check rausu chatgpt auth file
            let rausu_path = chatgpt_rausu_auth_path();
            if rausu_path.exists() {
                return (
                    ConnStatus::Ok,
                    format!("token available ({})", rausu_path.display()),
                );
            }
            (
                ConnStatus::Warn,
                "no token found (set CHATGPT_ACCESS_TOKEN or run codex auth)".to_string(),
            )
        }
    }
}

// ── Path helpers ────────────────────────────────────────────────────────────

fn copilot_hosts_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("github-copilot")
        .join("hosts.json")
}

fn claude_credentials_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join(".credentials.json")
}

fn codex_auth_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("auth.json")
}

fn chatgpt_rausu_auth_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("rausu")
        .join("chatgpt-auth.json")
}

fn gcloud_adc_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("gcloud")
        .join("application_default_credentials.json")
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::*;

    #[test]
    fn test_valid_provider_types() {
        assert!(VALID_PROVIDERS.contains(&"openai"));
        assert!(VALID_PROVIDERS.contains(&"anthropic"));
        assert!(VALID_PROVIDERS.contains(&"github-copilot"));
        assert!(VALID_PROVIDERS.contains(&"chatgpt-subscription"));
        assert!(VALID_PROVIDERS.contains(&"claude-subscription"));
        assert!(VALID_PROVIDERS.contains(&"vertex-ai"));
        assert!(!VALID_PROVIDERS.contains(&"invalid-provider"));
    }

    #[test]
    fn test_provider_short_label_openai_default() {
        let d = ProviderDeployment {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            api_key: None,
            base_url: None,
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        };
        assert_eq!(provider_short_label(&d), "openai");
    }

    #[test]
    fn test_provider_short_label_openai_custom_url() {
        let d = ProviderDeployment {
            provider: "openai".to_string(),
            model: "deepseek-chat".to_string(),
            api_key: None,
            base_url: Some("https://api.deepseek.com/v1".to_string()),
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        };
        assert_eq!(
            provider_short_label(&d),
            "openai (https://api.deepseek.com/v1)"
        );
    }

    #[test]
    fn test_provider_short_label_other() {
        let d = ProviderDeployment {
            provider: "anthropic".to_string(),
            model: "claude-opus-4-6".to_string(),
            api_key: None,
            base_url: None,
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        };
        assert_eq!(provider_short_label(&d), "anthropic");
    }

    #[test]
    fn test_auth_display_disabled() {
        let c = Colors {
            green: "",
            red: "",
            yellow: "",
            bold: "",
            reset: "",
        };
        let cfg = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models: vec![],
        };
        assert_eq!(auth_display(&cfg, &c), "disabled");
    }

    #[test]
    fn test_auth_display_static_keys() {
        let c = Colors {
            green: "",
            red: "",
            yellow: "",
            bold: "",
            reset: "",
        };
        let cfg = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig {
                mode: "static".to_string(),
                keys: vec![
                    AuthKey {
                        name: "a".to_string(),
                        key: "k1".to_string(),
                    },
                    AuthKey {
                        name: "b".to_string(),
                        key: "k2".to_string(),
                    },
                ],
            },
            models: vec![],
        };
        assert_eq!(auth_display(&cfg, &c), "static (2 keys)");
    }

    #[test]
    fn test_auth_display_static_one_key() {
        let c = Colors {
            green: "",
            red: "",
            yellow: "",
            bold: "",
            reset: "",
        };
        let cfg = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig {
                mode: "static".to_string(),
                keys: vec![AuthKey {
                    name: "a".to_string(),
                    key: "k1".to_string(),
                }],
            },
            models: vec![],
        };
        assert_eq!(auth_display(&cfg, &c), "static (1 key)");
    }

    #[test]
    fn test_build_endpoint_openai_default() {
        let d = ProviderDeployment {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            api_key: Some("sk-test".to_string()),
            base_url: None,
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        };
        let ep = build_endpoint(&d).unwrap();
        assert_eq!(ep.key, "openai:https://api.openai.com/v1");
        assert!(
            matches!(ep.kind, EndpointKind::HttpGet { url } if url == "https://api.openai.com/v1/models")
        );
    }

    #[test]
    fn test_build_endpoint_openai_custom_url() {
        let d = ProviderDeployment {
            provider: "openai".to_string(),
            model: "deepseek-chat".to_string(),
            api_key: Some("sk-test".to_string()),
            base_url: Some("https://api.deepseek.com/v1".to_string()),
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        };
        let ep = build_endpoint(&d).unwrap();
        assert_eq!(ep.key, "openai:https://api.deepseek.com/v1");
    }

    #[test]
    fn test_build_endpoint_github_copilot() {
        let d = ProviderDeployment {
            provider: "github-copilot".to_string(),
            model: "claude-opus-4.6".to_string(),
            api_key: None,
            base_url: None,
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        };
        let ep = build_endpoint(&d).unwrap();
        assert_eq!(ep.key, "github-copilot");
        assert!(matches!(ep.kind, EndpointKind::FileExists { .. }));
    }

    #[test]
    fn test_build_endpoint_chatgpt_subscription() {
        let d = ProviderDeployment {
            provider: "chatgpt-subscription".to_string(),
            model: "gpt-5.4".to_string(),
            api_key: None,
            base_url: None,
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        };
        let ep = build_endpoint(&d).unwrap();
        assert_eq!(ep.key, "chatgpt-subscription");
        assert!(matches!(ep.kind, EndpointKind::ChatGptToken));
    }

    #[test]
    fn test_build_endpoint_unknown_provider() {
        let d = ProviderDeployment {
            provider: "unknown".to_string(),
            model: "test".to_string(),
            api_key: None,
            base_url: None,
            token_source: None,
            credentials_path: None,
            project_id: None,
            location: None,
        };
        assert!(build_endpoint(&d).is_none());
    }

    #[test]
    fn test_validate_model_valid() {
        let c = Colors {
            green: "",
            red: "",
            yellow: "",
            bold: "",
            reset: "",
        };
        let model = ModelConfig {
            name: "gpt-5".to_string(),
            aliases: Some(vec!["gpt-5.4".to_string()]),
            providers: vec![ProviderDeployment {
                provider: "chatgpt-subscription".to_string(),
                model: "gpt-5.4".to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                credentials_path: None,
                project_id: None,
                location: None,
            }],
        };
        let (ok, endpoints) = validate_model(&model, &c);
        assert!(ok);
        assert_eq!(endpoints.len(), 1);
    }

    #[test]
    fn test_validate_model_invalid_provider() {
        let c = Colors {
            green: "",
            red: "",
            yellow: "",
            bold: "",
            reset: "",
        };
        let model = ModelConfig {
            name: "test".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "invalid-type".to_string(),
                model: "test".to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                credentials_path: None,
                project_id: None,
                location: None,
            }],
        };
        let (ok, endpoints) = validate_model(&model, &c);
        assert!(!ok);
        assert!(endpoints.is_empty());
    }

    #[test]
    fn test_validate_model_vertex_missing_project_id() {
        let c = Colors {
            green: "",
            red: "",
            yellow: "",
            bold: "",
            reset: "",
        };
        let model = ModelConfig {
            name: "gemini".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "vertex-ai".to_string(),
                model: "gemini-2.5-flash".to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                credentials_path: None,
                project_id: None,
                location: None,
            }],
        };
        let (ok, _) = validate_model(&model, &c);
        assert!(!ok);
    }
}
