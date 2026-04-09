//! Shared configuration validation used by `rausu check`, `rausu setup`, and
//! the normal startup path.
//!
//! Distinguishes **hard errors** (block startup) from **warnings** (report but
//! continue).

use crate::config::schema::{AppConfig, ModelConfig, ProviderDeployment};

/// Known provider types.
pub const VALID_PROVIDERS: &[&str] = &[
    "openai",
    "openrouter",
    "anthropic",
    "claude-subscription",
    "chatgpt-subscription",
    "github-copilot",
    "azure-openai",
    "vertex-ai",
    "google-ai-studio",
    "bedrock",
];

/// Valid token sources for subscription-based providers.
pub const VALID_TOKEN_SOURCES_CLAUDE: &[&str] = &["auto", "env", "credentials_file"];
pub const VALID_TOKEN_SOURCES_CHATGPT: &[&str] =
    &["auto", "env", "codex", "device_flow", "credentials_file"];

// ── Result types ────────────────────────────────────────────────────────────

/// A single validation finding.
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub context: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocks startup / save.
    Error,
    /// Reported but does not block.
    Warning,
}

/// Aggregated validation result.
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    pub issues: Vec<ValidationIssue>,
}

impl ValidationResult {
    pub fn has_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    pub fn has_warnings(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Warning)
    }

    pub fn errors(&self) -> Vec<&ValidationIssue> {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect()
    }

    pub fn warnings(&self) -> Vec<&ValidationIssue> {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .collect()
    }

    fn push_error(&mut self, context: impl Into<String>, message: impl Into<String>) {
        self.issues.push(ValidationIssue {
            severity: Severity::Error,
            context: context.into(),
            message: message.into(),
        });
    }

    fn push_warning(&mut self, context: impl Into<String>, message: impl Into<String>) {
        self.issues.push(ValidationIssue {
            severity: Severity::Warning,
            context: context.into(),
            message: message.into(),
        });
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Validate the full application config.
///
/// Returns a [`ValidationResult`] containing all errors and warnings.
/// Callers decide how to present them and whether to proceed.
pub fn validate_config(config: &AppConfig) -> ValidationResult {
    let mut result = ValidationResult::default();

    // Auth validation
    validate_auth(config, &mut result);

    // Model-level validation
    if config.models.is_empty() {
        result.push_warning("models", "no models configured");
    }

    let mut seen_names: Vec<String> = Vec::new();
    let mut seen_aliases: Vec<String> = Vec::new();

    for model in &config.models {
        validate_model(model, &mut seen_names, &mut seen_aliases, &mut result);
    }

    // TLS validation (file existence is a soft check — files may not be
    // present on the machine running `rausu setup`)
    if let Some(tls) = &config.server.tls {
        validate_tls_files(tls, &mut result);
    }

    result
}

/// Validate a single model entry (also usable from setup for per-model checks).
#[allow(dead_code)]
pub fn validate_model_entry(model: &ModelConfig) -> ValidationResult {
    let mut result = ValidationResult::default();
    let mut seen_names = Vec::new();
    let mut seen_aliases = Vec::new();
    validate_model(model, &mut seen_names, &mut seen_aliases, &mut result);
    result
}

// ── Internal helpers ────────────────────────────────────────────────────────

fn validate_auth(config: &AppConfig, result: &mut ValidationResult) {
    if config.auth.mode == "static" && config.auth.keys.is_empty() {
        result.push_error("auth", "mode is 'static' but no keys are configured");
    }
}

fn validate_model(
    model: &ModelConfig,
    seen_names: &mut Vec<String>,
    seen_aliases: &mut Vec<String>,
    result: &mut ValidationResult,
) {
    let ctx = format!("model '{}'", model.name);

    // Empty model name
    if model.name.trim().is_empty() {
        result.push_error("models", "model name is empty");
        return;
    }

    // Duplicate model name
    if seen_names.contains(&model.name) {
        result.push_error(&ctx, "duplicate model name");
    } else {
        seen_names.push(model.name.clone());
    }

    // Duplicate aliases
    if let Some(aliases) = &model.aliases {
        for alias in aliases {
            if alias.trim().is_empty() {
                result.push_error(&ctx, "empty alias");
            } else if seen_names.contains(alias) || seen_aliases.contains(alias) {
                result.push_error(&ctx, format!("duplicate alias '{alias}'"));
            } else {
                seen_aliases.push(alias.clone());
            }
        }
    }

    // No providers
    if model.providers.is_empty() {
        result.push_error(&ctx, "no provider deployments configured");
        return;
    }

    for (i, deployment) in model.providers.iter().enumerate() {
        validate_deployment(deployment, &ctx, i, result);
    }
}

fn validate_deployment(
    d: &ProviderDeployment,
    model_ctx: &str,
    _index: usize,
    result: &mut ValidationResult,
) {
    let ctx = format!("{model_ctx}/{}", d.provider);

    // Unknown provider
    if !VALID_PROVIDERS.contains(&d.provider.as_str()) {
        result.push_error(&ctx, format!("unknown provider type '{}'", d.provider));
        return;
    }

    // Empty upstream model
    if d.model.trim().is_empty() {
        result.push_error(&ctx, "upstream model name is empty");
    }

    // Provider-specific checks
    match d.provider.as_str() {
        "openai" | "openrouter" | "anthropic" | "google-ai-studio" => {
            if d.api_key.as_ref().is_none_or(|k| k.is_empty()) {
                result.push_warning(&ctx, "no api_key configured");
            }
        }
        "azure-openai" => {
            if d.api_key.as_ref().is_none_or(|k| k.is_empty()) {
                result.push_warning(&ctx, "no api_key configured");
            }
            if d.base_url.as_ref().is_none_or(|u| u.is_empty()) {
                result.push_error(
                    &ctx,
                    "base_url is required for azure-openai (e.g. https://<resource>.openai.azure.com/)",
                );
            }
        }
        "vertex-ai" => {
            if d.project_id.is_none() {
                result.push_error(&ctx, "project_id is required");
            }
        }
        "bedrock" => {
            if d.region.as_ref().is_none_or(|r| r.is_empty()) {
                result.push_error(&ctx, "region is required for bedrock (e.g. us-east-1)");
            }
        }
        "claude-subscription" => {
            if let Some(ts) = &d.token_source {
                if !VALID_TOKEN_SOURCES_CLAUDE.contains(&ts.as_str()) {
                    result.push_error(&ctx, format!("invalid token_source '{ts}'"));
                }
            }
        }
        "chatgpt-subscription" => {
            if let Some(ts) = &d.token_source {
                if !VALID_TOKEN_SOURCES_CHATGPT.contains(&ts.as_str()) {
                    result.push_error(&ctx, format!("invalid token_source '{ts}'"));
                }
            }
        }
        _ => {}
    }

    // Credential file existence (soft warning)
    if let Some(path) = &d.credentials_path {
        if !path.is_empty() && !std::path::Path::new(path).exists() {
            result.push_warning(&ctx, format!("credentials file not found: {path}"));
        }
    }
}

fn validate_tls_files(tls: &crate::config::schema::TlsConfig, result: &mut ValidationResult) {
    check_file_exists(&tls.cert_file, "TLS certificate", result);
    check_file_exists(&tls.key_file, "TLS private key", result);
    if let Some(ca) = &tls.client_ca_file {
        check_file_exists(ca, "client CA certificate", result);
    }
}

fn check_file_exists(path: &str, description: &str, result: &mut ValidationResult) {
    let p = std::path::Path::new(path);
    if !p.exists() {
        result.push_warning("tls", format!("{description} not found: {path}"));
    } else if std::fs::metadata(p).is_ok_and(|m| m.len() == 0) {
        result.push_error("tls", format!("{description} is empty: {path}"));
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::*;

    fn minimal_config(models: Vec<ModelConfig>) -> AppConfig {
        AppConfig {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            auth: AuthConfig::default(),
            models,
        }
    }

    fn deployment(provider: &str, model: &str) -> ProviderDeployment {
        ProviderDeployment {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: Some("test-key".to_string()),
            base_url: None,
            token_source: None,
            credentials_path: None,
            api_version: None,
            project_id: None,
            location: None,
            region: None,
        }
    }

    #[test]
    fn test_valid_config() {
        let config = minimal_config(vec![ModelConfig {
            name: "gpt-4o".to_string(),
            aliases: None,
            providers: vec![deployment("openai", "gpt-4o")],
        }]);
        let result = validate_config(&config);
        assert!(!result.has_errors());
    }

    #[test]
    fn test_empty_model_name() {
        let config = minimal_config(vec![ModelConfig {
            name: "".to_string(),
            aliases: None,
            providers: vec![deployment("openai", "gpt-4o")],
        }]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result.errors().iter().any(|i| i.message.contains("empty")));
    }

    #[test]
    fn test_duplicate_model_names() {
        let config = minimal_config(vec![
            ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![deployment("openai", "gpt-4o")],
            },
            ModelConfig {
                name: "gpt-4o".to_string(),
                aliases: None,
                providers: vec![deployment("openai", "gpt-4o-2")],
            },
        ]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("duplicate model name")));
    }

    #[test]
    fn test_duplicate_aliases() {
        let config = minimal_config(vec![
            ModelConfig {
                name: "model-a".to_string(),
                aliases: Some(vec!["shared".to_string()]),
                providers: vec![deployment("openai", "a")],
            },
            ModelConfig {
                name: "model-b".to_string(),
                aliases: Some(vec!["shared".to_string()]),
                providers: vec![deployment("openai", "b")],
            },
        ]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("duplicate alias")));
    }

    #[test]
    fn test_unknown_provider_type() {
        let config = minimal_config(vec![ModelConfig {
            name: "test".to_string(),
            aliases: None,
            providers: vec![deployment("unknown-provider", "test")],
        }]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("unknown provider type")));
    }

    #[test]
    fn test_empty_upstream_model() {
        let config = minimal_config(vec![ModelConfig {
            name: "test".to_string(),
            aliases: None,
            providers: vec![deployment("openai", "")],
        }]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("upstream model name is empty")));
    }

    #[test]
    fn test_vertex_missing_project_id() {
        let config = minimal_config(vec![ModelConfig {
            name: "gemini".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "vertex-ai".to_string(),
                model: "gemini-2.5-pro".to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                credentials_path: None,
                api_version: None,
                project_id: None,
                location: None,
                region: None,
            }],
        }]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("project_id")));
    }

    #[test]
    fn test_invalid_token_source_claude() {
        let config = minimal_config(vec![ModelConfig {
            name: "claude".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "claude-subscription".to_string(),
                model: "claude-sonnet-4-6".to_string(),
                api_key: None,
                base_url: None,
                token_source: Some("bogus".to_string()),
                credentials_path: None,
                api_version: None,
                project_id: None,
                location: None,
                region: None,
            }],
        }]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("invalid token_source")));
    }

    #[test]
    fn test_invalid_token_source_chatgpt() {
        let config = minimal_config(vec![ModelConfig {
            name: "gpt".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "chatgpt-subscription".to_string(),
                model: "gpt-5".to_string(),
                api_key: None,
                base_url: None,
                token_source: Some("invalid".to_string()),
                credentials_path: None,
                api_version: None,
                project_id: None,
                location: None,
                region: None,
            }],
        }]);
        let result = validate_config(&config);
        assert!(result.has_errors());
    }

    #[test]
    fn test_no_providers_is_error() {
        let config = minimal_config(vec![ModelConfig {
            name: "empty".to_string(),
            aliases: None,
            providers: vec![],
        }]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("no provider deployments")));
    }

    #[test]
    fn test_no_models_is_warning() {
        let config = minimal_config(vec![]);
        let result = validate_config(&config);
        assert!(!result.has_errors());
        assert!(result.has_warnings());
    }

    #[test]
    fn test_missing_api_key_is_warning() {
        let config = minimal_config(vec![ModelConfig {
            name: "test".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                credentials_path: None,
                api_version: None,
                project_id: None,
                location: None,
                region: None,
            }],
        }]);
        let result = validate_config(&config);
        assert!(!result.has_errors());
        assert!(result.has_warnings());
    }

    #[test]
    fn test_auth_static_no_keys_is_error() {
        let mut config = minimal_config(vec![ModelConfig {
            name: "test".to_string(),
            aliases: None,
            providers: vec![deployment("openai", "gpt-4o")],
        }]);
        config.auth.mode = "static".to_string();
        config.auth.keys = vec![];
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("no keys")));
    }

    #[test]
    fn test_validate_model_entry_standalone() {
        let model = ModelConfig {
            name: "test".to_string(),
            aliases: None,
            providers: vec![deployment("openai", "gpt-4o")],
        };
        let result = validate_model_entry(&model);
        assert!(!result.has_errors());
    }

    #[test]
    fn test_valid_token_sources_accepted() {
        for ts in VALID_TOKEN_SOURCES_CLAUDE {
            let config = minimal_config(vec![ModelConfig {
                name: format!("claude-{ts}"),
                aliases: None,
                providers: vec![ProviderDeployment {
                    provider: "claude-subscription".to_string(),
                    model: "claude-sonnet-4-6".to_string(),
                    api_key: None,
                    base_url: None,
                    token_source: Some(ts.to_string()),
                    credentials_path: None,
                    api_version: None,
                    project_id: None,
                    location: None,
                    region: None,
                }],
            }]);
            let result = validate_config(&config);
            assert!(
                !result.has_errors(),
                "token_source '{ts}' should be valid for claude-subscription"
            );
        }
    }

    #[test]
    fn test_bedrock_missing_region() {
        let config = minimal_config(vec![ModelConfig {
            name: "claude-bedrock".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "bedrock".to_string(),
                model: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                credentials_path: None,
                api_version: None,
                project_id: None,
                location: None,
                region: None,
            }],
        }]);
        let result = validate_config(&config);
        assert!(result.has_errors());
        assert!(result
            .errors()
            .iter()
            .any(|i| i.message.contains("region is required")));
    }

    #[test]
    fn test_bedrock_with_region_valid() {
        let config = minimal_config(vec![ModelConfig {
            name: "claude-bedrock".to_string(),
            aliases: None,
            providers: vec![ProviderDeployment {
                provider: "bedrock".to_string(),
                model: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
                api_key: None,
                base_url: None,
                token_source: None,
                credentials_path: None,
                api_version: None,
                project_id: None,
                location: None,
                region: Some("us-east-1".to_string()),
            }],
        }]);
        let result = validate_config(&config);
        assert!(!result.has_errors());
    }
}
