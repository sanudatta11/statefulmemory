//! TCP-mode bearer-token authentication interceptor.
//!
//! Spec sections: FR2.3, EH-7, SC-16.
//!
//! - Reads `authorization: Bearer <hex>` metadata.
//! - SHA-256 of the secret bytes is constant-time-compared to the stored hash
//!   in `tokens.db`.
//! - On success, an `AuthCtx { token_name, is_admin }` is attached to the
//!   request extensions for handlers that gate admin-only RPCs.
//! - On UDS (no `MEMLAYER_LISTEN`), the interceptor is *not* installed —
//!   filesystem permissions (`0600`) are the auth surface.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use tonic::{Request, Status};
use tracing::warn;

use crate::tokens::TokenStore;

#[derive(Debug, Clone)]
pub struct AuthCtx {
    pub token_name: String,
    pub is_admin: bool,
}

/// tonic interceptor closure.
#[allow(clippy::result_large_err)]
pub fn make_interceptor(
    store: Arc<TokenStore>,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone + Send + Sync + 'static {
    move |mut req: Request<()>| {
        let auth_header = req
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let token = match auth_header {
            Some(s) => match s.strip_prefix("Bearer ") {
                Some(t) => t.trim().to_string(),
                None => return Err(Status::unauthenticated("missing Bearer prefix")),
            },
            None => return Err(Status::unauthenticated("missing authorization header")),
        };
        // Hash and compare in constant time.
        let mut h = Sha256::new();
        h.update(token.as_bytes());
        let candidate = h.finalize();
        let candidate = candidate.as_slice();
        match store.find(candidate) {
            Some(meta) if meta.revoked_at.is_none() => {
                req.extensions_mut().insert(AuthCtx {
                    token_name: meta.name.clone(),
                    is_admin: meta.is_admin,
                });
                Ok(req)
            }
            Some(_) => Err(Status::unauthenticated("token revoked")),
            None => {
                warn!("auth: token not recognized");
                Err(Status::unauthenticated("invalid token"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::{TokenMeta, TokenStore};
    use tempfile::TempDir;

    #[allow(dead_code)]
    fn fake_store() -> (TempDir, Arc<TokenStore>, String) {
        let d = TempDir::new().unwrap();
        let store = TokenStore::open(d.path().join("tokens.db")).unwrap();
        let plaintext = "abc123".to_string();
        let mut h = Sha256::new();
        h.update(plaintext.as_bytes());
        let hash = h.finalize().to_vec();
        store
            .insert(TokenMeta {
                name: "alice".into(),
                hash: hash.clone(),
                is_admin: true,
                created_at: chrono::Utc::now().to_rfc3339(),
                revoked_at: None,
            })
            .unwrap();
        (d, Arc::new(store), plaintext)
    }

    #[test]
    fn constant_time_eq_holds() {
        use subtle::ConstantTimeEq;
        let a = [1u8; 32];
        let b = [1u8; 32];
        assert!(bool::from(a.ct_eq(&b)));
        let c = [2u8; 32];
        assert!(!bool::from(a.ct_eq(&c)));
    }
}
