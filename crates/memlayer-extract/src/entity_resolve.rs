//! Deterministic, LLM-free entity resolution for the briefing graph
//! (spec: graph-briefing, P1c).
//!
//! Pure functions only: no DB, no network, no model calls. Input is the raw
//! observation text plus its anchors; output is a deduplicated list of
//! [`RawEntity`] mentions ordered by source priority
//! (Anchor > Backtick > Token > Topic).

use memlayer_core::config::{normalize_entity_name, EntityKind, MentionSource};

/// Sentinel offsets for mentions that are not present in the combined text
/// (anchors live outside `title + "\n" + content`).
pub const NOT_IN_TEXT: (usize, usize) = (usize::MAX, usize::MAX);

/// One mined entity mention, before graph persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntity {
    pub kind: EntityKind,
    /// Display form as found / canonicalized.
    pub name: String,
    /// Char span in the combined text (`title + "\n" + content`); callers must
    /// slice by chars, not bytes. [`NOT_IN_TEXT`] when mined from anchors.
    pub offsets: (usize, usize),
    pub source: MentionSource,
}

/// Extensions that mark a backtick or path-like token as a [`EntityKind::File`].
const FILE_EXTENSIONS: [&str; 9] = [
    ".rs", ".ts", ".py", ".go", ".js", ".md", ".toml", ".yml", ".yaml",
];

/// Backtick tokens longer than this are noise (e.g. fenced code blocks).
const MAX_BACKTICK_LEN: usize = 64;
const MAX_BACKTICK_ENTITIES: usize = 12;
const MAX_TOKEN_ENTITIES: usize = 12;
const MAX_QUERY_ENTITIES: usize = 3;

/// Mine entities from title + content + anchors (deterministic tiers).
///
/// Tier order (and dedupe priority):
/// 1. anchor — "path/file.rs::symbol" (or a plain path) entries,
/// 2. backtick — `` `token` `` spans in the combined text,
/// 3. token — whitespace-split path-like tokens with a known extension,
/// 4. topic — reserved for later `topic_key` extraction (no-op).
///
/// Dedupe key is [`normalize_entity_name`] of the canonical
/// display name; first occurrence wins and later sources' mentions are
/// dropped entirely (offsets included).
pub fn extract_entities(title: &str, content: &str, anchors: &[String]) -> Vec<RawEntity> {
    let combined = format!("{title}\n{content}");
    let mut out: Vec<RawEntity> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for anchor in anchors {
        let anchor = anchor.trim();
        if anchor.is_empty() {
            continue;
        }
        let mut emit = |kind: EntityKind, name: String| {
            push_deduped(
                &mut out,
                &mut seen,
                RawEntity {
                    kind,
                    name,
                    offsets: NOT_IN_TEXT,
                    source: MentionSource::Anchor,
                },
            );
        };
        match anchor.split_once("::") {
            Some((path, symbol)) => {
                emit(EntityKind::File, canonical_file(path));
                emit(EntityKind::Symbol, canonical_symbol(symbol));
            }
            None => emit(EntityKind::File, canonical_file(anchor)),
        }
    }

    let mut backticks: Vec<RawEntity> = Vec::new();
    let chars: Vec<char> = combined.chars().collect();
    let mut i = 0;
    while i < chars.len() && backticks.len() < MAX_BACKTICK_ENTITIES {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        match (i + 1..chars.len()).find(|&j| chars[j] == '`') {
            Some(end) => {
                let span = (i + 1, end);
                let token: String = chars[span.0..span.1].iter().collect();
                if is_backtick_candidate(&token) {
                    backticks.push(RawEntity {
                        kind: classify_token(&token),
                        name: token.clone(),
                        offsets: span,
                        source: MentionSource::Backtick,
                    });
                }
                i = end + 1;
            }
            None => break,
        }
    }
    for entity in backticks {
        push_deduped(&mut out, &mut seen, entity);
    }

    let mut token_count = 0;
    for token in combined.split_whitespace() {
        if token_count >= MAX_TOKEN_ENTITIES {
            break;
        }
        if is_path_like_token(token) {
            token_count += 1;
            push_deduped(
                &mut out,
                &mut seen,
                RawEntity {
                    kind: EntityKind::File,
                    name: token.to_string(),
                    offsets: NOT_IN_TEXT,
                    source: MentionSource::Token,
                },
            );
        }
    }

    // TIER 4 topic (source Topic): no-op for now. `topic_key` extraction will
    // register its mentions here once defined (spec: graph-briefing).

    out
}

