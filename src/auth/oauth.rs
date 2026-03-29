//! OAuth token manager for Claude subscription authentication.
//!
//! Supports loading tokens from environment variables or the Claude CLI
//! credentials file, and automatically refreshes expired tokens.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Refresh margin — refresh 5 minutes before actual expiry.
const REFRESH_MARGIN_MS: i64 = 5 * 60 * 1000;

/// Environment variable name for a static OAuth access token.
const CLAUDE_OAUTH_TOKEN_ENV: &str = "CLAUDE_OAUTH_TOKEN";

/// Default credentials file relative to the home directory.
const DEFAULT_CREDENTIALS_RELATIVE: &str = ".claude/.credentials.json";

/// OAuth metadata discovery URL for Claude CLI clients.
const OAUTH_METADATA_URL: &str = "https://claude.ai/oauth/claude-code-client-metadata";

// ── Token source ─────────────────────────────────────────────────────────────

/// Determines where the OAuth token is loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// Use the `CLAUDE_OAUTH_TOKEN` environment variable (no refresh).
    Env,
    /// Use `~/.claude/.credentials.json` (or a custom path).
    CredentialsFile,
    /// Auto-detect: try env first, then credentials file.
    Auto,
}

// ── Token state ───────────────────────────────────────────────────────────────

/// An in-memory OAuth token.
#[derive(Debug, Clone)]
pub struct OAuthToken {
    /// Bearer access token.
    pub access_token: String,
    /// Refresh token, if available.
    pub refresh_token: Option<String>,
    /// Expiry time in Unix milliseconds. `None` means no expiry.
    pub expires_at_ms: Option<i64>,
}

impl OAuthToken {
    /// Returns `true` if the token has expired or is within the refresh margin.
    pub fn is_expired(&self) -> bool {
        match self.expires_at_ms {
            Some(expires_at_ms) => {
                let now_ms = chrono::Utc::now().timestamp_millis();
                now_ms + REFRESH_MARGIN_MS >= expires_at_ms
            }
            None => false,
        }
    }
}

// ── Wire types ────────────────────────────────────────────────────────────────

/// `~/.claude/.credentials.json` top-level structure.
#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: ClaudeAiOauth,
}

/// OAuth section inside the credentials file.
#[derive(Debug, Deserialize)]
struct ClaudeAiOauth {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken", default)]
    refresh_token: Option<String>,
    /// Expiry in Unix milliseconds.
    #[serde(rename = "expiresAt", default)]
    expires_at: Option<i64>,
}

/// OAuth metadata endpoint response.
#[derive(Debug, Deserialize)]
struct OAuthMetadata {
    token_endpoint: String,
    #[serde(default)]
    client_id: Option<String>,
}

/// Standard OAuth2 token response.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Lifetime in seconds.
    #[serde(default)]
    expires_in: Option<i64>,
}

// ── Manager ───────────────────────────────────────────────────────────────────

/// Thread-safe OAuth token manager.
///
/// Handles token loading and automatic refresh. Construct via [`OAuthTokenManager::new`]
/// which returns an `Arc<Self>` suitable for sharing across async tasks.
pub struct OAuthTokenManager {
    client: Client,
    token_source: TokenSource,
    credentials_path: Option<PathBuf>,
    state: RwLock<Option<OAuthToken>>,
}

