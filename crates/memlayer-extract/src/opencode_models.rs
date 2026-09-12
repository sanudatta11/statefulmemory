//! OpenCode `--model` ids (`provider/model`).
//!
//! OpenCode accepts 75+ providers via Models.dev; Zen is the curated gateway
//! (`opencode/<id>`). Bare nicknames (`glm`, `qwen`, `deepseek`, `kimi`) and
//! Zen ids without a prefix are expanded here so `opencode run -m` gets a
//! real id instead of a Claude-only name.

/// Documented defaults if a caller *does* pin a role (MCP/CLI inherit instead).
pub const OPENCODE_FAST: &str = "opencode/glm-5.3-flash";
pub const OPENCODE_CAPABLE: &str = "opencode/glm-5.3";

/// Zen catalog (live `GET https://opencode.ai/zen/v1/models` plus docs-listed
/// Qwen 3.7). Must stay ≥ 50; tests assert the count.
pub const OPENCODE_ZEN_IDS: &[&str] = &[
    // Claude
    "claude-fable-5",
    "claude-fable-5-1",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-opus-4-5",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-sonnet-4-5",
    "claude-sonnet-4",
    "claude-haiku-4-5",
    // Gemini
    "gemini-3.8-flash",
    "gemini-3.7-flash",
    "gemini-3.6-flash",
    "gemini-3.5-flash",
    "gemini-3.5-flash-lite",
    "gemini-3.1-pro",
    "gemini-3-flash",
    // GPT
    "gpt-6-astra",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.5-pro",
    "gpt-5.4",
    "gpt-5.4-pro",
    "gpt-5.4-mini",
    "gpt-5.4-nano",
    "gpt-5.3-codex",
    "gpt-5.3-codex-spark",
    "gpt-5.2",
    "gpt-5.2-codex",
    "gpt-5.1",
    "gpt-5.1-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex-mini",
    "gpt-5",
    "gpt-5-codex",
    "gpt-5-nano",
    // xAI / Muse
    "grok-4.6",
    "grok-4.5",
    "grok-build-0.1",
    "muse-spark-1.3",
    "muse-spark-1.2",
    // Chinese OSS + peers
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.6-plus",
    "qwen3.5-plus",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "deepseek-v4-flash-vision-exp",
    "deepseek-v4-flash-free",
    "glm-5.3-flash",
    "glm-5.3",
    "glm-5.2",
    "glm-5.1",
    "glm-5",
    "minimax-m3",
    "minimax-m2.7",
    "minimax-m2.5",
    "kimi-k3",
    "kimi-k2.7-code",
    "kimi-k2.6",
    "kimi-k2.5",
    "big-pickle",
    "mimo-v2.5-free",
    "ling-3.0-flash-fin-free",
    "nemotron-3-ultra-free",
    "nemotron-3.5-lightning-free",
    "muse-spark-1.3-contributor-free",
    "muse-spark-1.2-contributor-free",
];

fn zen_id(bare: &str) -> Option<&'static str> {
    OPENCODE_ZEN_IDS
        .iter()
        .copied()
        .find(|id| id.eq_ignore_ascii_case(bare))
}

/// Nickname → Zen id (no `opencode/` prefix).
fn alias_zen(bare: &str) -> Option<&'static str> {
    Some(match bare {
        "haiku" | "claude-haiku" | "claude-haiku-4.5" => "claude-haiku-4-5",
        "sonnet" | "claude-sonnet" | "claude-sonnet-4.6" => "claude-sonnet-4-6",
        "opus" | "claude-opus" => "claude-opus-5",
        "fable" | "claude-fable" => "claude-fable-5-1",
        "qwen" | "qwen3" | "qwen3-plus" | "qwen-plus" | "qwen3-coder" | "qwen3-coder-plus" => {
            "qwen3.7-plus"
        }
        "qwen-max" | "qwen3-max" | "qwen3.7-max" => "qwen3.7-max",
        "qwen3.5" | "qwen3.5-plus" => "qwen3.5-plus",
        "qwen3.6" | "qwen3.6-plus" => "qwen3.6-plus",
        "glm" | "glm5" | "glm-5" | "chatglm" | "zhipu" | "zai" => "glm-5.3",
        "glm-flash" | "glm5-flash" | "glm-5-flash" => "glm-5.3-flash",
        "glm-5.3" => "glm-5.3",
        "glm-5.2" => "glm-5.2",
        "deepseek" | "ds" | "deepseek-v4" | "deepseek-pro" | "deepseek-chat" => "deepseek-v4-pro",
        "deepseek-flash" | "ds-flash" => "deepseek-v4-flash",
        "kimi" | "k2" | "moonshot" | "kimi-k2" | "kimi-code" => "kimi-k2.7-code",
        "kimi-k3" | "k3" => "kimi-k3",
        "minimax" | "m3" | "minimax-m3" => "minimax-m3",
        "minimax-m2" | "m2.7" => "minimax-m2.7",
        "grok" | "grok-4" | "xai" => "grok-4.6",
        "gemini" | "gemini-flash" | "gemini-3-flash" => "gemini-3.8-flash",
        "gemini-pro" | "gemini-3-pro" => "gemini-3.1-pro",
        "gpt" | "gpt-5" | "gpt5" => "gpt-5.5",
        "gpt-codex" | "codex" => "gpt-5.3-codex",
        "luna" => "gpt-5.6-luna",
        "nano" => "gpt-5.4-nano",
        "muse" | "spark" => "muse-spark-1.3",
        "mimo" => "mimo-v2.5-free",
        _ => return None,
    })
}

