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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
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

/// Model routing entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelConfig {
    /// Virtual model name exposed to clients.
    pub name: String,
    /// Provider deployments for this model.
    pub providers: Vec<ProviderDeployment>,
}

/// A single provider deployment for a model.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderDeployment {
    /// Provider type: openai | anthropic.
    pub provider: String,
    /// The model name on the provider side.
    pub model: String,
    /// API key (supports `${ENV_VAR}` interpolation).
    pub api_key: Option<String>,
    /// Optional base URL override.
    pub base_url: Option<String>,
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

        Ok(app_config)
    }
}

/// Expand `${VAR_NAME}` patterns in a string using environment variables.
fn interpolate_env(s: &str) -> String {
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
    }
}
