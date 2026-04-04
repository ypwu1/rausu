//! Configuration schema types.

use anyhow::{Context, Result};
use config::{Config, Environment, File};
use serde::{Deserialize, Serialize};

/// Top-level application configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    /// HTTP server settings.
    #[serde(default)]
    pub server: ServerConfig,
    /// Logging settings.
    #[serde(default)]
    pub logging: LoggingConfig,
    /// Authentication settings.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Model routing configuration.
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

/// HTTP server configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    /// Bind host (default: 0.0.0.0).
    #[serde(default = "default_host")]
    pub host: String,
    /// Bind port (default: 4000).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Optional TLS configuration.
    #[serde(default)]
    pub tls: Option<TlsConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            tls: None,
        }
    }
}

/// TLS / mTLS configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TlsConfig {
    /// Path to PEM-encoded server certificate chain (supports `${ENV_VAR}`).
    pub cert_file: String,
    /// Path to PEM-encoded server private key (supports `${ENV_VAR}`).
    pub key_file: String,
    /// Optional path to PEM-encoded CA certificate for client verification (mTLS).
    /// When set, the server requires and verifies client certificates.
    pub client_ca_file: Option<String>,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    4000
}

/// Logging configuration.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LoggingConfig {
    /// Log level: trace, debug, info, warn, error (default: info).
    pub level: Option<String>,
    /// Log format: json | pretty (default: json).
    pub format: Option<String>,
}

/// Authentication configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    /// Auth mode: "disabled" (default) or "static".
    #[serde(default = "default_auth_mode")]
    pub mode: String,
    /// API keys (only used when mode is "static").
    #[serde(default)]
    pub keys: Vec<AuthKey>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: default_auth_mode(),
            keys: Vec::new(),
        }
    }
}

/// A named API key for static authentication.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthKey {
    /// Human-readable label for this key.
    pub name: String,
    /// The secret key value (supports `${ENV_VAR}` interpolation).
    pub key: String,
}

fn default_auth_mode() -> String {
    "disabled".to_string()
}

/// Model routing entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelConfig {
    /// Virtual model name exposed to clients.
    pub name: String,
    /// Optional alternative names that also route to this model entry.
    /// Useful when clients may send either a short alias or a full versioned ID.
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    /// Provider deployments for this model.
    pub providers: Vec<ProviderDeployment>,
}

/// A single provider deployment for a model.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderDeployment {
    /// Provider type: openai | anthropic | claude-subscription | chatgpt-subscription | vertex-ai.
    pub provider: String,
    /// The model name on the provider side.
    pub model: String,
    /// API key (supports `${ENV_VAR}` interpolation).
    pub api_key: Option<String>,
    /// Optional base URL override (OpenAI-compatible providers only).
    pub base_url: Option<String>,
    /// Token source for `claude-subscription`: `env` | `credentials_file` | `auto` (default).
    pub token_source: Option<String>,
    /// Custom path to the credentials file.
    ///
    /// - For `claude-subscription`: overrides default `~/.claude/.credentials.json`.
    /// - For `vertex-ai`: path to a service-account JSON or ADC JSON file; also
    ///   falls back to `GOOGLE_APPLICATION_CREDENTIALS` env var, then the default
    ///   ADC path `~/.config/gcloud/application_default_credentials.json`.
    pub credentials_path: Option<String>,

    // ── Vertex AI specific ────────────────────────────────────────────────────
    /// GCP project ID (required for `vertex-ai`).
    pub project_id: Option<String>,
    /// GCP region or `"global"` (required for `vertex-ai`, default `"us-central1"`).
    pub location: Option<String>,
}

