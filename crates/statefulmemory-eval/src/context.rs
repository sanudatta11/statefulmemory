//! Fact-first answer context assembly.

use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactContext {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub temporal: Option<String>,
    pub source_ids: Vec<String>,
    pub excerpts: Vec<String>,
}

impl FactContext {
    fn key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.subject, self.predicate, self.object
        )
        .to_ascii_lowercase()
    }
    fn render(&self, index: usize) -> String {
        let temporal = self.temporal.as_deref().unwrap_or("unknown");
        let sources = if self.source_ids.is_empty() {
            "unknown".into()
        } else {
            self.source_ids.join(", ")
        };
        let excerpts = if self.excerpts.is_empty() {
            String::new()
        } else {
            format!("\n  supporting excerpts:\n{}", self.excerpts.join("\n"))
        };
        format!("[Fact {index}]\nsubject: {}\npredicate: {}\nobject: {}\ntemporal: {}\nsource observation/dialogue ids: {}{}", self.subject, self.predicate, self.object, temporal, sources, excerpts)
    }
}

/// Pack facts first, deduplicating by subject/predicate/object. Raw hits are
/// retained as supporting observations for extraction-miss fallbacks.
pub fn pack_fact_context(hits: &[String], max_chars: usize) -> Vec<String> {
    let mut facts: BTreeMap<String, FactContext> = BTreeMap::new();
    let mut raw = Vec::new();
    for hit in hits {
        let (fact, excerpt) = parse_fact_hit(hit);
        if let Some(mut fact) = fact {
            let key = fact.key();
            if let Some(existing) = facts.get_mut(&key) {
                existing.source_ids.append(&mut fact.source_ids);
                existing.excerpts.append(&mut fact.excerpts);
                existing.excerpts.push(excerpt);
                dedup(&mut existing.source_ids);
                dedup(&mut existing.excerpts);
            } else {
                fact.excerpts.push(excerpt);
                facts.insert(key, fact);
            }
        } else if !hit.trim().is_empty() {
            raw.push(hit.clone());
        }
    }
    let mut out = Vec::new();
    let mut used = 0usize;
    for (i, fact) in facts.values().enumerate() {
        let rendered = fact.render(i + 1);
        if used + rendered.len() > max_chars && !out.is_empty() {
            break;
        }
        used += rendered.len() + 2;
        out.push(rendered);
    }
    for hit in raw {
        let rendered = format!("[Supporting observation]\n{hit}");
        if used + rendered.len() > max_chars && !out.is_empty() {
            break;
        }
        used += rendered.len() + 2;
        out.push(rendered);
    }
    out
}

fn parse_fact_hit(hit: &str) -> (Option<FactContext>, String) {
    let mut lines = hit.lines();
    let Some(first) = lines.next() else {
        return (None, String::new());
    };
    let mut line = first.trim();
    let mut source_ids = Vec::new();
    if let Some(end) = line
        .strip_prefix('[')
        .and_then(|s| s.find(']').map(|i| i + 1))
    {
        let prefix = &line[1..end - 1];
        if prefix.starts_with('D') && prefix.contains(':') {
            source_ids.push(prefix.to_string());
            line = line[end..].trim();
        }
    }
    let mut temporal = None;
    if let Some(end) = line
        .strip_prefix('[')
        .and_then(|s| s.find(']').map(|i| i + 1))
    {
        temporal = Some(line[1..end - 1].to_string());
        line = line[end..].trim();
    }
    let (subject, predicate, object) = if let Some((s, rest)) = line.split_once(" - ") {
        let Some((p, o)) = rest.split_once(" - ") else {
            return (None, hit.to_string());
        };
        (s.to_string(), p.to_string(), o.to_string())
    } else {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() < 3 {
            return (None, hit.to_string());
        }
        (
            words[0].to_string(),
            words[1].to_string(),
            words[2..].join(" "),
        )
    };
    if object.is_empty() {
        return (None, hit.to_string());
    }
    let mut excerpts = Vec::new();
    for line in lines {
        let line = line.trim();
        if !line.is_empty() {
            if let Some(id) = extract_source_id(line) {
                source_ids.push(id);
            }
            excerpts.push(line.to_string());
        }
    }
    dedup(&mut source_ids);
    (
        Some(FactContext {
            subject,
            predicate,
            object,
            temporal,
            source_ids,
            excerpts,
        }),
        hit.to_string(),
    )
}

fn extract_source_id(s: &str) -> Option<String> {
    let start = s.find('D')?;
    let tail = &s[start..];
    let end = tail
        .find(|c: char| !c.is_ascii_alphanumeric() && c != ':')
        .unwrap_or(tail.len());
    let id = &tail[..end];
    (id.contains(':') && id[1..].chars().all(|c| c.is_ascii_digit() || c == ':'))
        .then(|| id.to_string())
}

fn dedup(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|v| seen.insert(v.to_ascii_lowercase()));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deduplicates_facts_and_preserves_sources() {
        let hits = vec![
            "[D1:3] Caroline attended group\n  - [D1:3] source one".into(),
            "[D1:5] Caroline attended group\n  - [D1:5] source two".into(),
        ];
        let packed = pack_fact_context(&hits, 10_000).join("\n");
        assert_eq!(packed.matches("[Fact 1]").count(), 1);
        assert!(packed.contains("D1:3") && packed.contains("D1:5"));
    }
}
