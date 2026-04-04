//! TLS / mTLS configuration builder.

use std::fs;
use std::io::BufReader;
use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::RootCertStore;

use crate::config::schema::TlsConfig;

/// Build a `rustls::ServerConfig` from the application TLS settings.
///
/// - Loads the server certificate chain and private key from PEM files.
/// - If `client_ca_file` is set, enables mTLS via `WebPkiClientVerifier`.
/// - Otherwise, one-way TLS with no client authentication.
pub fn build_rustls_server_config(tls: &TlsConfig) -> Result<rustls::ServerConfig> {
    // Ensure the ring crypto provider is installed (idempotent).
    let _ = rustls::crypto::ring::default_provider().install_default();

    let certs = load_certs(&tls.cert_file)
        .with_context(|| format!("Failed to load server certificate from {}", tls.cert_file))?;
    let key = load_private_key(&tls.key_file)
        .with_context(|| format!("Failed to load private key from {}", tls.key_file))?;

    let builder = if let Some(ca_path) = &tls.client_ca_file {
        // mTLS: require and verify client certificates against provided CA.
        let mut root_store = RootCertStore::empty();
        let ca_certs = load_certs(ca_path)
            .with_context(|| format!("Failed to load client CA from {ca_path}"))?;
        for cert in ca_certs {
            root_store
                .add(cert)
                .context("Failed to add client CA certificate to root store")?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(root_store))
            .build()
            .context("Failed to build client certificate verifier")?;
        rustls::ServerConfig::builder().with_client_cert_verifier(verifier)
    } else {
        // One-way TLS — no client certificate required.
        rustls::ServerConfig::builder().with_no_client_auth()
    };

    builder
        .with_single_cert(certs, key)
        .context("Failed to set server certificate/key pair")
}

/// Parse PEM-encoded certificates from a file.
pub fn load_certs(path: &str) -> Result<Vec<CertificateDer<'static>>> {
    let file = fs::File::open(path).with_context(|| format!("Cannot open {path}"))?;
    let mut reader = BufReader::new(file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("Failed to parse PEM certificates from {path}"))?;
    if certs.is_empty() {
        anyhow::bail!("No certificates found in {path}");
    }
    Ok(certs)
}

/// Parse a PEM-encoded private key from a file (PKCS#8 or RSA or EC).
pub fn load_private_key(path: &str) -> Result<PrivateKeyDer<'static>> {
    let file = fs::File::open(path).with_context(|| format!("Cannot open {path}"))?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("Failed to parse private key from {path}"))?
        .ok_or_else(|| anyhow::anyhow!("No private key found in {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Helper: generate a self-signed CA + server cert using rcgen.
    fn generate_test_certs() -> (rcgen::CertifiedKey, rcgen::CertifiedKey) {
        use rcgen::{CertificateParams, KeyPair};

        // CA
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(vec!["Test CA".to_string()]).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();

        // Server cert signed by CA
        let server_key = KeyPair::generate().unwrap();
        let server_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let server_cert = server_params.signed_by(&server_key, &ca, &ca_key).unwrap();

        (
            rcgen::CertifiedKey {
                cert: ca,
                key_pair: ca_key,
            },
            rcgen::CertifiedKey {
                cert: server_cert,
                key_pair: server_key,
            },
        )
    }

    fn write_temp_pem(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_build_tls_only() {
        let (_, server) = generate_test_certs();
        let cert_file = write_temp_pem(&server.cert.pem());
        let key_file = write_temp_pem(&server.key_pair.serialize_pem());

        let tls_config = TlsConfig {
            cert_file: cert_file.path().to_str().unwrap().to_string(),
            key_file: key_file.path().to_str().unwrap().to_string(),
            client_ca_file: None,
        };

        let result = build_rustls_server_config(&tls_config);
        assert!(result.is_ok(), "TLS-only config should succeed: {result:?}");
    }

    #[test]
    fn test_build_mtls() {
        let (ca, server) = generate_test_certs();
        let cert_file = write_temp_pem(&server.cert.pem());
        let key_file = write_temp_pem(&server.key_pair.serialize_pem());
        let ca_file = write_temp_pem(&ca.cert.pem());

        let tls_config = TlsConfig {
            cert_file: cert_file.path().to_str().unwrap().to_string(),
            key_file: key_file.path().to_str().unwrap().to_string(),
            client_ca_file: Some(ca_file.path().to_str().unwrap().to_string()),
        };

        let result = build_rustls_server_config(&tls_config);
        assert!(result.is_ok(), "mTLS config should succeed: {result:?}");
    }

    #[test]
    fn test_missing_cert_file() {
        let tls_config = TlsConfig {
            cert_file: "/nonexistent/cert.pem".to_string(),
            key_file: "/nonexistent/key.pem".to_string(),
            client_ca_file: None,
        };
        let result = build_rustls_server_config(&tls_config);
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("cert"), "Error should mention cert: {err}");
    }

    #[test]
    fn test_empty_cert_file() {
        let cert_file = write_temp_pem("");
        let key_file = write_temp_pem("");

        let tls_config = TlsConfig {
            cert_file: cert_file.path().to_str().unwrap().to_string(),
            key_file: key_file.path().to_str().unwrap().to_string(),
            client_ca_file: None,
        };
        let result = build_rustls_server_config(&tls_config);
        assert!(result.is_err(), "Empty cert should fail");
    }

    #[test]
    fn test_invalid_pem_content() {
        let cert_file = write_temp_pem("this is not a PEM file");
        let key_file = write_temp_pem("this is not a key");

        let tls_config = TlsConfig {
            cert_file: cert_file.path().to_str().unwrap().to_string(),
            key_file: key_file.path().to_str().unwrap().to_string(),
            client_ca_file: None,
        };
        let result = build_rustls_server_config(&tls_config);
        assert!(result.is_err(), "Invalid PEM should fail");
    }

    #[test]
    fn test_missing_client_ca_file() {
        let (_, server) = generate_test_certs();
        let cert_file = write_temp_pem(&server.cert.pem());
        let key_file = write_temp_pem(&server.key_pair.serialize_pem());

        let tls_config = TlsConfig {
            cert_file: cert_file.path().to_str().unwrap().to_string(),
            key_file: key_file.path().to_str().unwrap().to_string(),
            client_ca_file: Some("/nonexistent/ca.pem".to_string()),
        };
        let result = build_rustls_server_config(&tls_config);
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("CA"), "Error should mention CA: {err}");
    }
}
