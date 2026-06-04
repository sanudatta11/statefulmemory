//! TLS configuration for TCP-mode `tonic` server (FR2.2, EH-6).

use std::path::Path;

use rustls::pki_types::PrivateKeyDer;
use rustls_pemfile::{certs, pkcs8_private_keys};
use tonic::transport::ServerTlsConfig;

use memlayer_core::error::{Error, Result};

/// Install rustls's default `CryptoProvider` exactly once per process.
///
/// rustls 0.23 removed the implicit provider auto-pick when more than one
/// provider crate is reachable in the dep graph. Our workspace pulls both
/// `ring` and `aws-lc-rs` transitively, so the first
/// `ServerConfig::builder()` / `ClientConfig::builder()` call panics with
/// "Could not automatically determine the process-level CryptoProvider".
/// Calling this from any TLS entry point — daemon-side
/// [`server_tls_config`] and the client-side connect path — guarantees the
/// provider is in place before tonic touches rustls.
///
/// Idempotent: subsequent invocations are no-ops via [`std::sync::Once`].
pub fn install_default_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // aws-lc-rs is rustls' upstream default and FIPS-friendly. The
        // install_default call returns Err if a provider was already set
        // via a different code path; we don't care which provider won
        // the race as long as one is installed.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Read a PEM cert + key pair and produce the `ServerTlsConfig` tonic expects.
pub fn server_tls_config(cert_path: &Path, key_path: &Path) -> Result<ServerTlsConfig> {
    install_default_crypto_provider();
    let cert_pem = std::fs::read(cert_path).map_err(|e| {
        Error::FailedPrecondition(format!("read TLS cert {}: {e}", cert_path.display()))
    })?;
    let key_pem = std::fs::read(key_path).map_err(|e| {
        Error::FailedPrecondition(format!("read TLS key {}: {e}", key_path.display()))
    })?;
    // Validate the PEM blocks parse before handing them to tonic.
    let _: Vec<_> = certs(&mut cert_pem.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::FailedPrecondition(format!("parse cert PEM: {e}")))?;
    let _: Vec<PrivateKeyDer<'_>> = pkcs8_private_keys(&mut key_pem.as_slice())
        .map(|r| r.map(PrivateKeyDer::Pkcs8))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::FailedPrecondition(format!("parse key PEM: {e}")))?;
    let identity = tonic::transport::Identity::from_pem(&cert_pem, &key_pem);
    Ok(ServerTlsConfig::new().identity(identity))
}
