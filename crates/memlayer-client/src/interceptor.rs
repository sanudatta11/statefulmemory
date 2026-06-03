//! tonic interceptor that adds a bearer-token `authorization` header.
//!
//! Spec sections: FR10, §11.3, NFR6, EH-8.
//!
//! Used in TCP mode only. The daemon's [`auth::make_interceptor`](../../memlayer-daemon/src/auth.rs)
//! reads the same `authorization: Bearer <hex>` header, hashes the token with
//! SHA-256, and constant-time-compares against `tokens.db`.
//!
//! The token is parsed once into a [`MetadataValue`] at interceptor
//! construction; per-call overhead is a single ascii clone. Tokens in this
//! system are 64-character hex (32-byte `OsRng`), so the parse is infallible
//! for valid tokens — an invalid token is a programmer bug and panics.

use tonic::metadata::MetadataValue;
use tonic::{Request, Status};

/// Build an interceptor closure that injects `authorization: Bearer <token>`
/// into every outbound request's metadata.
///
/// # Panics
///
/// Panics if `token` contains characters that cannot be encoded in an HTTP
/// header value (i.e. anything outside printable ASCII). Tokens minted by
/// `memlayer team token-create` are 64-char hex and never trigger this path.
pub fn bearer_interceptor(
    token: String,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone {
    let header: MetadataValue<_> = format!("Bearer {token}")
        .parse()
        .expect("bearer token must be a valid HTTP header value");
    // The `Result<Request<()>, Status>` shape is dictated by tonic's
    // `Interceptor` trait — boxing the Status here would mismatch the trait
    // and break `MemlayerClient::with_interceptor`.
    #[allow(clippy::result_large_err)]
    move |mut req: Request<()>| {
        req.metadata_mut().insert("authorization", header.clone());
        Ok(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_added_to_metadata() {
        let interceptor = bearer_interceptor("abc123".to_string());
        let req = Request::new(());
        let modified = interceptor(req).expect("interceptor returns Ok for valid token");
        let value = modified
            .metadata()
            .get("authorization")
            .expect("authorization header present");
        assert_eq!(value.to_str().unwrap(), "Bearer abc123");
    }

    #[test]
    fn bearer_overwrites_existing_authorization() {
        let interceptor = bearer_interceptor("new-token".to_string());
        let mut req = Request::new(());
        req.metadata_mut().insert("authorization", "Bearer stale".parse().unwrap());
        let modified = interceptor(req).expect("ok");
        assert_eq!(
            modified.metadata().get("authorization").unwrap().to_str().unwrap(),
            "Bearer new-token"
        );
    }

    #[test]
    fn interceptor_is_clone_and_reusable() {
        let interceptor = bearer_interceptor("xyz".to_string());
        let cloned = interceptor.clone();
        let r1 = interceptor(Request::new(())).unwrap();
        let r2 = cloned(Request::new(())).unwrap();
        assert_eq!(r1.metadata().get("authorization").unwrap(), "Bearer xyz");
        assert_eq!(r2.metadata().get("authorization").unwrap(), "Bearer xyz");
    }
}
