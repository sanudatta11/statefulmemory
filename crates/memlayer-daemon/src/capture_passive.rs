//! `CapturePassive`: extract `## Key Learnings:` bullet items from markdown.
//!
//! Spec sections: FR12.6, OQ-2.
//!
//! Pure markdown parser — no SQL, no IO. Given a body of text, return every
//! bullet line that appears under any `## Key Learnings:` heading, until the
//! next heading at the same or higher level (`#`, `##`) or the end of input.
//! Per OQ-2, multiple `## Key Learnings:` blocks in one document are merged
//! into a single output list.
//!
//! Recognized bullet markers: `-`, `*`, `+` followed by whitespace. Numbered
//! list markers (`1.`, `2.`) are also recognized. Bullets nested under other
//! bullets (indented) are flattened — every bullet whose marker is at any
//! indentation level becomes one snippet, with leading whitespace stripped.

const HEADING_PREFIXES: &[&str] = &["# ", "## ", "### ", "#### ", "##### ", "###### "];

/// Extract every bullet under any `## Key Learnings:` block.
///
/// Returns the list in document order, deduplicated by exact equality.
pub fn extract_key_learnings(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_block = false;
    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if is_key_learnings_heading(line) {
            in_block = true;
            continue;
        }
        if in_block && is_block_terminator(line) {
            in_block = false;
            continue;
        }
        if !in_block {
            continue;
        }
        if let Some(snippet) = parse_bullet(line) {
            if !snippet.is_empty() && !out.iter().any(|s| s == &snippet) {
                out.push(snippet);
            }
        }
    }
    out
}

/// `## Key Learnings:` (with or without a trailing colon, case-insensitive).
fn is_key_learnings_heading(line: &str) -> bool {
    let stripped = line.trim_start();
    let after_hashes = match stripped.strip_prefix("## ") {
        Some(rest) => rest,
        None => return false,
    };
    let lower = after_hashes.to_ascii_lowercase();
    let lower = lower.trim_end_matches(':').trim();
    lower == "key learnings"
}

fn is_block_terminator(line: &str) -> bool {
    let trimmed = line.trim_start();
    HEADING_PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

/// Parse a bullet line into the trimmed snippet text. Returns `None` if the
/// line is not a bullet (blank, prose, code block, etc.).
fn parse_bullet(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    // Marker bullets: `-`, `*`, `+`.
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest.trim().to_string());
        }
    }
    // Numbered bullets: digits + `. ` or `) `.
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i + 1 < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') && bytes[i + 1] == b' '
    {
        return Some(trimmed[i + 2..].trim().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bullets_from_block() {
        let md = "\
# Some Document

Intro text.

## Key Learnings:
- First learning
- Second learning
- Third learning

Other content.
";
        let got = extract_key_learnings(md);
        assert_eq!(
            got,
            vec![
                "First learning".to_string(),
                "Second learning".to_string(),
                "Third learning".to_string(),
            ]
        );
    }

    #[test]
    fn no_header_returns_empty() {
        let md = "\
# Just a doc

- This bullet is not under Key Learnings
- So it should be ignored.

## Some other heading
- Same here.
";
        let got = extract_key_learnings(md);
        assert!(got.is_empty(), "expected empty, got {got:?}");
    }

    #[test]
    fn merges_multiple_blocks_oq_2() {
        let md = "\
## Key Learnings:
- a
- b

## Notes
- not extracted

## Key Learnings
- c
- d
";
        let got = extract_key_learnings(md);
        assert_eq!(got, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn supports_star_and_plus_markers() {
        let md = "\
## Key Learnings:
* star bullet
+ plus bullet
- dash bullet
";
        let got = extract_key_learnings(md);
        assert_eq!(got, vec!["star bullet", "plus bullet", "dash bullet"]);
    }

    #[test]
    fn supports_numbered_bullets() {
        let md = "\
## Key Learnings:
1. first
2. second
3) third
";
        let got = extract_key_learnings(md);
        assert_eq!(got, vec!["first", "second", "third"]);
    }

    #[test]
    fn nested_bullets_are_flattened() {
        let md = "\
## Key Learnings:
- top level
  - nested under top
    - deeply nested
- another top
";
        let got = extract_key_learnings(md);
        assert_eq!(
            got,
            vec!["top level", "nested under top", "deeply nested", "another top"]
        );
    }

    #[test]
    fn deduplicates_exact_repeats() {
        let md = "\
## Key Learnings:
- alpha
- beta
- alpha
";
        let got = extract_key_learnings(md);
        assert_eq!(got, vec!["alpha", "beta"]);
    }

    #[test]
    fn case_insensitive_heading_match() {
        let md = "\
## KEY LEARNINGS
- captured
";
        assert_eq!(extract_key_learnings(md), vec!["captured"]);
    }

    #[test]
    fn other_h2_terminates_block() {
        let md = "\
## Key Learnings:
- inside

## Conclusion
- outside (not extracted)
";
        assert_eq!(extract_key_learnings(md), vec!["inside"]);
    }
}
