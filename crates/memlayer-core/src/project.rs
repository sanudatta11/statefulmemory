//! Project-name normalization (PRD §9.2).
//!
//! Rules (verbatim from PRD §9.2):
//! - Lowercase the input.
//! - Replace ` ` and `_` with `-`.
//! - Drop characters outside `[A-Za-z0-9._-]`.
//! - Reject names that start with `.` or contain `..`.
//!
//! After normalization, the resulting string is the **on-disk identifier**: the
//! file `~/.memlayer/projects/<normalized>.db` is what the daemon opens.
//!
//! Two distinct display names that normalize to the same identifier collide.
//! `Storage` enforces collision detection against `config.json` per FR7.2.

use crate::error::{Error, Result};

/// Normalize a display project name into its on-disk identifier.
///
/// Returns `Err(InvalidArgument)` if the input would produce an unsafe path
/// (path-traversal, hidden-file).
pub fn normalize(display_name: &str) -> Result<String> {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid("project name is empty"));
    }
    // Lowercase, then map spaces and underscores to dashes.
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.to_ascii_lowercase().chars() {
        match ch {
            ' ' | '_' => out.push('-'),
            'a'..='z' | '0'..='9' | '.' | '-' => out.push(ch),
            // Anything else is silently dropped per PRD §9.2.
            _ => {}
        }
    }
    // Reject empty result (e.g., display name was "????").
    if out.is_empty() {
        return Err(Error::invalid(format!(
            "project name '{display_name}' contains no valid characters",
        )));
    }
    // Path-traversal safety: forbid leading '.' and any occurrence of "..".
    if out.starts_with('.') {
        return Err(Error::invalid(format!(
            "project name '{display_name}' starts with '.' (forbidden)",
        )));
    }
    if out.contains("..") {
        return Err(Error::invalid(format!(
            "project name '{display_name}' contains '..' (forbidden)",
        )));
    }
    Ok(out)
}

/// Validate that an on-disk project identifier is well-formed (already normalized).
///
/// Used by code paths that read the identifier from the filesystem rather than
/// from caller input — those paths should still defend against tampered config.
pub fn validate(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(Error::invalid("empty project id"));
    }
    if id.starts_with('.') {
        return Err(Error::invalid("project id starts with '.'"));
    }
    if id.contains("..") {
        return Err(Error::invalid("project id contains '..'"));
    }
    for ch in id.chars() {
        match ch {
            'a'..='z' | '0'..='9' | '.' | '-' => {}
            _ => return Err(Error::invalid(format!("invalid char '{ch}' in project id"))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_dashes() {
        assert_eq!(normalize("My Project").unwrap(), "my-project");
        assert_eq!(normalize("foo_bar").unwrap(), "foo-bar");
    }

    #[test]
    fn drops_invalid_chars() {
        assert_eq!(normalize("Acme!Co/Inc").unwrap(), "acmecoinc");
    }

    #[test]
    fn keeps_dots_and_dashes() {
        assert_eq!(normalize("v1.2.3-rc").unwrap(), "v1.2.3-rc");
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(normalize(".hidden").is_err());
    }

    #[test]
    fn rejects_double_dot() {
        assert!(normalize("foo..bar").is_err());
        assert!(normalize("..").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(normalize("").is_err());
        assert!(normalize("   ").is_err());
        assert!(normalize("???").is_err());
    }

    #[test]
    fn validate_round_trip() {
        let id = normalize("My Project").unwrap();
        assert!(validate(&id).is_ok());
    }

    #[test]
    fn validate_rejects_bad_input() {
        assert!(validate(".hidden").is_err());
        assert!(validate("foo..bar").is_err());
        assert!(validate("UPPERCASE").is_err());
        assert!(validate("foo/bar").is_err());
    }
}
