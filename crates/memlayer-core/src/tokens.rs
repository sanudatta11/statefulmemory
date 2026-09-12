//! Cheap token estimates for context budgets.
//!
//! This is **not** tiktoken. The eval harness uses a real tokenizer; daemon /
//! CLI packing uses [`estimate_tokens`] so we stay dependency-free on the
//! hot path. Empirically this is within ~±20% of cl100k for English prose
//! and code comments; treat reported `tokens_used` as a budget guard, not a
//! billing meter.

/// Approximate token count: `ceil(char_count / 4)`.
///
/// Empty string → 0. Whitespace-only strings still count characters.
pub fn estimate_tokens(s: &str) -> usize {
    let n = s.chars().count();
    if n == 0 {
        0
    } else {
        n.div_ceil(4)
    }
}

/// Tokens attributed to one observation when packing a briefing.
pub fn estimate_observation_tokens(r#type: &str, title: &str, content: &str) -> usize {
    estimate_tokens(&format!("{type}\n{title}\n{content}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn four_chars_one_token() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn observation_includes_type_title_content() {
        let t = estimate_observation_tokens("note", "hi", "hello world");
        assert!(t >= estimate_tokens("hello world"));
    }
}
