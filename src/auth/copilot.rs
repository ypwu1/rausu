//! GitHub Copilot token manager.
//!
//! Two-step authentication:
//! 1. Load a GitHub OAuth token (from env var or `hosts.json`).
//! 2. Exchange it for a short-lived Copilot API token via the
//!    `api.github.com/copilot_internal/v2/token` endpoint.
//!
//! Copilot API tokens are cached and re-exchanged automatically when they
//! approach expiry (within `REFRESH_MARGIN_SECS`).
//!
//! # Token sources
//!
//! | `token_source` | Behaviour |
//! |----------------|-----------|
//! | `auto` (default) | Try `GH_TOKEN` / `GITHUB_TOKEN` env vars first, then `hosts.json` |
//! | `env`          | `GH_TOKEN` or `GITHUB_TOKEN` env var only |
//! | `hosts_file`   | `~/.config/github-copilot/hosts.json` (or custom path) |

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Re-exchange margin — get a new Copilot token 5 minutes before expiry.
const REFRESH_MARGIN_SECS: i64 = 5 * 60;

/// Environment variables checked for the GitHub OAuth token.
const GH_TOKEN_ENVS: &[&str] = &["GH_TOKEN", "GITHUB_TOKEN"];

/// Default path to the Copilot hosts file relative to $HOME.
const DEFAULT_HOSTS_RELATIVE: &str = ".config/github-copilot/hosts.json";

/// GitHub API endpoint for exchanging a GitHub token for a Copilot API token.
const COPILOT_TOKEN_ENDPOINT: &str =
    "https://api.github.com/copilot_internal/v2/token";

/// Default Copilot API base URL (used when the token exchange does not return one).
const DEFAULT_COPILOT_ENDPOINT: &str = "https://api.githubcopilot.com";

// ── Token source ──────────────────────────────────────────────────────────────

/// Determines where the GitHub OAuth token is loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopilotTokenSource {
    /// Try `GH_TOKEN` / `GITHUB_TOKEN` first, then hosts file.
    Auto,
    /// `GH_TOKEN` or `GITHUB_TOKEN` environment variable only.
    Env,
    /// `~/.config/github-copilot/hosts.json` (or a custom path).
    HostsFile,
}

// ── Wire types ────────────────────────────────────────────────────────────────

/// `~/.config/github-copilot/hosts.json` top-level structure.
///
/// Format: `{ "github.com": { "oauth_token": "ghu_...", "user": "..." } }`
#[derive(Debug, Deserialize)]
struct HostsFile {
    #[serde(rename = "github.com")]
    github: Option<HostEntry>,
}

#[derive(Debug, Deserialize)]
struct HostEntry {
    oauth_token: String,
}

/// Response from `GET /copilot_internal/v2/token`.
#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
    /// Unix timestamp (seconds) at which this token expires.
    expires_at: i64,
    /// Optional API endpoint override returned by the server.
    #[serde(default)]
    endpoints: Option<CopilotEndpoints>,
}

#[derive(Debug, Deserialize)]
struct CopilotEndpoints {
    api: Option<String>,
}

// ── Cached state ──────────────────────────────────────────────────────────────

/// An in-memory Copilot API token with its endpoint and expiry.
#[derive(Debug, Clone)]
pub struct CachedCopilotToken {
    /// The short-lived Copilot API bearer token.
    pub api_token: String,
    /// Base URL for Copilot API requests (e.g. `https://api.githubcopilot.com`).
    pub endpoint: String,
    /// Expiry time in Unix seconds.
    pub expires_at_secs: i64,
}

impl CachedCopilotToken {
    /// Returns `true` if the token has expired or is within the refresh margin.
    pub fn is_expired(&self) -> bool {
        chrono::Utc::now().timestamp() + REFRESH_MARGIN_SECS >= self.expires_at_secs
    }
}

// ── Manager ───────────────────────────────────────────────────────────────────

/// Thread-safe GitHub Copilot token manager.
///
/// Loads a GitHub OAuth token from the configured source, exchanges it for a
/// Copilot API token, and caches it for re-use until near expiry.
pub struct CopilotTokenManager {
    client: Client,
    token_source: CopilotTokenSource,
    /// Optional override for the hosts.json path.
    hosts_path: Option<PathBuf>,
    state: RwLock<Option<CachedCopilotToken>>,
}

