//! ChatGPT OAuth token manager.
//!
//! Supports loading tokens from environment variables, a credentials file,
//! or automatic OpenAI Device Code Flow login.
//!
//! If `Auto` source is used and no env/file credentials exist, and stdout is
//! a TTY, the manager will automatically initiate an OpenAI Device Flow login
//! so the user can authorize in a browser.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
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

/// Codex CLI credentials file relative to the home directory.
const CODEX_AUTH_RELATIVE: &str = ".codex/auth.json";

/// Token endpoint for refresh grants.
const TOKEN_ENDPOINT: &str = "https://auth0.openai.com/oauth/token";

/// OAuth client ID used by the ChatGPT pi client.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// JWT claim key for the ChatGPT account ID (nested under the auth namespace).
const JWT_AUTH_NS: &str = "https://api.openai.com/auth";

/// OpenAI Device Flow: request a device code.
const DEVICE_CODE_URL: &str = "https://auth0.openai.com/oauth/device/code";

/// Audience for the OpenAI Device Flow token request.
const DEVICE_AUDIENCE: &str = "https://api.openai.com/v1";

// ── Token source ──────────────────────────────────────────────────────────────

/// Determines where the ChatGPT OAuth token is loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatGptTokenSource {
    /// Use the `CHATGPT_ACCESS_TOKEN` environment variable (no refresh).
    Env,
    /// Use `~/.config/rausu/chatgpt-auth.json` (or a custom path).
    CredentialsFile,
    /// Use Codex CLI's `~/.codex/auth.json`.
    Codex,
    /// Explicitly use OpenAI Device Code Flow login.
    DeviceFlow,
    /// Auto-detect: try env first, then credentials file, then Codex auth, then device flow.
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

/// Codex CLI `~/.codex/auth.json` structure.
#[derive(Debug, Deserialize)]
struct CodexCredentialsFile {
    #[allow(dead_code)]
    auth_mode: Option<String>,
    tokens: Option<CodexTokens>,
}

/// Token block within the Codex credentials file.
#[derive(Debug, Deserialize)]
struct CodexTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
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

/// Response from `POST /oauth/device/code`.
#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    #[allow(dead_code)]
    user_code: String,
    verification_uri_complete: String,
    /// Minimum polling interval in seconds.
    #[serde(default)]
    interval: Option<u64>,
    /// Seconds until the device code expires.
    #[allow(dead_code)]
    expires_in: u64,
}

/// Response from `POST /oauth/token` during device flow polling.
#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    /// Present on success.
    access_token: Option<String>,
    /// Present on success.
    #[serde(default)]
    refresh_token: Option<String>,
    /// Lifetime in seconds (present on success).
    #[serde(default)]
    expires_in: Option<i64>,
    /// Present on error — `authorization_pending`, `slow_down`, `expired_token`, etc.
    error: Option<String>,
}

/// Minimal structure written to `chatgpt-auth.json` after device flow login.
#[derive(Debug, Serialize)]
struct CredentialsFileWrite {
    access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
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
            client: Client::builder()
                .user_agent("codex/1.0")
                .build()
                .expect("Failed to build HTTP client"),
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

