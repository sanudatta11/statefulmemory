// Generated with AI Coding Rules Hub
//! Entity extraction for the retrieval-upgrade-v1 P2 entity layer.
//!
//! After we extract atomic facts (`prompt::parse_facts`), we run a second
//! lightweight pass to pull canonical entity names out of each fact's
//! `subject` and `object` strings. The names are stored in the eval-side
//! `entities` + `entity_links` tables (spec §5) and exposed at query time
//! to boost facts whose entities match query entities (spec
//! §3 audit, SC-12 / SC-13).
//!
//! This module ships **two** extractors so callers can choose:
//! * [`HeuristicEntityExtractor`] — pure-Rust, no LLM. Splits on whitespace
//!   and conjunctions, lowercases, strips punctuation, drops stopwords and
//!   tokens shorter than 3 chars. Cheap, deterministic, good for synthetic
//!   benchmarks (BEAM) and as an offline baseline.
//! * [`HaikuEntityExtractor`] — LLM-backed. Sends a JSON-prompt to the
//!   `claude` CLI asking for `{"entities": ["...", ...]}`; on parse failure
//!   falls back to the heuristic extractor (so the pipeline never crashes
//!   on a flaky LLM response). Slightly more accurate for free-form
//!   conversational text (LoCoMo, LongMemEval).
//!
//! Spec links: TS-19, SC-12. Plan: P2 §4.5 (entity-match follow-up).

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::claude_cli::{ClaudeClient, HAIKU_MODEL};
use crate::Fact;

/// Trait for entity extractors. Returns canonical lowercased entity names
/// for a single fact (deduped, ordered by first appearance).
#[async_trait]
pub trait EntityExtractor: Send + Sync {
    async fn extract(&self, fact: &Fact) -> Result<Vec<String>>;
}

/// Stopwords for the heuristic extractor — short common words, articles,
/// pronouns, helpers. Kept small; the canonical name normalization plus
/// the `>=3 chars` filter does most of the work.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "of", "in", "on", "at", "to",
    "for", "with", "by", "from", "as", "is", "are", "was", "were", "be",
    "been", "being", "do", "does", "did", "have", "has", "had",
    "this", "that", "these", "those", "it", "its", "they", "them",
    "i", "me", "my", "you", "your", "we", "us", "our",
    "would", "could", "should", "will", "shall", "may", "might", "can",
];

/// Heuristic, LLM-free entity extractor.
///
/// Pulls candidate tokens from `subject` and `object` (the two fields most
/// likely to carry entities), normalizes to lowercase, drops stopwords and
/// tokens under 3 chars, dedupes. Cheap and deterministic — good enough for
/// synthetic benchmarks and as a fallback when the LLM extractor fails.
pub struct HeuristicEntityExtractor;

impl HeuristicEntityExtractor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HeuristicEntityExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EntityExtractor for HeuristicEntityExtractor {
    async fn extract(&self, fact: &Fact) -> Result<Vec<String>> {
        Ok(extract_heuristic_tokens(&format!(
            "{} {}",
            fact.subject, fact.object
        )))
    }
}

/// Pure helper exposed for tests: tokenize a free-form string into the
/// heuristic entity name set.
pub fn extract_heuristic_tokens(text: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for tok in text.split(|c: char| !c.is_alphanumeric() && c != '\'') {
        if tok.len() < 3 {
            continue;
        }
        let lower = tok.to_lowercase();
        if STOPWORDS.contains(&lower.as_str()) {
            continue;
        }
        if seen.insert(lower.clone()) {
            out.push(lower);
        }
    }
    out
}

/// LLM-backed entity extractor. Sends a single JSON prompt to the `claude`
/// CLI and parses the response. Falls back to [`HeuristicEntityExtractor`]
/// on any parse failure so the pipeline never crashes on a flaky response.
pub struct HaikuEntityExtractor {
    client: Arc<dyn ClaudeClient>,
    fallback: HeuristicEntityExtractor,
}

impl HaikuEntityExtractor {
    pub fn new(client: Arc<dyn ClaudeClient>) -> Self {
        Self {
            client,
            fallback: HeuristicEntityExtractor::new(),
        }
    }

    fn build_prompt(fact: &Fact) -> String {
        format!(
            "Extract canonical entity names (people, places, things) from this \
             fact. Return only a JSON object of the form \
             {{\"entities\": [\"name1\", \"name2\", ...]}}. Lowercase each \
             name. Output nothing else.\n\n\
             Fact: subject=\"{}\" predicate=\"{}\" object=\"{}\"",
            fact.subject, fact.predicate, fact.object
        )
    }

