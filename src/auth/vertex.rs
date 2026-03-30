//! Google Cloud Platform (GCP) access token manager for Vertex AI.
//!
//! Supports two credential types read from a JSON file:
//! - `service_account` — sign a JWT with the RSA private key, exchange for an
//!   access token at `https://oauth2.googleapis.com/token`.
//! - `authorized_user` — use the refresh token to obtain an access token from
//!   `https://oauth2.googleapis.com/token` (Application Default Credentials).
//!
//! # Credential resolution order
//! 1. `credentials_path` field passed to [`VertexTokenManager::new`]
//! 2. `GOOGLE_APPLICATION_CREDENTIALS` environment variable
//! 3. Default ADC path: `~/.config/gcloud/application_default_credentials.json`

// Deserialization structs mirror the GCP JSON credential file format exactly.
// Fields are read by serde even when not directly accessed in Rust code.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Re-acquisition margin — get a new token this many seconds before expiry.
const REFRESH_MARGIN_SECS: i64 = 5 * 60;

/// Default token TTL when the server does not return `expires_in`.
const DEFAULT_TOKEN_TTL_SECS: i64 = 3600;

/// Google token endpoint.
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// OAuth2 scope required for Vertex AI REST calls.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// Default ADC file path relative to `$HOME`.
const DEFAULT_ADC_RELATIVE: &str = ".config/gcloud/application_default_credentials.json";

// ── Credential file types ─────────────────────────────────────────────────────

/// Top-level GCP credentials JSON.
///
/// Dispatches on the `type` field.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GcpCredentials {
    /// Service account key file (downloaded from IAM → Service Accounts).
    ServiceAccount(ServiceAccountCreds),
    /// Application Default Credentials written by `gcloud auth application-default login`.
    AuthorizedUser(AuthorizedUserCreds),
}

#[derive(Debug, Deserialize)]
struct ServiceAccountCreds {
    /// Service account email used as JWT `iss` and `sub`.
    client_email: String,
    /// RSA private key in PEM format.
    private_key: String,
    /// Optional key ID — included as JWT `kid` header if present.
    private_key_id: Option<String>,
    /// Token URI (defaults to `GOOGLE_TOKEN_URL`).
    token_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorizedUserCreds {
    client_id: String,
    client_secret: String,
    refresh_token: String,
}

// ── Token exchange response ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    /// Seconds until the token expires (typically 3600).
    expires_in: Option<i64>,
}

// ── Cached state ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    /// Unix timestamp at which this token should be refreshed.
    expires_at_secs: i64,
}

impl CachedToken {
    fn is_expired(&self) -> bool {
        Utc::now().timestamp() + REFRESH_MARGIN_SECS >= self.expires_at_secs
    }
}

// ── Manager ───────────────────────────────────────────────────────────────────

/// Thread-safe GCP access token manager.
///
/// Loads credentials from a JSON file and exchanges them for a short-lived
/// OAuth2 access token.  The token is cached and refreshed automatically when
/// it approaches expiry.
pub struct VertexTokenManager {
    client: Client,
    /// Explicit override for the credentials file path.
    credentials_path: Option<PathBuf>,
    state: RwLock<Option<CachedToken>>,
}