impl AppConfig {
    /// Load configuration from a YAML file with environment variable overrides.
    ///
    /// Environment variables are prefixed with `RAUSU_` and use `__` as separator.
    /// For example, `RAUSU_SERVER__PORT=8080` overrides `server.port`.
    pub fn load(path: &str) -> Result<Self> {
        let config = Config::builder()
            .add_source(File::with_name(path).required(false))
            .add_source(
                Environment::with_prefix("RAUSU")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()
            .context("Failed to build configuration")?;

        let mut app_config: AppConfig = config
            .try_deserialize()
            .context("Failed to deserialise configuration")?;

        // Interpolate environment variables in api_key fields
        for model in &mut app_config.models {
            for deployment in &mut model.providers {
                if let Some(key) = &deployment.api_key {
                    deployment.api_key = Some(interpolate_env(key));
                }
            }
        }

        // Interpolate environment variables in auth keys
        for auth_key in &mut app_config.auth.keys {
            auth_key.key = interpolate_env(&auth_key.key);
        }

        // Interpolate environment variables in TLS paths
        if let Some(tls) = &mut app_config.server.tls {
            tls.cert_file = interpolate_env(&tls.cert_file);
            tls.key_file = interpolate_env(&tls.key_file);
            if let Some(ca) = &tls.client_ca_file {
                tls.client_ca_file = Some(interpolate_env(ca));
            }
        }

        Ok(app_config)
    }
}

/// Expand `${VAR_NAME}` patterns in a string using environment variables.
pub fn interpolate_env(s: &str) -> String {
    let mut result = s.to_string();
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = std::env::var(var_name).unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                value,
                &result[start + end + 1..]
            );
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interpolate_env() {
        std::env::set_var("TEST_KEY_XYZ", "secret");
        assert_eq!(interpolate_env("${TEST_KEY_XYZ}"), "secret");
        assert_eq!(
            interpolate_env("prefix_${TEST_KEY_XYZ}_suffix"),
            "prefix_secret_suffix"
        );
        assert_eq!(interpolate_env("no_vars"), "no_vars");
        std::env::remove_var("TEST_KEY_XYZ");
    }

    #[test]
    fn test_default_server_config() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 4000);
    }

    #[test]
    fn test_load_missing_config_uses_defaults() {
        let cfg = AppConfig::load("nonexistent_config_test.yaml").unwrap();
        assert_eq!(cfg.server.host, "0.0.0.0");
        assert_eq!(cfg.server.port, 4000);
        assert!(cfg.models.is_empty());
        assert_eq!(cfg.auth.mode, "disabled");
        assert!(cfg.auth.keys.is_empty());
    }

    #[test]
    fn test_auth_config_defaults() {
        let cfg = AuthConfig::default();
        assert_eq!(cfg.mode, "disabled");
        assert!(cfg.keys.is_empty());
    }

    #[test]
    fn test_auth_key_env_interpolation() {
        std::env::set_var("RAUSU_TEST_AUTH_KEY", "rausu-sk-secret");
        let mut cfg = AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig {
                mode: "static".to_string(),
                keys: vec![AuthKey {
                    name: "test".to_string(),
                    key: "${RAUSU_TEST_AUTH_KEY}".to_string(),
                }],
            },
            models: vec![],
        };
        // Simulate the interpolation that load() performs
        for auth_key in &mut cfg.auth.keys {
            auth_key.key = interpolate_env(&auth_key.key);
        }
        assert_eq!(cfg.auth.keys[0].key, "rausu-sk-secret");
        std::env::remove_var("RAUSU_TEST_AUTH_KEY");
    }

    #[test]
    fn test_default_server_config_tls_none() {
        let cfg = ServerConfig::default();
        assert!(cfg.tls.is_none());
    }

    #[test]
    fn test_tls_config_deserialization() {
        let json = r#"{
            "server": {
                "host": "127.0.0.1",
                "port": 8443,
                "tls": {
                    "cert_file": "/etc/rausu/server.crt",
                    "key_file": "/etc/rausu/server.key"
                }
            },
            "models": []
        }"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        let tls = cfg.server.tls.unwrap();
        assert_eq!(tls.cert_file, "/etc/rausu/server.crt");
        assert_eq!(tls.key_file, "/etc/rausu/server.key");
        assert!(tls.client_ca_file.is_none());
    }

    #[test]
    fn test_tls_config_with_mtls() {
        let json = r#"{
            "server": {
                "tls": {
                    "cert_file": "server.crt",
                    "key_file": "server.key",
                    "client_ca_file": "ca.crt"
                }
            },
            "models": []
        }"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        let tls = cfg.server.tls.unwrap();
        assert_eq!(tls.client_ca_file.as_deref(), Some("ca.crt"));
    }

    #[test]
    fn test_tls_env_interpolation() {
        std::env::set_var("RAUSU_TEST_CERT", "/tmp/test.crt");
        std::env::set_var("RAUSU_TEST_KEY", "/tmp/test.key");
        let mut tls = TlsConfig {
            cert_file: "${RAUSU_TEST_CERT}".to_string(),
            key_file: "${RAUSU_TEST_KEY}".to_string(),
            client_ca_file: None,
        };
        tls.cert_file = interpolate_env(&tls.cert_file);
        tls.key_file = interpolate_env(&tls.key_file);
        assert_eq!(tls.cert_file, "/tmp/test.crt");
        assert_eq!(tls.key_file, "/tmp/test.key");
        std::env::remove_var("RAUSU_TEST_CERT");
        std::env::remove_var("RAUSU_TEST_KEY");
    }
}