/// Entity-name candidates for a query string: backtick tokens first, then
/// path-like tokens, capped at [`MAX_QUERY_ENTITIES`]. Returns canonical
/// display names; callers normalize via
/// [`normalize_entity_name`].
pub fn query_entities(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() && out.len() < MAX_QUERY_ENTITIES {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        match (i + 1..chars.len()).find(|&j| chars[j] == '`') {
            Some(end) => {
                let token: String = chars[i + 1..end].iter().collect();
                if is_backtick_candidate(&token) {
                    let name = token;
                    let norm = normalize_entity_name(&name);
                    if !norm.is_empty() && !out.contains(&name) {
                        out.push(name);
                    }
                }
                i = end + 1;
            }
            None => break,
        }
    }
    for token in text.split_whitespace() {
        if out.len() >= MAX_QUERY_ENTITIES {
            break;
        }
        if is_path_like_token(token) {
            let name = token.to_string();
            let norm = normalize_entity_name(&name);
            if !norm.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// Backtick token filters: 1..=64 chars, non-empty after trimming whitespace.
fn is_backtick_candidate(token: &str) -> bool {
    let trimmed = token.trim();
    !trimmed.is_empty() && token.chars().count() <= MAX_BACKTICK_LEN && trimmed.chars().count() >= 1
}

/// File iff the token contains '/' or ends with a known source-file extension;
/// otherwise a [`EntityKind::Concept`].
fn classify_token(token: &str) -> EntityKind {
    if token.contains('/') || has_file_extension(token) {
        EntityKind::File
    } else {
        EntityKind::Concept
    }
}

/// Path-like plain tokens must contain '/' and end with a known extension.
fn is_path_like_token(token: &str) -> bool {
    token.contains('/') && has_file_extension(token)
}

fn has_file_extension(token: &str) -> bool {
    FILE_EXTENSIONS.iter().any(|ext| token.ends_with(ext))
}

/// File display name: keep the path as written, minus a leading "./".
fn canonical_file(path: &str) -> String {
    path.strip_prefix("./").unwrap_or(path).to_string()
}

/// Symbol display name: strip a leading "&" and trailing parens (e.g.
/// "foo()" / "&foo()").
fn canonical_symbol(symbol: &str) -> String {
    let stripped = symbol.strip_prefix('&').unwrap_or(symbol);
    stripped.strip_suffix("()").unwrap_or(stripped).to_string()
}

/// Skip entities whose normalized name is empty or already seen
/// (first occurrence wins; later mentions are dropped, not merged).
fn push_deduped(
    out: &mut Vec<RawEntity>,
    seen: &mut std::collections::HashSet<String>,
    e: RawEntity,
) {
    let norm = normalize_entity_name(&e.name);
    if norm.is_empty() || !seen.insert(norm) {
        return;
    }
    out.push(e);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor_list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn anchor_split_path_and_symbol() {
        let ents = extract_entities("t", "c", &anchor_list(&["src/main.rs::run"]));
        assert_eq!(ents.len(), 2);
        assert_eq!(ents[0].kind, EntityKind::File);
        assert_eq!(ents[0].name, "src/main.rs");
        assert_eq!(ents[0].source, MentionSource::Anchor);
        assert_eq!(ents[0].offsets, NOT_IN_TEXT);
        assert_eq!(ents[1].kind, EntityKind::Symbol);
        assert_eq!(ents[1].name, "run");
        assert_eq!(ents[1].source, MentionSource::Anchor);
    }

    #[test]
    fn anchor_path_only_treated_as_file() {
        let ents = extract_entities("t", "c", &anchor_list(&["src/main.rs"]));
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].kind, EntityKind::File);
        assert_eq!(ents[0].name, "src/main.rs");
    }

    #[test]
    fn backtick_offsets_slice_combined_text() {
        let title = "Fix loader";
        let content = "see `crates/foo/bar.rs` and `Widget` now";
        let ents = extract_entities(title, content, &[]);
        let files: Vec<&RawEntity> = ents
            .iter()
            .filter(|e| e.source == MentionSource::Backtick)
            .collect();
        assert_eq!(files.len(), 2);
        for e in &files {
            let combined = format!("{title}\n{content}");
            let sliced: String = combined.chars().collect::<Vec<char>>()[e.offsets.0..e.offsets.1]
                .iter()
                .collect();
            assert_eq!(sliced, e.name);
        }
        assert_eq!(files[0].name, "crates/foo/bar.rs");
        assert_eq!(files[0].kind, EntityKind::File);
        assert_eq!(files[1].name, "Widget");
        assert_eq!(files[1].kind, EntityKind::Concept);
    }

    #[test]
    fn dedupe_priority_anchor_over_backtick() {
        let ents = extract_entities(
            "title has `src/main.rs`",
            "more",
            &anchor_list(&["src/main.rs"]),
        );
        let main: Vec<&RawEntity> = ents.iter().filter(|e| e.name == "src/main.rs").collect();
        assert_eq!(main.len(), 1);
        assert_eq!(main[0].source, MentionSource::Anchor);
    }

    #[test]
    fn extension_classification() {
        let mk = |tok: &str| classify_token(tok);
        assert_eq!(mk("a/b.py"), EntityKind::File);
        assert_eq!(mk("a/b.yaml"), EntityKind::File);
        assert_eq!(mk("a/b.json"), EntityKind::File);
        assert_eq!(mk("a/b.go"), EntityKind::File);
        assert_eq!(mk("a/b.rs"), EntityKind::File);
        assert_eq!(mk("a/b.weird"), EntityKind::File);
        assert_eq!(mk("b.rs"), EntityKind::File);
        assert_eq!(mk("SomeName"), EntityKind::Concept);
    }

    #[test]
    fn token_tier_pathlike_only_with_extension() {
        let ents = extract_entities("t", "edit crates/x/y.rs and src/main.rs", &[]);
        let tokens: Vec<&RawEntity> = ents
            .iter()
            .filter(|e| e.source == MentionSource::Token)
            .collect();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].name, "crates/x/y.rs");
        assert_eq!(tokens[1].name, "src/main.rs");
    }

    #[test]
    fn canonical_name_forms() {
        let ents = extract_entities("t", "c", &anchor_list(&["./src/lib.rs::&helper()"]));
        assert_eq!(ents[0].name, "src/lib.rs");
        assert_eq!(ents[1].name, "helper");
    }

    #[test]
    fn query_entities_cap_three_priority_backtick() {
        let q = "why does `alpha.rs` fail in `beta.rs` vs `gamma.rs` and `delta.rs`?";
        let out = query_entities(q);
        assert_eq!(
            out,
            vec![
                "alpha.rs".to_string(),
                "beta.rs".to_string(),
                "gamma.rs".to_string()
            ]
        );
    }

    #[test]
    fn query_entities_falls_back_to_path_tokens() {
        let out = query_entities("check src/lib.rs please");
        assert_eq!(out, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn empty_inputs_yield_no_entities() {
        assert!(extract_entities("", "", &[]).is_empty());
        assert!(query_entities("").is_empty());
        assert!(extract_entities("", "", &anchor_list(&["", "   "])).is_empty());
    }
}
