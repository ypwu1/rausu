//! GitHub Copilot token manager.
//!
//! Two-step authentication:
//! 1. Load a GitHub OAuth device-flow token (`ghu_...`) from `hosts.json`.
//! 2. Exchange it for a short-lived Copilot API token via the
//!    `api.github.com/copilot_internal/v2/token` endpoint.
//!
//! Copilot API tokens are cached and re-exchanged automatically when they
//! approach expiry (within `REFRESH_MARGIN_SECS`).
//!
//! If `hosts.json` does not exist or lacks a token, and stdout is a TTY,
//! the manager will automatically initiate a GitHub Device Flow login so
//! the user can authorize without needing VS Code or `gh` CLI.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Re-exchange margin — get a new Copilot token 5 minutes before expiry.
const REFRESH_MARGIN_SECS: i64 = 5 * 60;

/// Default path to the Copilot hosts file relative to $HOME.
const DEFAULT_HOSTS_RELATIVE: &str = ".config/github-copilot/hosts.json";

/// GitHub API endpoint for exchanging a GitHub token for a Copilot API token.
const COPILOT_TOKEN_ENDPOINT: &str = "https://api.github.com/copilot_internal/v2/token";

/// Default Copilot API base URL (used when the token exchange does not return one).
const DEFAULT_COPILOT_ENDPOINT: &str = "https://api.githubcopilot.com";

/// GitHub OAuth client ID used by GitHub Copilot extensions.
const GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// GitHub Device Flow: request a device code.
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";

/// GitHub Device Flow: poll for the access token.
const DEVICE_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

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

/// Response from `POST /login/device/code`.
#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    /// Minimum polling interval in seconds.
    interval: Option<u64>,
    /// Seconds until the device code expires.
    #[allow(dead_code)]
    expires_in: u64,
}

/// Response from `POST /login/oauth/access_token` during device flow polling.
#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    /// Present on success.
    access_token: Option<String>,
    /// Present on error — one of `authorization_pending`, `slow_down`,
    /// `expired_token`, `access_denied`, etc.
    error: Option<String>,
}

/// Minimal structure written to `hosts.json`.
#[derive(Debug, Serialize)]
struct HostsFileWrite {
    #[serde(rename = "github.com")]
    github: HostEntryWrite,
}

#[derive(Debug, Serialize)]
struct HostEntryWrite {
    oauth_token: String,
    user: String,
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
/// Loads a GitHub OAuth device-flow token from `hosts.json`, exchanges it for a
/// Copilot API token, and caches it for re-use until near expiry.
pub struct CopilotTokenManager {
    client: Client,
    /// Optional override for the hosts.json path.
    hosts_path: Option<PathBuf>,
    state: RwLock<Option<CachedCopilotToken>>,
}

impl CopilotTokenManager {
    /// Create a new token manager wrapped in an `Arc`.
    ///
    /// `hosts_path` overrides the default `~/.config/github-copilot/hosts.json`.
    pub fn new(hosts_path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build Copilot HTTP client"),
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
        let github_token = self.load_or_login().await?;
        let cached = self.exchange_token(&github_token).await?;
        info!("Copilot API token acquired successfully");

        let result = (cached.api_token.clone(), cached.endpoint.clone());
        *state = Some(cached);
        Ok(result)
    }

    // ── GitHub token loading ──────────────────────────────────────────────────

