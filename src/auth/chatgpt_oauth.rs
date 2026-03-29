//! ChatGPT OAuth token manager.
//!
//! Supports loading tokens from environment variables or a credentials file,
//! with automatic refresh via the OpenAI OAuth token endpoint.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Refresh margin — refresh 5 minutes before actual expiry.
const REFRESH_MARGIN_MS: i64 = 5 * 60 * 1_000;

/// Environment variable for a static ChatGPT access token.
const CHATGPT_ACCESS_TOKEN_ENV: &str = "CHATGPT_ACCESS_TOKEN";

/// Environment variable for a ChatGPT refresh token.
const CHATGPT_REFRESH_TOKEN_ENV: &str = "CHATGPT_REFRESH_TOKEN";

/// Environment variable to supply the account ID directly (skips JWT decode).
const CHATGPT_ACCOUNT_ID_ENV: &str = "CHATGPT_ACCOUNT_ID";

/// Default credentials file relative to the home directory.
const DEFAULT_CREDENTIALS_RELATIVE: &str = ".config/rausu/chatgpt-auth.json";

/// Token endpoint for refresh grants.
const TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";

/// OAuth client ID used by the ChatGPT pi client.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// JWT claim key for the ChatGPT account ID (nested under the auth namespace).
const JWT_AUTH_NS: &str = "https://api.openai.com/auth";

// ── Token source ──────────────────────────────────────────────────────────────

/// Determines where the ChatGPT OAuth token is loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatGptTokenSource {
    /// Use the `CHATGPT_ACCESS_TOKEN` environment variable (no refresh).
    Env,
    /// Use `~/.config/rausu/chatgpt-auth.json` (or a custom path).
    CredentialsFile,
    /// Auto-detect: try env first, then credentials file.
    Auto,
}

// ── Token state ───────────────────────────────────────────────────────────────

/// An in-memory ChatGPT OAuth token plus extracted account ID.
#[derive(Debug, Clone)]
pub struct ChatGptToken {
    /// Bearer access token.
    pub access_token: String,
    /// Refresh token, if available.
    pub refresh_token: Option<String>,
    /// Expiry time in Unix milliseconds. `None` means no expiry.
    pub expires_at_ms: Option<i64>,
    /// ChatGPT account ID extracted from the JWT.
    pub account_id: Option<String>,
}

impl ChatGptToken {
    /// Returns `true` if the token has expired or is within the refresh margin.
    pub fn is_expired(&self) -> bool {
        match self.expires_at_ms {
            Some(exp) => chrono::Utc::now().timestamp_millis() + REFRESH_MARGIN_MS >= exp,
            None => false,
        }
    }
}

// ── Wire types ────────────────────────────────────────────────────────────────

/// `~/.config/rausu/chatgpt-auth.json` structure.
#[derive(Debug, Deserialize)]
struct CredentialsFile {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Expiry in Unix milliseconds.
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    account_id: Option<String>,
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

// ── JWT decode ────────────────────────────────────────────────────────────────

/// Decode the JWT payload and extract the ChatGPT account ID.
///
/// The account ID lives at `["https://api.openai.com/auth"]["chatgpt_account_id"]`.
pub fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload_b64 = parts[1];
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    json.get(JWT_AUTH_NS)
        .and_then(|v| v.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

// ── Manager ───────────────────────────────────────────────────────────────────

/// Thread-safe ChatGPT OAuth token manager.
pub struct ChatGptOAuthTokenManager {
    client: Client,
    token_source: ChatGptTokenSource,
    credentials_path: Option<PathBuf>,
    state: RwLock<Option<ChatGptToken>>,
}

impl ChatGptOAuthTokenManager {
    /// Create a new token manager wrapped in an `Arc`.
    pub fn new(token_source: ChatGptTokenSource, credentials_path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            client: Client::new(),
            token_source,
            credentials_path,
            state: RwLock::new(None),
        })
    }