impl OAuthTokenManager {
    /// Create a new token manager wrapped in an `Arc`.
    pub fn new(token_source: TokenSource, credentials_path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            client: Client::new(),
            token_source,
            credentials_path,
            state: RwLock::new(None),
        })
    }

    /// Return a valid access token, loading or refreshing as needed.
    pub async fn get_token(&self) -> Result<String> {
        // Fast path: read lock — return token if still valid.
        {
            let state = self.state.read().await;
            if let Some(token) = &*state {
                if !token.is_expired() {
                    return Ok(token.access_token.clone());
                }
            }
        }

        // Slow path: write lock — refresh or reload.
        let mut state = self.state.write().await;

        // Double-check after acquiring write lock.
        if let Some(token) = &*state {
            if !token.is_expired() {
                return Ok(token.access_token.clone());
            }
            // Attempt refresh if we have a refresh token.
            if let Some(refresh_token) = token.refresh_token.clone() {
                debug!("OAuth token expired, attempting refresh");
                match self.do_refresh(&refresh_token).await {
                    Ok(new_token) => {
                        info!("OAuth token refreshed successfully");
                        let access = new_token.access_token.clone();
                        *state = Some(new_token);
                        return Ok(access);
                    }
                    Err(e) => {
                        warn!(error = %e, "Token refresh failed; reloading from source");
                    }
                }
            }
        }

        // Load a fresh token from the configured source.
        let token = self.load_token().await?;
        let access = token.access_token.clone();
        *state = Some(token);
        Ok(access)
    }

    // ── Loading ───────────────────────────────────────────────────────────────

    async fn load_token(&self) -> Result<OAuthToken> {
        match &self.token_source {
            TokenSource::Env => self.load_from_env(),
            TokenSource::CredentialsFile => self.load_from_credentials_file(),
            TokenSource::Auto => {
                if let Ok(token) = self.load_from_env() {
                    return Ok(token);
                }
                self.load_from_credentials_file()
            }
        }
    }

    /// Load a static token from the `CLAUDE_OAUTH_TOKEN` environment variable.
    fn load_from_env(&self) -> Result<OAuthToken> {
        let token = std::env::var(CLAUDE_OAUTH_TOKEN_ENV)
            .context("CLAUDE_OAUTH_TOKEN environment variable not set")?;
        Ok(OAuthToken {
            access_token: token,
            refresh_token: None,
            expires_at_ms: None,
        })
    }

    /// Load a token from `~/.claude/.credentials.json` (or custom path).
    pub fn load_from_credentials_file(&self) -> Result<OAuthToken> {
        let path = self
            .credentials_path()
            .context("Could not determine credentials file path")?;
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read credentials file: {}", path.display()))?;
        let creds: CredentialsFile =
            serde_json::from_str(&contents).context("Failed to parse credentials file")?;
        Ok(OAuthToken {
            access_token: creds.claude_ai_oauth.access_token,
            refresh_token: creds.claude_ai_oauth.refresh_token,
            expires_at_ms: creds.claude_ai_oauth.expires_at,
        })
    }

    /// Resolve the credentials file path (custom override or default).
    fn credentials_path(&self) -> Option<PathBuf> {
        if let Some(path) = &self.credentials_path {
            return Some(path.clone());
        }
        dirs::home_dir().map(|home| home.join(DEFAULT_CREDENTIALS_RELATIVE))
    }

    // ── Refresh ───────────────────────────────────────────────────────────────

    /// Perform an OAuth2 refresh_token grant.
    async fn do_refresh(&self, refresh_token: &str) -> Result<OAuthToken> {
        let metadata = self.fetch_oauth_metadata().await?;

        let mut form: Vec<(&str, String)> = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token.to_string()),
        ];
        if let Some(client_id) = &metadata.client_id {
            form.push(("client_id", client_id.clone()));
        }

        let response = self
            .client
            .post(&metadata.token_endpoint)
            .form(&form)
            .send()
            .await
            .context("Failed to send token refresh request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Token refresh returned {}: {}", status, body);
        }

        let token_resp: TokenResponse = response
            .json()
            .await
            .context("Failed to parse token refresh response")?;

        let expires_at_ms = token_resp.expires_in.map(|secs| {
            chrono::Utc::now().timestamp_millis() + secs * 1000
        });

        Ok(OAuthToken {
            access_token: token_resp.access_token,
            refresh_token: token_resp
                .refresh_token
                .or_else(|| Some(refresh_token.to_string())),
            expires_at_ms,
        })
    }

    /// Fetch the OAuth metadata document to discover the token endpoint.
    async fn fetch_oauth_metadata(&self) -> Result<OAuthMetadata> {
        let response = self
            .client
            .get(OAUTH_METADATA_URL)
            .send()
            .await
            .context("Failed to fetch OAuth metadata")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "OAuth metadata fetch returned {}",
                response.status()
            );
        }

        response
            .json::<OAuthMetadata>()
            .await
            .context("Failed to parse OAuth metadata")
    }
}

// ── Interactive OAuth stub ────────────────────────────────────────────────────

/// Start an interactive OAuth browser flow (not yet implemented).
///
/// This is a stub for a future phase. Calling it always returns an error.
#[allow(dead_code)]
pub async fn interactive_oauth_flow() -> Result<OAuthToken> {
    anyhow::bail!("Interactive OAuth flow is not yet implemented")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that mutate the process environment.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_token_not_expired() {
        let token = OAuthToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            // Expires 1 hour from now
            expires_at_ms: Some(chrono::Utc::now().timestamp_millis() + 3_600_000),
        };
        assert!(!token.is_expired());
    }

    #[test]
    fn test_token_expired() {
        let token = OAuthToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            // Expired 1 hour ago
            expires_at_ms: Some(chrono::Utc::now().timestamp_millis() - 3_600_000),
        };
        assert!(token.is_expired());
    }

    #[test]
    fn test_token_within_refresh_margin() {
        let token = OAuthToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            // Expires in 3 minutes (within the 5-minute margin)
            expires_at_ms: Some(chrono::Utc::now().timestamp_millis() + 3 * 60 * 1000),
        };
        assert!(token.is_expired());
    }

    #[test]
    fn test_token_no_expiry_never_expires() {
        let token = OAuthToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at_ms: None,
        };
        assert!(!token.is_expired());
    }

    #[test]
    fn test_load_from_env() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("CLAUDE_OAUTH_TOKEN", "test-oauth-token");
        let manager = OAuthTokenManager::new(TokenSource::Env, None);
        let token = manager.load_from_env().unwrap();
        assert_eq!(token.access_token, "test-oauth-token");
        assert!(token.refresh_token.is_none());
        assert!(token.expires_at_ms.is_none());
        std::env::remove_var("CLAUDE_OAUTH_TOKEN");
    }

    #[test]
    fn test_load_from_env_missing() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("CLAUDE_OAUTH_TOKEN");
        let manager = OAuthTokenManager::new(TokenSource::Env, None);
        assert!(manager.load_from_env().is_err());
    }

    #[test]
    fn test_load_from_credentials_file() {
        let json = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "access-abc",
                "refreshToken": "refresh-xyz",
                "expiresAt": 1_743_000_000_000i64,
                "subscriptionType": "pro"
            }
        });

        let dir = std::env::temp_dir();
        let path = dir.join("rausu_test_credentials.json");
        std::fs::write(&path, json.to_string()).unwrap();

        let manager = OAuthTokenManager::new(TokenSource::CredentialsFile, Some(path.clone()));
        let token = manager.load_from_credentials_file().unwrap();

        assert_eq!(token.access_token, "access-abc");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-xyz"));
        assert_eq!(token.expires_at_ms, Some(1_743_000_000_000));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_load_from_credentials_file_missing_path() {
        let manager = OAuthTokenManager::new(
            TokenSource::CredentialsFile,
            Some(PathBuf::from("/nonexistent/path/credentials.json")),
        );
        assert!(manager.load_from_credentials_file().is_err());
    }
}