        let token = self.load_or_login().await?;
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
            ChatGptTokenSource::Codex => self.load_from_codex_auth(),
            ChatGptTokenSource::DeviceFlow => {
                Err(anyhow::anyhow!("Device flow requires async; use load_or_login()"))
            }
            ChatGptTokenSource::Auto => {
                if let Ok(token) = self.load_from_env() {
                    return Ok(token);
                }
                if let Ok(token) = self.load_from_credentials_file() {
                    return Ok(token);
                }
                self.load_from_codex_auth()
            }
        }
    }

    /// Load credentials from env/file, or fall back to device flow login if
    /// stdout is a TTY.
    pub async fn load_or_login(&self) -> Result<ChatGptToken> {
        match &self.token_source {
            ChatGptTokenSource::DeviceFlow => {
                let token = device_flow_login(&self.client).await?;
                self.save_credentials(&token)?;
                Ok(token)
            }
            ChatGptTokenSource::Codex => self.load_from_codex_auth_with_refresh().await,
            ChatGptTokenSource::Auto => {
                // Try sync sources first.
                if let Ok(token) = self.load_from_env() {
                    return Ok(token);
                }
                if let Ok(token) = self.load_from_credentials_file() {
                    return Ok(token);
                }
                // Try Codex auth (with async refresh support).
                if let Ok(token) = self.load_from_codex_auth_with_refresh().await {
                    return Ok(token);
                }
                // Fall back to device flow if TTY is available.
                if !std::io::stdout().is_terminal() {
                    anyhow::bail!(
                        "ChatGPT credentials not found in env, credentials file, or Codex auth"
                    );
                }
                info!("ChatGPT credentials not found, initiating device flow login...");
                let token = device_flow_login(&self.client).await?;
                self.save_credentials(&token)?;
                info!("ChatGPT login successful! Credentials saved.");
                Ok(token)
            }
            _ => match self.load_token() {
                Ok(token) => Ok(token),
                Err(e) => {
                    if !std::io::stdout().is_terminal() {
                        return Err(e);
                    }
                    info!("ChatGPT credentials not found, initiating device flow login...");
                    let token = device_flow_login(&self.client).await?;
                    self.save_credentials(&token)?;
                    info!("ChatGPT login successful! Credentials saved.");
                    Ok(token)
                }
            },
        }
    }

    /// Save credentials to `~/.config/rausu/chatgpt-auth.json`.
    fn save_credentials(&self, token: &ChatGptToken) -> Result<()> {
        let path = self
            .credentials_path()
            .context("Could not determine chatgpt credentials file path")?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let creds = CredentialsFileWrite {
            access_token: token.access_token.clone(),
            refresh_token: token.refresh_token.clone(),
            expires_at: token.expires_at_ms,
            account_id: token.account_id.clone(),
        };

        let json =
            serde_json::to_string_pretty(&creds).context("Failed to serialize credentials")?;
        std::fs::write(&path, json)
            .with_context(|| format!("Failed to write credentials file: {}", path.display()))?;

        debug!(path = %path.display(), "Saved ChatGPT credentials");
        Ok(())
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

    /// Load from Codex CLI's `~/.codex/auth.json`.
    pub fn load_from_codex_auth(&self) -> Result<ChatGptToken> {
        let path = dirs::home_dir()
            .map(|home| home.join(CODEX_AUTH_RELATIVE))
            .context("Could not determine home directory for Codex auth")?;

        let contents = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "Failed to read Codex credentials file: {}",
                path.display()
            )
        })?;

        let codex: CodexCredentialsFile =
            serde_json::from_str(&contents).context("Failed to parse Codex credentials file")?;

        let tokens = codex
            .tokens
            .context("Codex auth.json has no 'tokens' field")?;

        let access_token = tokens
            .access_token
            .filter(|s| !s.is_empty())
            .context("Codex auth.json has no access_token")?;

        let account_id = tokens
            .account_id
            .filter(|s| !s.is_empty())
            .or_else(|| extract_account_id_from_jwt(&access_token));

        info!(
            "Loaded ChatGPT credentials from Codex CLI (~/.codex/auth.json)"
        );

        Ok(ChatGptToken {
            access_token,
            refresh_token: tokens.refresh_token.filter(|s| !s.is_empty()),
            expires_at_ms: None, // Codex doesn't store expiry; rely on refresh-on-failure
            account_id,
        })
    }

    /// Load from Codex auth with async refresh support.
    ///
    /// If the Codex file has a refresh token but no (or empty) access token,
    /// performs a token refresh to obtain a fresh access token.
    async fn load_from_codex_auth_with_refresh(&self) -> Result<ChatGptToken> {
        let path = dirs::home_dir()
            .map(|home| home.join(CODEX_AUTH_RELATIVE))
            .context("Could not determine home directory for Codex auth")?;

        let contents = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "Failed to read Codex credentials file: {}",
                path.display()
            )
        })?;

        let codex: CodexCredentialsFile =
            serde_json::from_str(&contents).context("Failed to parse Codex credentials file")?;

        let tokens = codex
            .tokens
            .context("Codex auth.json has no 'tokens' field")?;

        let access_token = tokens.access_token.filter(|s| !s.is_empty());
        let refresh_token = tokens.refresh_token.filter(|s| !s.is_empty());

        // If we have an access token, use it directly.
        if let Some(access_token) = access_token {
            let account_id = tokens
                .account_id
                .filter(|s| !s.is_empty())
                .or_else(|| extract_account_id_from_jwt(&access_token));

            info!(
                "Loaded ChatGPT credentials from Codex CLI (~/.codex/auth.json)"
            );

            return Ok(ChatGptToken {
                access_token,
                refresh_token,
                expires_at_ms: None,
                account_id,
            });
        }

        // No access token — try refreshing if we have a refresh token.
        let refresh_token =
            refresh_token.context("Codex auth.json has neither access_token nor refresh_token")?;

        info!("Codex auth.json has no access_token; attempting refresh");
        let token = self.do_refresh(&refresh_token).await?;
        info!(
            "Loaded ChatGPT credentials from Codex CLI (~/.codex/auth.json) via token refresh"
        );
        Ok(token)
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

// ── Device Flow ──────────────────────────────────────────────────────────────