fn strip_opencode_prefix(s: &str) -> &str {
    s.strip_prefix("opencode/")
        .or_else(|| s.strip_prefix("opencodezen/"))
        .unwrap_or(s)
}

/// Resolve a role, nickname, Zen id, or `provider/model` for OpenCode/Kilo.
///
/// `None` means omit `--model` (agent default).
pub fn resolve(requested: &str) -> Option<String> {
    let r = requested.trim();
    if r.is_empty() {
        return None;
    }
    let lower = r.to_ascii_lowercase().replace('_', "-");
    match lower.as_str() {
        "auto" | "default" | "agent" | "current" | "fast" | "flash" | "mini" | "small"
        | "capable" | "pro" | "large" | "haiku" | "sonnet" => return None,
        _ => {}
    }

    let rest = strip_opencode_prefix(&lower);

    if let Some(id) = zen_id(rest) {
        return Some(format!("opencode/{id}"));
    }
    if let Some(id) = alias_zen(rest) {
        return Some(format!("opencode/{id}"));
    }

    // Already a provider/model (anthropic/…, openai/…, zhipuai/…).
    if rest.contains('/') {
        return Some(r.to_string());
    }

    // Unknown bare id: pass through so custom OpenCode providers still work.
    Some(r.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn zen_catalog_covers_at_least_50() {
        assert!(
            OPENCODE_ZEN_IDS.len() >= 50,
            "expected ≥50 Zen ids, got {}",
            OPENCODE_ZEN_IDS.len()
        );
        let set: HashSet<_> = OPENCODE_ZEN_IDS.iter().copied().collect();
        assert_eq!(set.len(), OPENCODE_ZEN_IDS.len(), "duplicate Zen ids");
    }

    #[test]
    fn chinese_oss_ids_are_in_catalog() {
        for id in [
            "qwen3.7-plus",
            "qwen3.5-plus",
            "glm-5.3",
            "glm-5.3-flash",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "kimi-k2.7-code",
            "minimax-m3",
        ] {
            assert!(OPENCODE_ZEN_IDS.contains(&id), "missing {id}");
        }
    }

    #[test]
    fn nicknames_expand_to_opencode_prefix() {
        assert_eq!(resolve("glm").as_deref(), Some("opencode/glm-5.3"));
        assert_eq!(resolve("qwen").as_deref(), Some("opencode/qwen3.7-plus"));
        assert_eq!(
            resolve("deepseek").as_deref(),
            Some("opencode/deepseek-v4-pro")
        );
        assert_eq!(resolve("kimi").as_deref(), Some("opencode/kimi-k2.7-code"));
        assert_eq!(resolve("haiku"), None);
        assert_eq!(
            resolve("glm-5.3-flash").as_deref(),
            Some("opencode/glm-5.3-flash")
        );
        assert_eq!(
            resolve("opencode/gpt-5.5").as_deref(),
            Some("opencode/gpt-5.5")
        );
    }

    #[test]
    fn roles_inherit_agent_current_model() {
        assert_eq!(resolve("fast"), None);
        assert_eq!(resolve("capable"), None);
        assert_eq!(resolve("haiku"), None);
        assert_eq!(resolve("sonnet"), None);
    }

    #[test]
    fn foreign_provider_slash_ids_pass_through() {
        assert_eq!(
            resolve("anthropic/claude-sonnet-4-6").as_deref(),
            Some("anthropic/claude-sonnet-4-6")
        );
        assert_eq!(
            resolve("openai/gpt-4.1").as_deref(),
            Some("openai/gpt-4.1")
        );
    }
}