    /// Load the raw GitHub OAuth device-flow token from `hosts.json`.
    ///
    /// If the file is missing and stdout is a TTY, automatically initiates
    /// GitHub Device Flow login and saves the resulting token.
    pub async fn load_or_login(&self) -> Result<String> {
        match self.load_from_hosts_file() {
            Ok(token) => Ok(token),
            Err(e) => {
                if !std::io::stdout().is_terminal() {
                    return Err(e);
                }
                info!("GitHub Copilot credentials not found, initiating login...");
                let token = device_flow_login(&self.client).await?;
                self.save_to_hosts_file(&token)?;
                info!("GitHub Copilot login successful! Token saved.");
                Ok(token)
            }
        }
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

        let hosts: HostsFile =
            serde_json::from_str(&contents).context("Failed to parse GitHub Copilot hosts.json")?;

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

    /// Save a GitHub OAuth token to `hosts.json`.
    fn save_to_hosts_file(&self, token: &str) -> Result<()> {
        let path = self
            .hosts_file_path()
            .context("Could not determine Copilot hosts.json path")?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let hosts = HostsFileWrite {
            github: HostEntryWrite {
                oauth_token: token.to_string(),
                user: "github".to_string(),
            },
        };

        let json =
            serde_json::to_string_pretty(&hosts).context("Failed to serialize hosts.json")?;
        std::fs::write(&path, json)
            .with_context(|| format!("Failed to write hosts file: {}", path.display()))?;

        debug!(path = %path.display(), "Saved GitHub token to hosts.json");
        Ok(())
    }

    // ── Token exchange ────────────────────────────────────────────────────────

    /// Exchange a GitHub OAuth token for a short-lived Copilot API token.
    async fn exchange_token(&self, github_token: &str) -> Result<CachedCopilotToken> {
        let response = self
            .client
            .get(COPILOT_TOKEN_ENDPOINT)
            .header("Authorization", format!("token {}", github_token))
            .header("Accept", "application/json")
            .header("User-Agent", "rausu/0.1 (github-copilot-provider)")
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

// ── Device Flow ──────────────────────────────────────────────────────────────

/// Perform GitHub Device Flow login and return the OAuth access token.
///
/// Prints a user code and verification URL to the terminal, then polls
/// until the user completes authorization (or the code expires).
pub async fn device_flow_login(client: &Client) -> Result<String> {
    // Step 1: Request a device code.
    let resp = client
        .post(DEVICE_CODE_URL)
        .header("accept", "application/json")
        .form(&[("client_id", GITHUB_CLIENT_ID), ("scope", "read:user")])
        .send()
        .await
        .context("Failed to request GitHub device code")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitHub device code request failed: {}", body);
    }

    let dc: DeviceCodeResponse = resp
        .json()
        .await
        .context("Failed to parse device code response")?;

    // Step 2: Print instructions.
    println!();
    println!("\u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}");
    println!("\u{2551}  GitHub Copilot Login Required           \u{2551}");
    println!("\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");
    println!("\u{2551}  1. Open: {}  ", dc.verification_uri);
    println!("\u{2551}  2. Enter code: {:<25}\u{2551}", dc.user_code);
    println!("\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}");
    println!();

    // Step 3: Poll for the access token.
    let mut interval_secs = dc.interval.unwrap_or(5);
    info!("Waiting for authorization...");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;

        let resp = client
            .post(DEVICE_TOKEN_URL)
            .header("accept", "application/json")
            .form(&[
                ("client_id", GITHUB_CLIENT_ID),
                ("device_code", dc.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .context("Failed to poll GitHub token endpoint")?;

        let dt: DeviceTokenResponse = resp
            .json()
            .await
            .context("Failed to parse token poll response")?;

        if let Some(token) = dt.access_token {
            return Ok(token);
        }

        match dt.error.as_deref() {
            Some("authorization_pending") => {
                // Expected — keep polling.
            }
            Some("slow_down") => {
                interval_secs += 5;
                debug!(interval_secs, "GitHub asked us to slow down");
            }
            Some("expired_token") => {
                anyhow::bail!("GitHub device code expired. Please restart Rausu to try again.");
            }
            Some("access_denied") => {
                anyhow::bail!("Authorization was denied by the user.");
            }
            Some(other) => {
                anyhow::bail!("GitHub device flow error: {}", other);
            }
            None => {
                anyhow::bail!(
                    "Unexpected response from GitHub token endpoint (no token, no error)"
                );
            }
        }
    }
}

/// Ensure that Copilot credentials are available, running device flow login
/// if necessary. Call this at server startup before binding the listener.
pub async fn ensure_copilot_credentials(token_manager: &CopilotTokenManager) -> Result<()> {
    // Try loading the token; if it fails and we're in a TTY, device flow
    // will be triggered automatically by `load_or_login`.
    let _ = token_manager.load_or_login().await?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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

        let mgr = CopilotTokenManager::new(Some(path.clone()));
        assert_eq!(mgr.load_from_hosts_file().unwrap(), "ghu_test_token_abc");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_load_from_hosts_file_missing() {
        let mgr = CopilotTokenManager::new(Some(PathBuf::from("/nonexistent/hosts.json")));
        assert!(mgr.load_from_hosts_file().is_err());
    }

    #[test]
    fn test_load_from_hosts_file_missing_github_key() {
        let json = serde_json::json!({ "other.com": { "oauth_token": "tok" } });
        let dir = std::env::temp_dir();
        let path = dir.join("rausu_test_copilot_hosts_nogithub.json");
        std::fs::write(&path, json.to_string()).unwrap();

        let mgr = CopilotTokenManager::new(Some(path.clone()));
        assert!(mgr.load_from_hosts_file().is_err());

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_default_endpoint() {
        assert_eq!(DEFAULT_COPILOT_ENDPOINT, "https://api.githubcopilot.com");
    }

    #[test]
    fn test_hosts_file_path_default() {
        let mgr = CopilotTokenManager::new(None);
        let path = mgr.hosts_file_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("github-copilot"));
        assert!(path.to_string_lossy().ends_with("hosts.json"));
    }
}
