//! Admin gate for TCP-mode privileged RPCs.
//!
//! Spec sections: FR2.3, FR10, EH-7.
//!
//! Privileged RPCs (`Shutdown`, `CreateToken`, `RevokeToken`, `ListTokens`,
//! `DeleteProject --hard`, `DaemonStop`, `DaemonRestart`) must check
//! `AuthCtx::is_admin` from the request extensions before proceeding.
//!
//! UDS mode never installs the auth interceptor (file mode `0600` is the
//! trust boundary), so requests carry no `AuthCtx`. In that case admin
//! checks pass through — root-equivalent access is implicit.
//!
//! TCP mode always installs the interceptor, so a request reaching a handler
//! has been authenticated. The handler then calls [`require_admin`] to
//! enforce the additional `is_admin=true` requirement; non-admin tokens get
//! `PERMISSION_DENIED`.

use tonic::{Request, Status};

use crate::auth::AuthCtx;

/// Return `Ok` if the request is allowed to invoke an admin RPC.
///
/// Two paths:
/// - No `AuthCtx` in extensions: UDS mode. Always allowed.
/// - `AuthCtx { is_admin: true, .. }`: explicit admin token. Allowed.
/// - `AuthCtx { is_admin: false, .. }`: TCP mode + non-admin. Denied.
pub fn require_admin<T>(req: &Request<T>) -> Result<(), Status> {
    match req.extensions().get::<AuthCtx>() {
        None => Ok(()),
        Some(ctx) if ctx.is_admin => Ok(()),
        Some(_) => Err(Status::permission_denied(
            "this RPC requires an admin bearer token",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req_without_auth() -> Request<()> {
        Request::new(())
    }

    fn req_with_auth(is_admin: bool) -> Request<()> {
        let mut r = Request::new(());
        r.extensions_mut().insert(AuthCtx {
            token_name: "test".into(),
            is_admin,
        });
        r
    }

    #[test]
    fn admin_passes_through() {
        // Explicit admin token in TCP mode.
        let r = req_with_auth(true);
        require_admin(&r).expect("admin should pass");
    }

    #[test]
    fn non_admin_tcp_denied() {
        // Authenticated TCP request, but the token is non-admin.
        let r = req_with_auth(false);
        let err = require_admin(&r).expect_err("non-admin should be denied");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn uds_request_passes_with_no_auth_ctx() {
        // No AuthCtx in extensions == UDS mode. Filesystem permissions are
        // the trust boundary; require_admin must pass through.
        let r = req_without_auth();
        require_admin(&r).expect("UDS (no auth ctx) should pass");
    }
}