    /// Return `(access_token, account_id)`, loading or refreshing as needed.
    pub async fn get_token(&self) -> Result<(String, Option<String>)> {
        // Fast path: read lock.
        {
            let state = self.state.read().await;
            if let Some(token) = &*state {
                if !token.is_expired() {
                    return Ok((token.access_token.clone(), token.account_id.clone()));
                }
            }
        }

        // Slow path: write lock.
        let mut state = self.state.write().await;

        // Double-check after acquiring write lock.
        if let Some(token) = &*state {
            if !token.is_expired() {
                return Ok((token.access_token.clone(), token.account_id.clone()));
            }
            if let Some(refresh_token) = token.refresh_token.clone() {
                debug!("ChatGPT token expired, attempting refresh");
                match self.do_refresh(&refresh_token).await {
                    Ok(new_token) => {
                        info!("ChatGPT token refreshed successfully");
                        let access = new_token.access_token.clone();
                        let account_id = new_token.account_id.clone();
                        *state = Some(new_token);
                        return Ok((access, account_id));
                    }
                    Err(e) => {
                        warn!(error = %e, "ChatGPT token refresh failed; reloading from source");
                    }
                }
            }
        }

        let token = self.load_token()?;
        let access = token.access_token.clone();
        let account_id = token.account_id.clone();
        *state = Some(token);
        Ok((access, account_id))
    }

    // ── Loading ───────────────────────────────────────────────────────────────

    fn load_token(&self) -> Result<ChatGptToken> {
        match &self.token_source {
            ChatGptTokenSource::Env => self.load_from_env(),
            ChatGptTokenSource::CredentialsFile => self.load_from_credentials_file(),
            ChatGptTokenSource::Auto => {
                if let Ok(token) = self.load_from_env() {
                    return Ok(token);
                }
                self.load_from_credentials_file()
            }
        }
    }

    /// Load from environment variables.
    pub fn load_from_env(&self) -> Result<ChatGptToken> {
        let access_token = std::env::var(CHATGPT_ACCESS_TOKEN_ENV)
            .context("CHATGPT_ACCESS_TOKEN environment variable not set")?;
        let refresh_token = std::env::var(CHATGPT_REFRESH_TOKEN_ENV).ok();
        let account_id = std::env::var(CHATGPT_ACCOUNT_ID_ENV)
            .ok()
            .or_else(|| extract_account_id_from_jwt(&access_token));
        Ok(ChatGptToken {
            access_token,
            refresh_token,
            expires_at_ms: None,
            account_id,
        })
    }

