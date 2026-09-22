//! `smem context compile` — pack durable memory into a host-window brief.
//!
//! Makes a 200k / 1M (or smaller) agent window act like a much larger working
//! set by compiling decisions, anchored code, facts, and related observations
//! under a token budget (Wave 2 effective-context).

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Instant;

use statefulmemory_daemon::token_budget::pack_proto_by_slots;
use statefulmemory_proto as p;

use crate::audit::{self, AuditEntry};
use crate::cli::ContextCompileArgs;
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::Formatter;

/// Map `--window` presets to estimated token budgets (chars/4 estimator).
pub fn window_to_tokens(window: &str) -> Option<i32> {
    match window.trim().to_ascii_lowercase().as_str() {
        "8k" => Some(8_000),
        "32k" => Some(32_000),
        "128k" => Some(128_000),
        "200k" => Some(200_000),
        "1m" | "1000k" => Some(1_000_000),
        other => other.parse::<i32>().ok().filter(|&n| n > 0),
    }
}

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: ContextCompileArgs,
) -> ExitCode {
    let started = Instant::now();
    let max_tokens = a
        .max_tokens
        .or_else(|| a.window.as_deref().and_then(window_to_tokens))
        .unwrap_or(32_000);
    let window_label = a
        .window
        .clone()
        .unwrap_or_else(|| format!("{max_tokens}"));

    let req = p::ContextRequest {
        project_name: project_name.to_string(),
        recent_limit: a.limit,
        mode: Some(a.mode.clone()),
        rerank: a.rerank.clone(),
        query: a.query.clone(),
        anchor: a.anchor.clone(),
        include_stale: a.include_stale,
        max_tokens: Some(max_tokens),
    };
    let resp = match client.context(req).await {
        Ok(r) => r.into_inner(),
        Err(s) => {
            eprintln!("statefulmemory: {}", s.message());
            return ExitCode::from(exit::from_status(s.code()));
        }
    };

    let rpc_tokens = resp.tokens_used;
    let hits = resp
        .snapshot
        .map(|s| s.recent_observations)
        .unwrap_or_default();
    let (decisions, anchored, other, slot_used) =
        pack_proto_by_slots(hits, max_tokens.max(0) as u32);

    let mut facts_block: Vec<(i64, Vec<p::Fact>)> = Vec::new();
    if !a.no_facts {
        let ids: Vec<i64> = decisions
            .iter()
            .chain(anchored.iter())
            .chain(other.iter())
            .map(|o| o.id)
            .take(12)
            .collect();
        for id in ids {
            let fr = p::GetFactsRequest {
                project_name: project_name.to_string(),
                observation_id: id,
            };
            if let Ok(r) = client.get_facts(fr).await {
                let facts = r.into_inner().facts;
                if !facts.is_empty() {
                    facts_block.push((id, facts));
                }
            }
        }
    }

    let total = decisions.len() + anchored.len() + other.len();

    let write_result = if fmt != Formatter::Text {
        let decisions_j: Vec<serde_json::Value> = decisions.iter().map(obs_json).collect();
        let anchored_j: Vec<serde_json::Value> = anchored.iter().map(obs_json).collect();
        let other_j: Vec<serde_json::Value> = other.iter().map(obs_json).collect();
        let facts_j: Vec<serde_json::Value> = facts_block
            .iter()
            .map(|(id, facts)| {
                serde_json::json!({
                    "observation_id": id,
                    "facts": facts.iter().map(|f| serde_json::json!({
                        "subject": f.subject,
                        "predicate": f.predicate,
                        "object": f.object,
                        "salience": f.salience,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let out = serde_json::json!({
            "window": window_label,
            "max_tokens": max_tokens,
            "tokens_used": slot_used,
            "context_tokens_used": rpc_tokens,
            "decisions": decisions_j,
            "anchored": anchored_j,
            "other": other_j,
            "facts": facts_j,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        Ok(())
    } else {
        write_text_brief(
            &window_label,
            max_tokens,
            slot_used,
            rpc_tokens,
            &decisions,
            &anchored,
            &other,
            &facts_block,
        )
    };

    if let Err(e) = write_result {
        eprintln!("statefulmemory: i/o error: {e}");
        return ExitCode::from(exit::GENERAL);
    }

    audit::record(&AuditEntry {
        ts: audit::now_rfc3339(),
        command: "context.compile",
        project: Some(project_name),
        result_count: Some(total),
        duration_ms: started.elapsed().as_millis(),
        query: a.query.clone(),
        top_hits: None,
        ..Default::default()
    });
    ExitCode::SUCCESS
}

fn obs_json(o: &p::Observation) -> serde_json::Value {
    serde_json::json!({
        "id": o.id,
        "type": o.r#type,
        "title": o.title,
        "content": o.content,
        "code_anchor": o.code_anchor,
        "verify_state": o.verify_state,
        "topic_key": o.topic_key,
    })
}

fn write_text_brief(
    window_label: &str,
    max_tokens: i32,
    slot_used: u32,
    rpc_tokens: Option<i32>,
    decisions: &[p::Observation],
    anchored: &[p::Observation],
    other: &[p::Observation],
    facts_block: &[(i64, Vec<p::Fact>)],
) -> io::Result<()> {
    let stdout = io::stdout();
    let mut h = stdout.lock();
    writeln!(
        h,
        "# Effective context compile (window={window_label}, budget={max_tokens})"
    )?;
    writeln!(
        h,
        "_Host window pack — durable store stays outside the KV cache._"
    )?;
    writeln!(h)?;

    if !decisions.is_empty() {
        writeln!(h, "## Decisions")?;
        for o in decisions {
            write_obs(&mut h, o)?;
        }
        writeln!(h)?;
    }
    if !anchored.is_empty() {
        writeln!(h, "## Anchored code")?;
        for o in anchored {
            write_obs(&mut h, o)?;
        }
        writeln!(h)?;
    }
    if !facts_block.is_empty() {
        writeln!(h, "## Facts")?;
        for (oid, facts) in facts_block {
            for f in facts {
                writeln!(
                    h,
                    "- (obs #{oid}) {} — {} — {}",
                    f.subject.trim(),
                    f.predicate.trim(),
                    f.object.trim()
                )?;
            }
        }
        writeln!(h)?;
        // Chain-of-evidence: link decision titles to supporting fact triples
        // (Wave 2 multi-hop briefing cue for the host model).
        let decision_ids: std::collections::HashSet<i64> =
            decisions.iter().map(|o| o.id).collect();
        let mut chains: Vec<String> = Vec::new();
        for (oid, facts) in facts_block {
            if !decision_ids.contains(oid) {
                continue;
            }
            let title = decisions
                .iter()
                .find(|o| o.id == *oid)
                .map(|o| o.title.trim())
                .unwrap_or("?");
            for f in facts.iter().take(3) {
                chains.push(format!(
                    "- **{title}** ← {} / {} / {}",
                    f.subject.trim(),
                    f.predicate.trim(),
                    f.object.trim()
                ));
            }
        }
        if !chains.is_empty() {
            writeln!(h, "## Chain of evidence")?;
            for line in chains {
                writeln!(h, "{line}")?;
            }
            writeln!(h)?;
        }
    }
    if !other.is_empty() {
        writeln!(h, "## Related observations")?;
        for o in other {
            write_obs(&mut h, o)?;
        }
        writeln!(h)?;
    }
    writeln!(
        h,
        "## tokens_used (estimate) {slot_used} / {max_tokens}  (rpc={})",
        rpc_tokens
            .map(|t| t.to_string())
            .unwrap_or_else(|| "n/a".into())
    )?;
    Ok(())
}

fn write_obs(h: &mut impl Write, o: &p::Observation) -> io::Result<()> {
    let anchor = o
        .code_anchor
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| format!(" `{s}`"))
        .unwrap_or_default();
    let vs = o
        .verify_state
        .as_deref()
        .filter(|s| !s.is_empty() && *s != "unanchored")
        .map(|s| format!(" [{s}]"))
        .unwrap_or_default();
    writeln!(h, "### [{}] {}{anchor}{vs}", o.r#type, o.title.trim())?;
    writeln!(h, "{}", o.content.trim_end())?;
    writeln!(h)?;
    Ok(())
}
