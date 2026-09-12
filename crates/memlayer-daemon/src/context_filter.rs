//! Stale withdrawal for auto-injected context.

use memlayer_storage::anchor::VerifyState;

/// Whether an observation with the given verify_state should appear in
/// `context` when `serve_stale` / `include_stale` are set as shown.
///
/// `unanchored` is never withdrawn — that is the default for legacy rows.
pub fn context_allows(
    verify_state: &str,
    serve_stale: bool,
    include_stale: bool,
) -> bool {
    if serve_stale || include_stale {
        return true;
    }
    !matches!(
        VerifyState::parse(verify_state),
        VerifyState::Stale | VerifyState::Invalidated | VerifyState::Unprovable
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unanchored_always_served() {
        assert!(context_allows("unanchored", false, false));
        assert!(context_allows("", false, false));
    }

    #[test]
    fn verified_served() {
        assert!(context_allows("verified", false, false));
    }

    #[test]
    fn invalidated_withdrawn_unless_override() {
        assert!(!context_allows("invalidated", false, false));
        assert!(!context_allows("stale", false, false));
        assert!(!context_allows("unprovable", false, false));
        assert!(context_allows("invalidated", true, false));
        assert!(context_allows("invalidated", false, true));
    }
}
