//! Detect and invoke the user's coding-agent CLI for LLM shell-outs.
//!
//! memlayer is not tied to one vendor. Extract, conflict, rerank, resolve,
//! and Decide call whichever agent binary is on PATH (or
//! `MEMLAYER_LLM_BIN` / `MEMLAYER_LLM_PROVIDER`).
//!
//! Hybrid *retrieval* itself is local (BM25 + BGE-small). Only the optional
//! rerank / extract / judge steps need an agent CLI.
//!
//! Model *roles* (`fast` / `capable`, plus legacy `haiku` / `sonnet`
//! aliases) map to a provider-specific id when we know one; otherwise the
//! `--model` flag is omitted so the agent uses its own default.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::debug;

use crate::opencode_models;

const LLM_TIMEOUT: Duration = Duration::from_secs(120);

/// `(id, binary names, fast model, capable model)` for one known agent CLI.
type ProviderSpec = (
    &'static str,
    &'static [&'static str],
    Option<&'static str>,
    Option<&'static str>,
);

/// One supported agent CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub bin: String,
    pub fast_model: Option<&'static str>,
    pub capable_model: Option<&'static str>,
}

fn specs() -> &'static [ProviderSpec] {
    // Prefer CLIs that work well headless (API-key / providers auth) before
    // IDE agents that often need an interactive `login` for `-p` mode.
    // Override anytime with MEMLAYER_LLM_BIN / MEMLAYER_LLM_PROVIDER.
    &[
        (
            "opencode",
            &["opencode"],
            Some(opencode_models::OPENCODE_FAST),
            Some(opencode_models::OPENCODE_CAPABLE),
        ),
        (
            "kilo",
            &["kilo"],
            Some(opencode_models::OPENCODE_FAST),
            Some(opencode_models::OPENCODE_CAPABLE),
        ),
        (
            "gemini",
            &["gemini"],
            Some("gemini-2.5-flash"),
            Some("gemini-2.5-pro"),
        ),
        (
            "claude",
            &["claude"],
            Some("claude-haiku-4-5"),
            Some("claude-sonnet-4-6"),
        ),
        ("amazon-q", &["q"], None, None),
        ("kimi", &["kimi"], None, None),
        ("codex", &["codex"], None, None),
        ("cursor", &["cursor-agent", "agent"], None, None),
        ("copilot", &["copilot"], None, None),
        ("windsurf", &["windsurf"], None, None),
        ("antigravity", &["antigravity", "agy"], None, None),
        ("zcode", &["zcode"], None, None),
    ]
}

fn home_hints(id: &str) -> &'static [&'static str] {
    match id {
        "cursor" => &[".cursor"],
        "copilot" => &[".copilot"],
        "gemini" => &[".gemini"],
        "claude" => &[".claude"],
        "codex" => &[".codex"],
        "opencode" => &[".config/opencode"],
        "kilo" => &[".config/kilo", ".kilo", ".kilocode"],
        "amazon-q" => &[".aws/amazonq"],
        "kimi" => &[".kimi", ".kimi-code"],
        "windsurf" => &[".codeium/windsurf"],
        "antigravity" => &[".gemini/antigravity", ".gemini/antigravity-cli"],
        "zcode" => &[".zcode"],
        _ => &[],
    }
}