/// Perform OpenAI Device Code Flow login and return a `ChatGptToken`.
///
/// Prints a verification URL and user code to the terminal, then polls
/// until the user completes authorization (or the code expires).
pub async fn device_flow_login(client: &Client) -> Result<ChatGptToken> {
    // Step 1: Request a device code.
    let resp = client
        .post(DEVICE_CODE_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .form(&[
            ("client_id", CLIENT_ID),
            ("audience", DEVICE_AUDIENCE),
            ("scope", "openai.public"),
        ])
        .send()
        .await
        .context("Failed to request OpenAI device code")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI device code request failed: {}", body);
    }

    let dc: DeviceCodeResponse = resp
        .json()
        .await
        .context("Failed to parse device code response")?;

    // Step 2: Print instructions.
    println!();
    println!("\u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}");
    println!("\u{2551}  ChatGPT Login Required                  \u{2551}");
    println!("\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");
    println!("\u{2551}  Open: {}", dc.verification_uri_complete);
    println!("\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}");
    println!();

    // Step 3: Poll for the access token.
    let mut interval_secs = dc.interval.unwrap_or(5);
    info!("Waiting for authorization...");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;

        let resp = client
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", dc.device_code.as_str()),
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:device_code",
                ),
            ])
            .send()
            .await
            .context("Failed to poll OpenAI token endpoint")?;

        let dt: DeviceTokenResponse = resp
            .json()
            .await
            .context("Failed to parse token poll response")?;

        if let Some(access_token) = dt.access_token {
            let expires_at_ms = dt
                .expires_in
                .map(|secs| chrono::Utc::now().timestamp_millis() + secs * 1_000);
            let account_id = extract_account_id_from_jwt(&access_token);

            return Ok(ChatGptToken {
                access_token,
                refresh_token: dt.refresh_token,
                expires_at_ms,
                account_id,
            });
        }

        match dt.error.as_deref() {
            Some("authorization_pending") => {
                // Expected — keep polling.
            }
            Some("slow_down") => {
                interval_secs += 5;
                debug!(interval_secs, "OpenAI asked us to slow down");
            }
            Some("expired_token") => {
                anyhow::bail!(
                    "OpenAI device code expired. Please restart Rausu to try again."
                );
            }
            Some("access_denied") => {
                anyhow::bail!("Authorization was denied by the user.");
            }
            Some(other) => {
                anyhow::bail!("OpenAI device flow error: {}", other);
            }
            None => {
                anyhow::bail!(
                    "Unexpected response from OpenAI token endpoint (no token, no error)"
                );
            }
        }
    }
}

/// Ensure that ChatGPT credentials are available, running device flow login
/// if necessary. Call this at server startup before binding the listener.
pub async fn ensure_chatgpt_credentials(
    token_manager: &ChatGptOAuthTokenManager,
) -> Result<()> {
    let _ = token_manager.load_or_login().await?;
    Ok(())
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

    #[test]
    fn test_load_from_codex_auth() {
        let jwt = make_test_jwt();
        let json = serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "eyJ_id",
                "access_token": jwt,
                "refresh_token": "v1.refresh_abc",
                "account_id": "codex-account-123"
            },
            "last_refresh": "2026-04-03T03:52:32.196331Z"
        });

        // Write a temporary file and point HOME at its parent so
        // dirs::home_dir() resolves to our temp dir.
        let dir = std::env::temp_dir().join("rausu_test_codex_auth");
        let codex_dir = dir.join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let path = codex_dir.join("auth.json");
        std::fs::write(&path, json.to_string()).unwrap();

        // Directly parse the file instead of relying on HOME override.
        let contents = std::fs::read_to_string(&path).unwrap();
        let codex: CodexCredentialsFile = serde_json::from_str(&contents).unwrap();
        let tokens = codex.tokens.unwrap();

        assert_eq!(tokens.access_token.as_deref(), Some(jwt.as_str()));
        assert_eq!(tokens.refresh_token.as_deref(), Some("v1.refresh_abc"));
        assert_eq!(tokens.account_id.as_deref(), Some("codex-account-123"));

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_codex_auth_missing_tokens() {
        let json = serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": "sk-test"
        });
        let codex: CodexCredentialsFile = serde_json::from_str(&json.to_string()).unwrap();
        assert!(codex.tokens.is_none());
    }

    #[test]
    fn test_codex_auth_empty_access_token() {
        let json = serde_json::json!({
            "tokens": {
                "access_token": "",
                "refresh_token": "v1.refresh"
            }
        });
        let codex: CodexCredentialsFile = serde_json::from_str(&json.to_string()).unwrap();
        let tokens = codex.tokens.unwrap();
        // Empty access_token should be filtered out
        assert!(tokens.access_token.unwrap().is_empty());
    }
}