    /// Load from `~/.config/rausu/chatgpt-auth.json` (or custom path).
    pub fn load_from_credentials_file(&self) -> Result<ChatGptToken> {
        let path = self
            .credentials_path()
            .context("Could not determine chatgpt credentials file path")?;
        let contents = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "Failed to read chatgpt credentials file: {}",
                path.display()
            )
        })?;
        let creds: CredentialsFile =
            serde_json::from_str(&contents).context("Failed to parse chatgpt credentials file")?;
        let account_id = creds
            .account_id
            .or_else(|| extract_account_id_from_jwt(&creds.access_token));
        Ok(ChatGptToken {
            access_token: creds.access_token,
            refresh_token: creds.refresh_token,
            expires_at_ms: creds.expires_at,
            account_id,
        })
    }

    fn credentials_path(&self) -> Option<PathBuf> {
        if let Some(p) = &self.credentials_path {
            return Some(p.clone());
        }
        dirs::home_dir().map(|home| home.join(DEFAULT_CREDENTIALS_RELATIVE))
    }

    // ── Refresh ───────────────────────────────────────────────────────────────

    async fn do_refresh(&self, refresh_token: &str) -> Result<ChatGptToken> {
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ];

        let response = self
            .client
            .post(TOKEN_ENDPOINT)
            .form(&form)
            .send()
            .await
            .context("Failed to send ChatGPT token refresh request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("ChatGPT token refresh returned {}: {}", status, body);
        }

        let token_resp: TokenResponse = response
            .json()
            .await
            .context("Failed to parse ChatGPT token refresh response")?;

        let expires_at_ms = token_resp
            .expires_in
            .map(|secs| chrono::Utc::now().timestamp_millis() + secs * 1_000);

        let access_token = token_resp.access_token;
        let account_id = extract_account_id_from_jwt(&access_token);

        Ok(ChatGptToken {
            refresh_token: token_resp
                .refresh_token
                .or_else(|| Some(refresh_token.to_string())),
            access_token,
            expires_at_ms,
            account_id,
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // A minimal JWT with payload: {"https://api.openai.com/auth":{"chatgpt_account_id":"acc_test123"}}
    // header.payload.sig — payload is base64url of the JSON above (no padding)
    fn make_test_jwt() -> String {
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc_test123"
            }
        });
        let encoded =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("header.{}.sig", encoded)
    }

    #[test]
    fn test_extract_account_id_from_jwt() {
        let jwt = make_test_jwt();
        let id = extract_account_id_from_jwt(&jwt);
        assert_eq!(id.as_deref(), Some("acc_test123"));
    }

    #[test]
    fn test_extract_account_id_invalid_jwt() {
        assert!(extract_account_id_from_jwt("notajwt").is_none());
        assert!(extract_account_id_from_jwt("a.b").is_none()); // payload not valid JSON after decode
    }

    #[test]
    fn test_token_not_expired() {
        let token = ChatGptToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at_ms: Some(chrono::Utc::now().timestamp_millis() + 3_600_000),
            account_id: None,
        };
        assert!(!token.is_expired());
    }

    #[test]
    fn test_token_expired() {
        let token = ChatGptToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at_ms: Some(chrono::Utc::now().timestamp_millis() - 3_600_000),
            account_id: None,
        };
        assert!(token.is_expired());
    }

    #[test]
    fn test_token_within_refresh_margin() {
        let token = ChatGptToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            // 3 minutes from now — within 5-minute margin
            expires_at_ms: Some(chrono::Utc::now().timestamp_millis() + 3 * 60 * 1_000),
            account_id: None,
        };
        assert!(token.is_expired());
    }

    #[test]
    fn test_token_no_expiry_never_expires() {
        let token = ChatGptToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at_ms: None,
            account_id: None,
        };
        assert!(!token.is_expired());
    }

    #[test]
    fn test_load_from_env() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("CHATGPT_ACCESS_TOKEN", "test-access-token");
        std::env::remove_var("CHATGPT_REFRESH_TOKEN");
        std::env::remove_var("CHATGPT_ACCOUNT_ID");

        let manager = ChatGptOAuthTokenManager::new(ChatGptTokenSource::Env, None);
        let token = manager.load_from_env().unwrap();
        assert_eq!(token.access_token, "test-access-token");
        assert!(token.refresh_token.is_none());

        std::env::remove_var("CHATGPT_ACCESS_TOKEN");
    }

    #[test]
    fn test_load_from_env_missing() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("CHATGPT_ACCESS_TOKEN");
        let manager = ChatGptOAuthTokenManager::new(ChatGptTokenSource::Env, None);
        assert!(manager.load_from_env().is_err());
    }

    #[test]
    fn test_load_from_env_with_account_id_override() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("CHATGPT_ACCESS_TOKEN", "tok");
        std::env::set_var("CHATGPT_ACCOUNT_ID", "acc_override");

        let manager = ChatGptOAuthTokenManager::new(ChatGptTokenSource::Env, None);
        let token = manager.load_from_env().unwrap();
        assert_eq!(token.account_id.as_deref(), Some("acc_override"));

        std::env::remove_var("CHATGPT_ACCESS_TOKEN");
        std::env::remove_var("CHATGPT_ACCOUNT_ID");
    }

    #[test]
    fn test_load_from_credentials_file() {
        let json = serde_json::json!({
            "access_token": "access-abc",
            "refresh_token": "refresh-xyz",
            "expires_at": 1_900_000_000_000i64,
            "account_id": "acc_file123"
        });
        let dir = std::env::temp_dir();
        let path = dir.join("rausu_test_chatgpt_credentials.json");
        std::fs::write(&path, json.to_string()).unwrap();

        let manager =
            ChatGptOAuthTokenManager::new(ChatGptTokenSource::CredentialsFile, Some(path.clone()));
        let token = manager.load_from_credentials_file().unwrap();

        assert_eq!(token.access_token, "access-abc");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-xyz"));
        assert_eq!(token.expires_at_ms, Some(1_900_000_000_000));
        assert_eq!(token.account_id.as_deref(), Some("acc_file123"));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_load_from_credentials_file_missing() {
        let manager = ChatGptOAuthTokenManager::new(
            ChatGptTokenSource::CredentialsFile,
            Some(PathBuf::from("/nonexistent/chatgpt-auth.json")),
        );
        assert!(manager.load_from_credentials_file().is_err());
    }
}
