//! `rausu check` subcommand — config validation and provider connectivity testing.
//!
//! Uses the shared validation module for config checks, then performs
//! provider-specific connectivity tests.

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::PathBuf;

use rausu::config::{
    paths::resolve_config_path,
    schema::{AppConfig, ProviderDeployment},
};
use rausu::validation::{self, Severity};

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
            println!("   Server: {}:{}", cfg.server.host, cfg.server.port);
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

    let mut all_ok = true;

    // ── Step 1b: TLS Validation ──────────────────────────────────────────────
    if let Some(tls) = &app_config.server.tls {
        let mtls = tls.client_ca_file.is_some();
        let mode_label = if mtls { "mTLS" } else { "TLS" };
        println!(
            "\u{1f512} {bold}Transport Security:{reset} {mode_label}",
            bold = c.bold,
            reset = c.reset
        );

        // Validate cert file
        let cert_ok = validate_tls_file(&tls.cert_file, "server certificate", &c);
        // Validate key file
        let key_ok = validate_tls_file(&tls.key_file, "private key", &c);

        // Validate client CA file if mTLS
        let ca_ok = if let Some(ca_path) = &tls.client_ca_file {
            validate_tls_file(ca_path, "client CA certificate", &c)
        } else {
            true
        };

        // Attempt to parse PEMs if files exist
        if cert_ok && key_ok && ca_ok {
            match rausu::server::tls::build_rustls_server_config(tls) {
                Ok(_) => {
                    println!(
                        "   {green}\u{2713}{reset} {mode_label} configuration valid (PEM parsed OK)",
                        green = c.green,
                        reset = c.reset
                    );
                }
                Err(e) => {
                    println!(
                        "   {red}\u{2717}{reset} {mode_label} configuration invalid: {e:#}",
                        red = c.red,
                        reset = c.reset
                    );
                    all_ok = false;
                }
            }
        } else {
            all_ok = false;
        }
        println!();
    }

    // ── Step 2: Shared Config Validation ────────────────────────────────────
    let validation_result = validation::validate_config(&app_config);
    if !validation_result.issues.is_empty() {
        println!(
            "\u{1f50d} {bold}Validation:{reset}",
            bold = c.bold,
            reset = c.reset
        );
        for issue in &validation_result.issues {
            let (icon, color) = match issue.severity {
                Severity::Error => {
                    all_ok = false;
                    ("\u{2717}", c.red)
                }
                Severity::Warning => ("\u{26a0}", c.yellow),
            };
            println!(
                "   {color}{icon}{reset} {}: {}",
                issue.context,
                issue.message,
                color = color,
                reset = c.reset,
            );
        }
        println!();
    }

    // ── Step 3: Model Summary ──────────────────────────────────────────────
    let model_count = app_config.models.len();
    println!(
        "\u{1f4e6} {bold}Models ({model_count}):{reset}",
        bold = c.bold,
        reset = c.reset
    );

    let mut provider_endpoints: Vec<ProviderEndpoint> = Vec::new();

    if model_count == 0 {
        println!(
            "   {yellow}\u{26a0} No models configured{reset}",
            yellow = c.yellow,
            reset = c.reset
        );
    }

    for model in &app_config.models {
        let provider_names: Vec<String> =
            model.providers.iter().map(provider_short_label).collect();
        let providers_str = provider_names.join(", ");

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

        println!(
            "   {green}\u{2713}{reset} {name}{alias_str} \u{2192} {providers_str}",
            green = c.green,
            reset = c.reset,
            name = model.name,
        );

        // Collect endpoints for connectivity testing
        for deployment in &model.providers {
            if let Some(ep) = build_endpoint(deployment) {
                if !provider_endpoints.iter().any(|e| e.key == ep.key) {
                    provider_endpoints.push(ep);
                }
            }
        }
    }
    println!();

    // ── Step 4: Provider Connectivity ───────────────────────────────────────
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

fn provider_short_label(d: &ProviderDeployment) -> String {
    match d.provider.as_str() {
        "openai" | "openrouter" => {
            if let Some(url) = &d.base_url {
                format!("{} ({url})", d.provider)
            } else {
                d.provider.clone()
            }
        }
        other => other.to_string(),
    }
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
        "openrouter" => {
            let base = d
                .base_url
                .as_deref()
                .unwrap_or("https://openrouter.ai/api/v1");
            let base = base.trim_end_matches('/');
            let url = format!("{base}/models");
            let label = if d.base_url.is_some() {
                format!("openrouter ({base})")
            } else {
                "openrouter (https://openrouter.ai/api/v1)".to_string()
            };
            Some(ProviderEndpoint {
                key: format!("openrouter:{base}"),
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

/// Check that a TLS file path exists and is readable, printing results.
fn validate_tls_file(path: &str, description: &str, c: &Colors) -> bool {
    let p = std::path::Path::new(path);
    if p.exists() {
        if std::fs::metadata(p).is_ok_and(|m| m.len() > 0) {
            println!(
                "   {green}\u{2713}{reset} {description}: {path}",
                green = c.green,
                reset = c.reset
            );
            true
        } else {
            println!(
                "   {red}\u{2717}{reset} {description} is empty: {path}",
                red = c.red,
                reset = c.reset
            );
            false
        }
    } else {
        println!(
            "   {red}\u{2717}{reset} {description} not found: {path}",
            red = c.red,
            reset = c.reset
        );
        false
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rausu::config::schema::*;

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
}
