//! TLS configuration for TCP-mode `tonic` server (FR2.2, EH-6).

use std::path::Path;

use rustls::pki_types::PrivateKeyDer;
use rustls_pemfile::{certs, pkcs8_private_keys};
use tonic::transport::ServerTlsConfig;

use memlayer_core::error::{Error, Result};

/// Read a PEM cert + key pair and produce the `ServerTlsConfig` tonic expects.
pub fn server_tls_config(cert_path: &Path, key_path: &Path) -> Result<ServerTlsConfig> {
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