impl VertexTokenManager {
    /// Create a new manager wrapped in an `Arc`.
    ///
    /// `credentials_path` overrides both `GOOGLE_APPLICATION_CREDENTIALS` and
    /// the default ADC path.
    pub fn new(credentials_path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build VertexAI HTTP client"),
            credentials_path,
            state: RwLock::new(None),
        })
    }

    /// Return a valid GCP Bearer access token, loading or refreshing as needed.
    pub async fn get_token(&self) -> Result<String> {
        // Fast path: valid cached token.
        {
            let state = self.state.read().await;
            if let Some(cached) = &*state {
                if !cached.is_expired() {
                    return Ok(cached.access_token.clone());
                }
            }
        }

        // Slow path: acquire write lock and refresh.
        let mut state = self.state.write().await;

        // Double-check in case another task refreshed while we waited.
        if let Some(cached) = &*state {
            if !cached.is_expired() {
                return Ok(cached.access_token.clone());
            }
        }

        debug!("GCP access token absent or expired; refreshing");
        let creds = self.load_credentials()?;
        let cached = self.exchange_token(creds).await?;
        info!("GCP access token acquired successfully");

        let token = cached.access_token.clone();
        *state = Some(cached);
        Ok(token)
    }

    // ── Credential loading ────────────────────────────────────────────────────

    fn resolve_credentials_path(&self) -> Option<PathBuf> {
        if let Some(p) = &self.credentials_path {
            return Some(p.clone());
        }
        if let Ok(env_path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
            if !env_path.is_empty() {
                return Some(PathBuf::from(env_path));
            }
        }
        dirs::home_dir().map(|home| home.join(DEFAULT_ADC_RELATIVE))
    }

    fn load_credentials(&self) -> Result<GcpCredentials> {
        let path = self
            .resolve_credentials_path()
            .context("Could not determine GCP credentials path")?;

        let contents = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "Failed to read GCP credentials file: {}. \
                 Set GOOGLE_APPLICATION_CREDENTIALS to your service-account JSON path, \
                 or run `gcloud auth application-default login`.",
                path.display()
            )
        })?;

        serde_json::from_str::<GcpCredentials>(&contents).context(
            "Failed to parse GCP credentials JSON (expected service_account or authorized_user)",
        )
    }

    // ── Token exchange ────────────────────────────────────────────────────────

    async fn exchange_token(&self, creds: GcpCredentials) -> Result<CachedToken> {
        match creds {
            GcpCredentials::ServiceAccount(sa) => self.exchange_service_account(sa).await,
            GcpCredentials::AuthorizedUser(au) => self.exchange_authorized_user(au).await,
        }
    }

    /// Sign a self-issued JWT and exchange it for a GCP access token.
    async fn exchange_service_account(&self, sa: ServiceAccountCreds) -> Result<CachedToken> {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

        let now = Utc::now().timestamp();

        // JWT claims required by Google's token endpoint.
        #[derive(Serialize)]
        struct Claims {
            iss: String,
            sub: String,
            aud: String,
            scope: String,
            iat: i64,
            exp: i64,
        }

        let token_uri = sa
            .token_uri
            .as_deref()
            .unwrap_or(GOOGLE_TOKEN_URL)
            .to_string();

        let claims = Claims {
            iss: sa.client_email.clone(),
            sub: sa.client_email.clone(),
            aud: token_uri.clone(),
            scope: CLOUD_PLATFORM_SCOPE.to_string(),
            iat: now,
            exp: now + DEFAULT_TOKEN_TTL_SECS,
        };

        let mut header = Header::new(Algorithm::RS256);
        if let Some(kid) = &sa.private_key_id {
            header.kid = Some(kid.clone());
        }

        let encoding_key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
            .context("Failed to parse service account private key (expected RSA PEM)")?;

        let jwt = encode(&header, &claims, &encoding_key)
            .context("Failed to sign service account JWT")?;

        let resp = self
            .client
            .post(&token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .context("Failed to reach Google token endpoint (service account)")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, "GCP service account token exchange failed");
            anyhow::bail!(
                "GCP service account token exchange returned HTTP {}: {}",
                status,
                body
            );
        }

        let token_resp: TokenResponse = resp
            .json()
            .await
            .context("Failed to parse GCP token response")?;

        let expires_at = now + token_resp.expires_in.unwrap_or(DEFAULT_TOKEN_TTL_SECS);
        Ok(CachedToken {
            access_token: token_resp.access_token,
            expires_at_secs: expires_at,
        })
    }

    /// Use a refresh token (ADC `authorized_user`) to obtain an access token.
    async fn exchange_authorized_user(&self, au: AuthorizedUserCreds) -> Result<CachedToken> {
        let now = Utc::now().timestamp();

        let resp = self
            .client
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &au.client_id),
                ("client_secret", &au.client_secret),
                ("refresh_token", &au.refresh_token),
            ])
            .send()
            .await
            .context("Failed to reach Google token endpoint (ADC refresh)")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, "GCP ADC token refresh failed");
            anyhow::bail!("GCP ADC refresh returned HTTP {}: {}", status, body);
        }

        let token_resp: TokenResponse = resp
            .json()
            .await
            .context("Failed to parse GCP ADC token response")?;

        let expires_at = now + token_resp.expires_in.unwrap_or(DEFAULT_TOKEN_TTL_SECS);
        Ok(CachedToken {
            access_token: token_resp.access_token,
            expires_at_secs: expires_at,
        })
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_token(offset_secs: i64) -> CachedToken {
        CachedToken {
            access_token: "tok".to_string(),
            expires_at_secs: Utc::now().timestamp() + offset_secs,
        }
    }

    #[test]
    fn test_token_valid() {
        assert!(!make_token(3600).is_expired());
    }

    #[test]
    fn test_token_expired() {
        assert!(make_token(-3600).is_expired());
    }

    #[test]
    fn test_token_within_refresh_margin() {
        // 3 minutes from now — inside the 5-minute refresh margin.
        assert!(make_token(3 * 60).is_expired());
    }

    #[test]
    fn test_token_outside_refresh_margin() {
        // 6 minutes from now — just outside the 5-minute margin.
        assert!(!make_token(6 * 60).is_expired());
    }

    #[test]
    fn test_resolve_credentials_path_explicit() {
        let mgr = VertexTokenManager::new(Some(PathBuf::from("/explicit/path.json")));
        assert_eq!(
            mgr.resolve_credentials_path(),
            Some(PathBuf::from("/explicit/path.json"))
        );
    }

    #[test]
    fn test_resolve_credentials_path_env() {
        // Clear any explicit path, set the env var.
        let mgr = VertexTokenManager::new(None);
        std::env::set_var(
            "GOOGLE_APPLICATION_CREDENTIALS",
            "/env/service-account.json",
        );
        let path = mgr.resolve_credentials_path();
        std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        assert_eq!(path, Some(PathBuf::from("/env/service-account.json")));
    }

    #[test]
    fn test_resolve_credentials_path_default_adc() {
        // No explicit path, no env var — should fall back to ~/.config/gcloud/...
        let mgr = VertexTokenManager::new(None);
        std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        let path = mgr.resolve_credentials_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("gcloud"));
        assert!(path
            .to_string_lossy()
            .ends_with("application_default_credentials.json"));
    }

    #[test]
    fn test_load_credentials_service_account() {
        let json = serde_json::json!({
            "type": "service_account",
            "project_id": "my-project",
            "private_key_id": "key123",
            "private_key": "-----BEGIN RSA PRIVATE KEY-----\nfake\n-----END RSA PRIVATE KEY-----\n",
            "client_email": "sa@my-project.iam.gserviceaccount.com",
            "client_id": "123456789",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": "https://oauth2.googleapis.com/token"
        });
        let dir = std::env::temp_dir();
        let path = dir.join("rausu_test_vertex_sa.json");
        std::fs::write(&path, json.to_string()).unwrap();

        let mgr = VertexTokenManager::new(Some(path.clone()));
        let creds = mgr.load_credentials();
        std::fs::remove_file(path).ok();

        assert!(creds.is_ok(), "should parse service_account JSON");
        match creds.unwrap() {
            GcpCredentials::ServiceAccount(sa) => {
                assert_eq!(sa.client_email, "sa@my-project.iam.gserviceaccount.com");
                assert_eq!(sa.private_key_id, Some("key123".to_string()));
            }
            _ => panic!("expected ServiceAccount variant"),
        }
    }

    #[test]
    fn test_load_credentials_authorized_user() {
        let json = serde_json::json!({
            "type": "authorized_user",
            "client_id": "clientid.apps.googleusercontent.com",
            "client_secret": "secret123",
            "refresh_token": "refresh_tok"
        });
        let dir = std::env::temp_dir();
        let path = dir.join("rausu_test_vertex_adc.json");
        std::fs::write(&path, json.to_string()).unwrap();

        let mgr = VertexTokenManager::new(Some(path.clone()));
        let creds = mgr.load_credentials();
        std::fs::remove_file(path).ok();

        assert!(creds.is_ok(), "should parse authorized_user JSON");
        match creds.unwrap() {
            GcpCredentials::AuthorizedUser(au) => {
                assert_eq!(au.client_id, "clientid.apps.googleusercontent.com");
                assert_eq!(au.refresh_token, "refresh_tok");
            }
            _ => panic!("expected AuthorizedUser variant"),
        }
    }

    #[test]
    fn test_load_credentials_missing_file() {
        let mgr = VertexTokenManager::new(Some(PathBuf::from("/nonexistent/creds.json")));
        assert!(mgr.load_credentials().is_err());
    }

    #[test]
    fn test_load_credentials_unknown_type() {
        let json = serde_json::json!({ "type": "unsupported_type" });
        let dir = std::env::temp_dir();
        let path = dir.join("rausu_test_vertex_unknown.json");
        std::fs::write(&path, json.to_string()).unwrap();

        let mgr = VertexTokenManager::new(Some(path.clone()));
        let err = mgr.load_credentials().unwrap_err();
        std::fs::remove_file(path).ok();
        assert!(
            err.to_string().contains("parse GCP credentials"),
            "unexpected error: {}",
            err
        );
    }
}
