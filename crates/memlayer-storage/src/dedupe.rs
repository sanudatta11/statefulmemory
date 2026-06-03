//! Dedupe helpers — content normalization, hashing, and the family/review map.
//!
//! Source: PRD §5.8 (dedupe rules) + §5.5 (review schedule).

use sha2::{Digest, Sha256};

/// Lowercase and collapse contiguous whitespace into single spaces.
///
/// Matches the spec text in PRD §5.8: "lowercase, collapse whitespace, hash".
pub fn normalize_content(content: &str) -> String {
    let lower = content.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev_space = false;
    for ch in lower.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// SHA-256 of the normalized content, hex-encoded.
pub fn hash(content: &str) -> String {
    let normalized = normalize_content(content);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hex::encode(hasher.finalize())
}

/// Map an observation `type` to its review interval in months.
///
/// Source: spec FR4.6.
pub fn review_months_for_type(ty: &str) -> Option<i32> {
    match ty {
        "decision" => Some(6),
        "policy" => Some(12),
        "preference" => Some(3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_collapses() {
        assert_eq!(normalize_content("foo   bar\nbaz"), "foo bar baz");
        assert_eq!(normalize_content("  Foo "), "foo");
    }

    #[test]
    fn hashes_are_stable() {
        let a = hash("Hello   World");
        let b = hash("hello world");
        let c = hash("HELLO  WORLD");
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn hashes_are_distinct_for_different_content() {
        assert_ne!(hash("foo"), hash("bar"));
    }

    #[test]
    fn review_months_per_type() {
        assert_eq!(review_months_for_type("decision"), Some(6));
        assert_eq!(review_months_for_type("policy"), Some(12));
        assert_eq!(review_months_for_type("preference"), Some(3));
        assert_eq!(review_months_for_type("note"), None);
    }
}