pub fn which_bin(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn home_has_hint(home: &Path, id: &str) -> bool {
    home_hints(id).iter().any(|rel| home.join(rel).exists())
}

fn lookup_spec(id: &str) -> Option<ProviderSpec> {
    specs().iter().copied().find(|(sid, _, _, _)| *sid == id)
}

fn make_provider(
    id: &'static str,
    bin: String,
    fast: Option<&'static str>,
    capable: Option<&'static str>,
) -> Provider {
    Provider {
        id,
        bin,
        fast_model: fast,
        capable_model: capable,
    }
}

/// Resolve the LLM CLI for this machine.
///
/// Order: `MEMLAYER_LLM_BIN` → `MEMLAYER_LLM_PROVIDER` → config-dir match
/// with a binary on PATH → first known binary on PATH.
pub fn detect_provider() -> Option<Provider> {
    if let Ok(bin) = std::env::var("MEMLAYER_LLM_BIN") {
        let bin = bin.trim();
        if !bin.is_empty() {
            let hinted = std::env::var("MEMLAYER_LLM_PROVIDER")
                .ok()
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty() && s != "auto");
            let id = hinted
                .as_deref()
                .and_then(lookup_spec)
                .map(|s| s.0)
                .unwrap_or_else(|| infer_id_from_bin(bin));
            if let Some((sid, _, fast, capable)) = lookup_spec(id) {
                return Some(make_provider(sid, bin.to_string(), fast, capable));
            }
            return Some(make_provider("generic", bin.to_string(), None, None));
        }
    }

    if let Ok(p) = std::env::var("MEMLAYER_LLM_PROVIDER") {
        let p = p.trim().to_ascii_lowercase();
        if !p.is_empty() && p != "auto" {
            if let Some((id, bins, fast, capable)) = lookup_spec(&p) {
                for b in bins {
                    if let Some(path) = which_bin(b) {
                        return Some(make_provider(
                            id,
                            path.to_string_lossy().into_owned(),
                            fast,
                            capable,
                        ));
                    }
                }
                return Some(make_provider(id, bins[0].to_string(), fast, capable));
            }
        }
    }

    if let Some(home) = home_dir() {
        for (id, bins, fast, capable) in specs() {
            if home_has_hint(&home, id) {
                for b in *bins {
                    if let Some(path) = which_bin(b) {
                        return Some(make_provider(
                            id,
                            path.to_string_lossy().into_owned(),
                            *fast,
                            *capable,
                        ));
                    }
                }
            }
        }
    }

    for (id, bins, fast, capable) in specs() {
        for b in *bins {
            if let Some(path) = which_bin(b) {
                return Some(make_provider(
                    id,
                    path.to_string_lossy().into_owned(),
                    *fast,
                    *capable,
                ));
            }
        }
    }
    None
}

fn infer_id_from_bin(bin: &str) -> &'static str {
    let name = Path::new(bin)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(bin);
    for (id, bins, _, _) in specs() {
        if bins.contains(&name) {
            return id;
        }
    }
    "generic"
}

/// True when any supported agent CLI is on PATH (or env override is set).
pub fn llm_cli_available() -> bool {
    detect_provider().is_some()
}

pub fn known_binaries() -> Vec<&'static str> {
    specs()
        .iter()
        .flat_map(|(_, bins, _, _)| *bins)
        .copied()
        .collect()
}

/// Role / empty request: omit `--model` and use the invoking agent's current
/// (or CLI-configured default) model.
pub fn is_unspecified_model(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "" | "auto"
            | "default"
            | "agent"
            | "current"
            | "fast"
            | "capable"
            | "haiku"
            | "sonnet"
            | "flash"
            | "mini"
            | "small"
            | "pro"
            | "large"
    )
}

/// Env vars an IDE/CLI may set to the model currently in use.
const HOST_MODEL_KEYS: &[&str] = &[
    "CURSOR_MODEL",
    "OPENCODE_MODEL",
    "KILO_MODEL",
    "ANTHROPIC_MODEL",
    "CLAUDE_MODEL",
    "GEMINI_MODEL",
    "COPILOT_MODEL",
    "AGY_MODEL",
    "QWEN_MODEL",
    "KIMI_MODEL",
];