impl CopilotTokenManager {
    /// Create a new token manager wrapped in an `Arc`.
    pub fn new(token_source: CopilotTokenSource, hosts_path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build Copilot HTTP client"),
            token_source,
            hosts_path,
            state: RwLock::new(None),
        })
    }

    /// Return `(api_token, endpoint)`, loading or re-exchanging as needed.
    ///
    /// `endpoint` is the base URL to use for Copilot API requests (without trailing slash).
    pub async fn get_token(&self) -> Result<(String, String)> {
        // Fast path: valid cached token.
        {
            let state = self.state.read().await;
            if let Some(cached) = &*state {
                if !cached.is_expired() {
                    return Ok((cached.api_token.clone(), cached.endpoint.clone()));
                }
            }
        }

        // Slow path: acquire write lock and re-exchange.
        let mut state = self.state.write().await;

        // Double-check after acquiring write lock (another task may have refreshed).
        if let Some(cached) = &*state {
            if !cached.is_expired() {
                return Ok((cached.api_token.clone(), cached.endpoint.clone()));
            }
        }

        debug!("Copilot API token expired or absent; re-exchanging");
        let github_token = self.load_github_token()?;
        let cached = self.exchange_token(&github_token).await?;
        info!("Copilot API token acquired successfully");

        let result = (cached.api_token.clone(), cached.endpoint.clone());
        *state = Some(cached);
        Ok(result)
    }

    // ── GitHub token loading ──────────────────────────────────────────────────

    /// Load the raw GitHub OAuth token from the configured source.
    pub fn load_github_token(&self) -> Result<String> {
        match &self.token_source {
            CopilotTokenSource::Env => self.load_from_env(),
            CopilotTokenSource::HostsFile => self.load_from_hosts_file(),
            CopilotTokenSource::Auto => {
                if let Ok(token) = self.load_from_env() {
                    return Ok(token);
                }
                self.load_from_hosts_file()
            }
        }
    }

    /// Load from `GH_TOKEN` or `GITHUB_TOKEN` environment variable.
    pub fn load_from_env(&self) -> Result<String> {
        for var in GH_TOKEN_ENVS {
            if let Ok(val) = std::env::var(var) {
                if !val.is_empty() {
                    return Ok(val);
                }
            }
        }
        anyhow::bail!(
            "Neither GH_TOKEN nor GITHUB_TOKEN environment variable is set"
        )
    }

    /// Load from `~/.config/github-copilot/hosts.json` (or custom path).
    pub fn load_from_hosts_file(&self) -> Result<String> {
        let path = self
            .hosts_file_path()
            .context("Could not determine Copilot hosts.json path")?;

        let contents = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "Failed to read GitHub Copilot hosts file: {}. \
                 Run `gh auth login` or install GitHub Copilot to create this file.",
                path.display()
            )
        })?;

        let hosts: HostsFile = serde_json::from_str(&contents)
            .context("Failed to parse GitHub Copilot hosts.json")?;

        hosts
            .github
            .map(|e| e.oauth_token)
            .filter(|t| !t.is_empty())
            .context(
                "No oauth_token found for github.com in hosts.json. \
                 Run `gh auth login` to authenticate.",
            )
    }

    fn hosts_file_path(&self) -> Option<PathBuf> {
        if let Some(p) = &self.hosts_path {
            return Some(p.clone());
        }
        dirs::home_dir().map(|home| home.join(DEFAULT_HOSTS_RELATIVE))
    }

    // ── Token exchange ────────────────────────────────────────────────────────

    /// Exchange a GitHub OAuth token for a short-lived Copilot API token.
    async fn exchange_token(&self, github_token: &str) -> Result<CachedCopilotToken> {
        let response = self
            .client
            .get(COPILOT_TOKEN_ENDPOINT)
            .header("Authorization", format!("token {}", github_token))
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                "rausu/0.1 (github-copilot-provider)",
            )
            .send()
            .await
            .context("Failed to reach GitHub Copilot token endpoint")?;

        let status = response.status();
        if !status.is_success() {
            let status_u16 = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            // Do NOT log the body verbatim here — it might include token fragments.
            warn!(status = status_u16, "Copilot token exchange failed");
            anyhow::bail!(
                "GitHub Copilot token exchange returned HTTP {}: {}",
                status_u16,
                body
            );
        }

        let resp: CopilotTokenResponse = response
            .json()
            .await
            .context("Failed to parse Copilot token response")?;

        let endpoint = resp
            .endpoints
            .as_ref()
            .and_then(|e| e.api.clone())
            .unwrap_or_else(|| DEFAULT_COPILOT_ENDPOINT.to_string());

        // Strip trailing slash for uniform URL construction.
        let endpoint = endpoint.trim_end_matches('/').to_string();

        Ok(CachedCopilotToken {
            api_token: resp.token,
            endpoint,
            expires_at_secs: resp.expires_at,
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_token_not_expired() {
        let cached = CachedCopilotToken {
            api_token: "tok".to_string(),
            endpoint: DEFAULT_COPILOT_ENDPOINT.to_string(),
            expires_at_secs: chrono::Utc::now().timestamp() + 3600,
        };
        assert!(!cached.is_expired());
    }

    #[test]
    fn test_token_expired() {
        let cached = CachedCopilotToken {
            api_token: "tok".to_string(),
            endpoint: DEFAULT_COPILOT_ENDPOINT.to_string(),
            expires_at_secs: chrono::Utc::now().timestamp() - 3600,
        };
        assert!(cached.is_expired());
    }

    #[test]
    fn test_token_within_refresh_margin() {
        let cached = CachedCopilotToken {
            api_token: "tok".to_string(),
            endpoint: DEFAULT_COPILOT_ENDPOINT.to_string(),
            // 3 minutes from now — within the 5-minute refresh margin.
            expires_at_secs: chrono::Utc::now().timestamp() + 3 * 60,
        };
        assert!(cached.is_expired());
    }

    #[test]
    fn test_load_from_env_gh_token() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("GH_TOKEN", "test-gh-token");
        std::env::remove_var("GITHUB_TOKEN");

        let mgr = CopilotTokenManager::new(CopilotTokenSource::Env, None);
        assert_eq!(mgr.load_from_env().unwrap(), "test-gh-token");

        std::env::remove_var("GH_TOKEN");
    }

    #[test]
    fn test_load_from_env_github_token_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("GH_TOKEN");
        std::env::set_var("GITHUB_TOKEN", "test-github-token");

        let mgr = CopilotTokenManager::new(CopilotTokenSource::Env, None);
        assert_eq!(mgr.load_from_env().unwrap(), "test-github-token");

        std::env::remove_var("GITHUB_TOKEN");
    }

    #[test]
    fn test_load_from_env_missing() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("GITHUB_TOKEN");

        let mgr = CopilotTokenManager::new(CopilotTokenSource::Env, None);
        assert!(mgr.load_from_env().is_err());
    }

    #[test]
    fn test_load_from_hosts_file() {
        let json = serde_json::json!({
            "github.com": {
                "user": "testuser",
                "oauth_token": "ghu_test_token_abc"
            }
        });
        let dir = std::env::temp_dir();
        let path = dir.join("rausu_test_copilot_hosts.json");
        std::fs::write(&path, json.to_string()).unwrap();

        let mgr = CopilotTokenManager::new(CopilotTokenSource::HostsFile, Some(path.clone()));
        assert_eq!(mgr.load_from_hosts_file().unwrap(), "ghu_test_token_abc");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_load_from_hosts_file_missing() {
        let mgr = CopilotTokenManager::new(
            CopilotTokenSource::HostsFile,
            Some(PathBuf::from("/nonexistent/hosts.json")),
        );
        assert!(mgr.load_from_hosts_file().is_err());
    }

    #[test]
    fn test_load_from_hosts_file_missing_github_key() {
        let json = serde_json::json!({ "other.com": { "oauth_token": "tok" } });
        let dir = std::env::temp_dir();
        let path = dir.join("rausu_test_copilot_hosts_nogithub.json");
        std::fs::write(&path, json.to_string()).unwrap();

        let mgr = CopilotTokenManager::new(CopilotTokenSource::HostsFile, Some(path.clone()));
        assert!(mgr.load_from_hosts_file().is_err());

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_auto_prefers_env_over_hosts_file() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("GH_TOKEN", "env-token");

        let mgr = CopilotTokenManager::new(CopilotTokenSource::Auto, None);
        // Auto should return the env token without trying hosts file.
        assert_eq!(mgr.load_github_token().unwrap(), "env-token");

        std::env::remove_var("GH_TOKEN");
    }

    #[test]
    fn test_default_endpoint() {
        assert_eq!(DEFAULT_COPILOT_ENDPOINT, "https://api.githubcopilot.com");
    }

    #[test]
    fn test_hosts_file_path_default() {
        let mgr = CopilotTokenManager::new(CopilotTokenSource::Auto, None);
        let path = mgr.hosts_file_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("github-copilot"));
        assert!(path.to_string_lossy().ends_with("hosts.json"));
    }
}