    fn parse_entities(raw: &str) -> Option<Vec<String>> {
        // Trim to the first '{' and last '}' to strip prose.
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        if end < start {
            return None;
        }
        let slice = &raw[start..=end];

        #[derive(serde::Deserialize)]
        struct Wrapper {
            entities: Vec<String>,
        }

        let parsed: Wrapper = serde_json::from_str(slice).ok()?;

        // Normalize: lowercase, drop empty, dedup preserving first-seen order.
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for e in parsed.entities {
            let lower = e.trim().to_lowercase();
            if lower.is_empty() {
                continue;
            }
            if seen.insert(lower.clone()) {
                out.push(lower);
            }
        }
        Some(out)
    }
}

#[async_trait]
impl EntityExtractor for HaikuEntityExtractor {
    async fn extract(&self, fact: &Fact) -> Result<Vec<String>> {
        let prompt = Self::build_prompt(fact);
        match self.client.ask(&prompt, HAIKU_MODEL).await {
            Ok(raw) => match Self::parse_entities(&raw) {
                Some(ents) if !ents.is_empty() => Ok(ents),
                _ => {
                    tracing::warn!(
                        target: "memlayer_extract::entities",
                        "Haiku entity response unparseable; falling back to heuristic"
                    );
                    self.fallback.extract(fact).await
                }
            },
            Err(e) => {
                tracing::warn!(
                    target: "memlayer_extract::entities",
                    error = %e,
                    "Haiku entity call failed; falling back to heuristic"
                );
                self.fallback.extract(fact).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(subject: &str, object: &str) -> Fact {
        Fact {
            subject: subject.into(),
            predicate: "is".into(),
            object: object.into(),
            temporal: None,
            salience: 1.0,
            evidence_obs_id: 1,
            source_session: None,
        }
    }

    #[test]
    fn heuristic_picks_proper_nouns() {
        let f = fact("Caroline", "the LGBTQ support group");
        let toks = extract_heuristic_tokens(&format!("{} {}", f.subject, f.object));
        assert!(toks.contains(&"caroline".to_string()));
        assert!(toks.contains(&"lgbtq".to_string()));
        assert!(toks.contains(&"support".to_string()));
        assert!(toks.contains(&"group".to_string()));
        // 'the' stripped as stopword.
        assert!(!toks.contains(&"the".to_string()));
    }

    #[test]
    fn heuristic_dedupes() {
        let toks = extract_heuristic_tokens("Caroline Caroline Caroline");
        assert_eq!(toks, vec!["caroline".to_string()]);
    }

    #[test]
    fn heuristic_drops_short_tokens() {
        let toks = extract_heuristic_tokens("a is on at to");
        assert!(toks.is_empty());
    }

    #[tokio::test]
    async fn heuristic_extractor_works_end_to_end() {
        let ex = HeuristicEntityExtractor::new();
        let f = fact("Caroline", "Paris");
        let ents = ex.extract(&f).await.unwrap();
        assert!(ents.contains(&"caroline".to_string()));
        assert!(ents.contains(&"paris".to_string()));
    }

    #[test]
    fn parse_entities_strips_prose() {
        let raw = r#"Sure, here's the JSON: {"entities": ["caroline", "PARIS"]}. Hope that helps!"#;
        let ents = HaikuEntityExtractor::parse_entities(raw).unwrap();
        assert_eq!(ents, vec!["caroline".to_string(), "paris".to_string()]);
    }

    #[test]
    fn parse_entities_returns_none_on_garbage() {
        assert!(HaikuEntityExtractor::parse_entities("no json here").is_none());
    }

    #[tokio::test]
    async fn haiku_falls_back_on_parse_failure() {
        use crate::claude_cli::MockClaudeClient;
        let mock = Arc::new(MockClaudeClient::with_responses(vec![
            "totally not json".to_string(),
        ]));
        let ex = HaikuEntityExtractor::new(mock);
        let f = fact("Caroline", "Paris");
        // Should NOT error — falls back to heuristic.
        let ents = ex.extract(&f).await.unwrap();
        assert!(ents.contains(&"caroline".to_string()));
        assert!(ents.contains(&"paris".to_string()));
    }
}