fn nonempty_env(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(v) = std::env::var(key) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// Resolve what to send to the agent CLI.
///
/// Explicit `MEMLAYER_LLM_MODEL` always wins. Roles (`fast`/`haiku`/…) use
/// the host's current model env if set, otherwise omit `--model`.
pub fn effective_requested(caller: &str) -> String {
    if let Some(v) = nonempty_env(&["MEMLAYER_LLM_MODEL", "MEMLAYER_CLAUDE_MODEL"]) {
        return v;
    }
    if is_unspecified_model(caller) {
        return nonempty_env(HOST_MODEL_KEYS).unwrap_or_default();
    }
    caller.trim().to_string()
}

/// Map a role or vendor id onto this provider. `None` means omit `--model`
/// and use the agent's current/default model.
pub fn map_model(provider: &Provider, requested: &str) -> Option<String> {
    let r = effective_requested(requested);
    if r.is_empty() || is_unspecified_model(&r) {
        return None;
    }
    if matches!(provider.id, "opencode" | "kilo") {
        return opencode_models::resolve(&r);
    }
    let lower = r.to_ascii_lowercase();

    let role = match lower.as_str() {
        "fast" | "haiku" | "flash" | "mini" | "small" => Some("fast"),
        "capable" | "sonnet" | "pro" | "large" => Some("capable"),
        _ if provider.id != "claude"
            && (lower.contains("claude")
                || lower.contains("haiku")
                || lower.contains("sonnet")
                || lower.contains("opus")) =>
        {
            // Explicit Claude id on another CLI: inherit that CLI's model
            // instead of sending a Claude slug.
            return None;
        }
        _ => None,
    };

    match role {
        Some(_) => None,
        None => Some(r),
    }
}

/// argv after the binary. Prompt is always included.
pub fn build_args(provider_id: &str, prompt: &str, model: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    match provider_id {
        "codex" => {
            args.push("exec".into());
            args.push("--skip-git-repo-check".into());
            if let Some(m) = model {
                args.push("-m".into());
                args.push(m.to_string());
            }
            args.push(prompt.to_string());
        }
        "opencode" | "kilo" => {
            args.push("run".into());
            if let Some(m) = model {
                args.push("-m".into());
                args.push(m.to_string());
            }
            args.push(prompt.to_string());
        }
        "amazon-q" => {
            args.push("chat".into());
            args.push("--no-interactive".into());
            if let Some(m) = model {
                args.push("--model".into());
                args.push(m.to_string());
            }
            args.push(prompt.to_string());
        }
        _ => {
            args.push("-p".into());
            // Cursor: ask mode keeps eval/judge prompts read-only.
            if provider_id == "cursor" {
                args.push("--mode".into());
                args.push("ask".into());
            }
            args.push(prompt.to_string());
            if let Some(m) = model {
                args.push("--model".into());
                args.push(m.to_string());
            }
            if provider_id == "claude" {
                args.push("--output-format".into());
                args.push("text".into());
            }
        }
    }
    args
}

pub fn looks_like_unavailable_model(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("unknown model")
        || lower.contains("invalid model")
        || lower.contains("model not found")
        || lower.contains("not available")
        || lower.contains("does not exist")
        || lower.contains("bad model")
        || lower.contains("unrecognized model")
        || lower.contains("empty response for model")
        || lower.contains("model_not_found")
}

pub async fn invoke(provider: &Provider, prompt: &str, requested_model: &str) -> Result<String> {
    let mapped = map_model(provider, requested_model);
    let mut attempts: Vec<Option<String>> = Vec::new();
    if mapped.is_some() {
        attempts.push(mapped);
    }
    attempts.push(None);

    let mut last_err: Option<anyhow::Error> = None;
    let mut seen = std::collections::HashSet::new();
    for model in attempts {
        let key = model.clone().unwrap_or_default();
        if !seen.insert(key) {
            continue;
        }
        match invoke_once(provider, prompt, model.as_deref()).await {
            Ok(text) => return Ok(text),
            Err(e) => {
                let msg = format!("{e:#}");
                if looks_like_unavailable_model(&msg) {
                    tracing::info!(
                        provider = provider.id,
                        tried = %model.as_deref().unwrap_or("<agent-default>"),
                        "model unavailable; trying agent default"
                    );
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no LLM model available on {}", provider.id)))
}

async fn invoke_once(provider: &Provider, prompt: &str, model: Option<&str>) -> Result<String> {
    let args = build_args(provider.id, prompt, model);
    let mut cmd = Command::new(&provider.bin);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .env_remove("FTP_PROXY")
        .env_remove("ftp_proxy")
        .env_remove("GRPC_PROXY")
        .env_remove("grpc_proxy");

    let child = cmd
        .spawn()
        .with_context(|| format!("spawn {} — is it on PATH?", provider.bin))?;

    let output = timeout(LLM_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "{} timed out after {}s",
                provider.bin,
                LLM_TIMEOUT.as_secs()
            )
        })?
        .with_context(|| format!("wait for {}", provider.bin))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "{} exited with {}: stderr={} stdout={}",
            provider.bin,
            output.status,
            stderr,
            stdout
        );
    }

    let text = String::from_utf8(output.stdout)
        .with_context(|| format!("{} output was not valid UTF-8", provider.bin))?
        .trim()
        .to_string();
    if text.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{} returned empty response for model '{}' (bad model ID?): stderr={}",
            provider.bin,
            model.unwrap_or("<agent-default>"),
            stderr
        );
    }
    debug!(
        provider = provider.id,
        model = %model.unwrap_or("<agent-default>"),
        response_len = text.len(),
        "agent CLI ok"
    );
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_fast_role_maps_to_flash() {
        let p = Provider {
            id: "gemini",
            bin: "gemini".into(),
            fast_model: Some("gemini-2.5-flash"),
            capable_model: Some("gemini-2.5-pro"),
        };
        assert_eq!(map_model(&p, "fast"), None);
        assert_eq!(map_model(&p, "haiku"), None);
        assert_eq!(map_model(&p, "capable"), None);
        assert_eq!(map_model(&p, "claude-haiku-4-5"), None);
        assert_eq!(
            map_model(&p, "gemini-2.5-flash").as_deref(),
            Some("gemini-2.5-flash")
        );
        assert_eq!(map_model(&p, "auto"), None);
    }

    #[test]
    fn cursor_omits_model_for_roles() {
        let p = Provider {
            id: "cursor",
            bin: "cursor-agent".into(),
            fast_model: None,
            capable_model: None,
        };
        assert_eq!(map_model(&p, "fast"), None);
        assert_eq!(map_model(&p, "capable"), None);
        assert_eq!(map_model(&p, "gpt-4.1").as_deref(), Some("gpt-4.1"));
        assert!(is_unspecified_model("fast"));
        assert!(is_unspecified_model("haiku"));
        assert!(!is_unspecified_model("qwen"));
        assert!(!is_unspecified_model("opencode/glm-5.3"));
    }

    #[test]
    fn claude_args_include_output_format() {
        let args = build_args("claude", "hi", Some("claude-haiku-4-5"));
        assert!(args.contains(&"--output-format".into()));
        assert!(args.contains(&"claude-haiku-4-5".into()));
    }

    #[test]
    fn cursor_args_omit_model_when_none() {
        let args = build_args("cursor", "rank these", None);
        assert_eq!(
            args,
            vec![
                "-p".to_string(),
                "--mode".to_string(),
                "ask".to_string(),
                "rank these".to_string()
            ]
        );
    }

    #[test]
    fn headless_providers_precede_ide_agents() {
        let ids: Vec<_> = specs().iter().map(|(id, _, _, _)| *id).collect();
        let opencode = ids.iter().position(|id| *id == "opencode").unwrap();
        let cursor = ids.iter().position(|id| *id == "cursor").unwrap();
        assert!(
            opencode < cursor,
            "opencode should be preferred over cursor for headless eval"
        );
        assert!(ids.contains(&"gemini"));
        assert!(ids.contains(&"claude"));
        assert!(ids.contains(&"codex"));
        assert!(ids.contains(&"kilo"));
    }

    #[test]
    fn opencode_maps_zen_catalog_and_nicknames() {
        let p = Provider {
            id: "opencode",
            bin: "opencode".into(),
            fast_model: Some(opencode_models::OPENCODE_FAST),
            capable_model: Some(opencode_models::OPENCODE_CAPABLE),
        };
        assert_eq!(map_model(&p, "fast"), None);
        assert_eq!(map_model(&p, "glm").as_deref(), Some("opencode/glm-5.3"));
        assert_eq!(map_model(&p, "qwen").as_deref(), Some("opencode/qwen3.7-plus"));
        assert_eq!(map_model(&p, "haiku"), None);
        assert_eq!(
            map_model(&p, "claude-haiku-4-5").as_deref(),
            Some("opencode/claude-haiku-4-5")
        );
        let args = build_args("opencode", "rank these", map_model(&p, "glm").as_deref());
        assert_eq!(args[0], "run");
        assert!(args.contains(&"-m".into()));
        assert!(args.contains(&"opencode/glm-5.3".into()));
    }

    #[test]
    fn kilo_reuses_opencode_model_map() {
        let p = Provider {
            id: "kilo",
            bin: "kilo".into(),
            fast_model: Some(opencode_models::OPENCODE_FAST),
            capable_model: Some(opencode_models::OPENCODE_CAPABLE),
        };
        assert_eq!(
            map_model(&p, "deepseek").as_deref(),
            Some("opencode/deepseek-v4-pro")
        );
        let args = build_args("kilo", "summarize", None);
        assert_eq!(args[0], "run");
    }
}
