//! `statefulmemory config` — view and edit the retrieval-pipeline TOML files.
//!
//! Two files are managed:
//! * `~/.statefulmemory/config.toml` (global)
//! * `~/.statefulmemory/projects/<name>.config.toml` (per-project, optional)
//!
//! Verbs:
//! * `config show [--raw] [--project <name>]` — resolved view (default) or
//!   raw global TOML.
//! * `config get <key> [--project <name>]` — print one resolved value.
//! * `config set <key> <value> [--project <name>]` — atomic write.
//!
//! Spec: retrieval-promotion P11. Tasks: rp-t11.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use statefulmemory_core::config::{
    self, statefulmemory_global_config_path, statefulmemory_project_config_path,
    StatefulMemoryConfig,
};

use crate::cli::{ConfigGetArgs, ConfigSetArgs, ConfigShowArgs, ConfigVerb};
use crate::exit;

pub async fn dispatch(verb: ConfigVerb) -> ExitCode {
    let result = match verb {
        ConfigVerb::Show(a) => show(a),
        ConfigVerb::Get(a) => get(a),
        ConfigVerb::Set(a) => set(a),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("statefulmemory: {e}");
            ExitCode::from(exit::USAGE)
        }
    }
}

fn show(a: ConfigShowArgs) -> Result<(), String> {
    let stdout = io::stdout();
    let mut h = stdout.lock();

    if a.raw {
        let path = statefulmemory_global_config_path();
        if path.exists() {
            let text =
                fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
            h.write_all(text.as_bytes()).ok();
            if !text.ends_with('\n') {
                writeln!(h).ok();
            }
        } else {
            writeln!(h, "# (no config.toml at {})", path.display()).ok();
        }
        return Ok(());
    }

    let cfg = config::load_resolved(a.project.as_deref());
    writeln!(h, "# Resolved statefulmemory config").ok();
    if let Some(name) = a.project.as_deref() {
        writeln!(h, "# project: {name}").ok();
    }
    writeln!(
        h,
        "# global file:      {}",
        statefulmemory_global_config_path().display()
    )
    .ok();
    if let Some(name) = a.project.as_deref() {
        writeln!(
            h,
            "# per-project file: {}",
            statefulmemory_project_config_path(name).display(),
        )
        .ok();
    }
    writeln!(h).ok();

    let toml_text =
        toml::to_string_pretty(&cfg).map_err(|e| format!("serialize resolved config: {e}"))?;
    h.write_all(toml_text.as_bytes()).ok();
    Ok(())
}

fn get(a: ConfigGetArgs) -> Result<(), String> {
    let cfg = config::load_resolved(a.project.as_deref());
    let value = lookup(&cfg, &a.key).ok_or_else(|| {
        format!(
            "unknown config key: {}; valid keys: extract.enabled, extract.model, \
             extract.timeout_secs, extract.workers, rerank.model, rerank.timeout_secs, \
             embed.workers, verify.serve_stale, graph.enabled, graph.hops, graph.boost, \
             graph.edge_types, graph.budget_pct, graph.degree_cap, graph.max_query_entities",
            a.key
        )
    })?;
    println!("{value}");
    Ok(())
}

fn set(a: ConfigSetArgs) -> Result<(), String> {
    // Validate the key + value pair *before* touching the file.
    validate_key_value(&a.key, &a.value)?;

    let path = match a.project.as_deref() {
        Some(name) => statefulmemory_project_config_path(name),
        None => statefulmemory_global_config_path(),
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir {}: {e}", parent.display()))?;
    }

    // Read existing TOML (may be empty/missing).
    let existing = match fs::read_to_string(&path) {
        Ok(s) => toml::from_str::<toml::Value>(&s)
            .map_err(|e| format!("parse {}: {e}", path.display()))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            toml::Value::Table(toml::value::Table::new())
        }
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };

    let mut tbl = match existing {
        toml::Value::Table(t) => t,
        other => {
            return Err(format!(
                "{} is not a TOML table at the top level (got {:?})",
                path.display(),
                other.type_str()
            ))
        }
    };

    set_in_table(&mut tbl, &a.key, parse_value(&a.value));

    let new_text = toml::to_string_pretty(&toml::Value::Table(tbl))
        .map_err(|e| format!("serialize {}: {e}", path.display()))?;

    atomic_write(&path, new_text.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))?;

    eprintln!("Wrote {}", path.display());
    Ok(())
}

/// Look up one resolved value. Returns the printable form ("haiku", "true",
/// "30", etc.) or `None` for unknown keys.
fn lookup(cfg: &StatefulMemoryConfig, key: &str) -> Option<String> {
    Some(match key {
        "extract.enabled" => cfg.extract.enabled.to_string(),
        "extract.model" => cfg.extract.model.as_lowercase().to_string(),
        "extract.timeout_secs" => cfg.extract.timeout_secs.to_string(),
        "extract.workers" => cfg.extract.workers.to_string(),
        "rerank.model" => cfg.rerank.model.as_lowercase().to_string(),
        "rerank.timeout_secs" => cfg.rerank.timeout_secs.to_string(),
        "embed.workers" => cfg.embed.workers.to_string(),
        "verify.serve_stale" => cfg.verify.serve_stale.to_string(),
        "graph.enabled" => cfg.graph.enabled.to_string(),
        "graph.hops" => cfg.graph.hops.to_string(),
        "graph.boost" => cfg.graph.boost.to_string(),
        "graph.edge_types" => cfg.graph.edge_types.join(","),
        "graph.budget_pct" => cfg.graph.budget_pct.to_string(),
        "graph.degree_cap" => cfg.graph.degree_cap.to_string(),
        "graph.max_query_entities" => cfg.graph.max_query_entities.to_string(),
        _ => return None,
    })
}

/// Validate key + value combination so `set` rejects garbage before
/// touching disk. Mirrors the schema in [`StatefulMemoryConfig`].
fn validate_key_value(key: &str, value: &str) -> Result<(), String> {
    let v = value.trim();
    match key {
        "extract.enabled" => {
            parse_bool(v)
                .ok_or_else(|| format!("extract.enabled must be true|false (got {value:?})"))?;
        }
        "verify.serve_stale" => {
            parse_bool(v)
                .ok_or_else(|| format!("verify.serve_stale must be true|false (got {value:?})"))?;
        }
        "extract.model" | "rerank.model" | "conflict.model" => {
            if ![
                "haiku", "sonnet", "fast", "capable", "flash", "pro", "mini", "small", "large",
            ]
            .contains(&v.to_ascii_lowercase().as_str())
            {
                return Err(format!(
                    "{key} must be a model role: fast|capable (aliases: haiku|sonnet) (got {value:?})"
                ));
            }
        }
        "extract.timeout_secs" | "rerank.timeout_secs" => {
            v.parse::<u64>()
                .map_err(|_| format!("{key} must be a non-negative integer (got {value:?})"))?;
        }
        "extract.workers" | "embed.workers" => {
            let n: usize = v
                .parse()
                .map_err(|_| format!("{key} must be a non-negative integer (got {value:?})"))?;
            if n == 0 {
                return Err(format!("{key} must be > 0 (got 0)"));
            }
        }
        "graph.enabled" => {
            parse_bool(v)
                .ok_or_else(|| format!("graph.enabled must be true|false (got {value:?})"))?;
        }
        "graph.hops" => {
            let n: u8 = v
                .parse()
                .map_err(|_| format!("graph.hops must be an integer 0..=2 (got {value:?})"))?;
            if n > 2 {
                return Err(format!("graph.hops must be 0..=2 (got {value:?})"));
            }
        }
        "graph.boost" | "graph.budget_pct" => {
            v.parse::<f64>()
                .map_err(|_| format!("{key} must be a float (got {value:?})"))?;
        }
        "graph.degree_cap" | "graph.max_query_entities" => {
            v.parse::<usize>()
                .map_err(|_| format!("{key} must be a non-negative integer (got {value:?})"))?;
        }
        "graph.edge_types" => {
            // Comma-separated: mentions,fixes,contradicts,about,co_occurs
            for t in v.split(',') {
                let t = t.trim();
                if t.is_empty() {
                    return Err(format!(
                        "graph.edge_types must be comma-separated relation names (got {value:?})"
                    ));
                }
            }
        }
        other => {
            return Err(format!(
                "unknown config key: {other}; valid keys: extract.enabled, extract.model, \
                 extract.timeout_secs, extract.workers, rerank.model, rerank.timeout_secs, \
                 embed.workers, verify.serve_stale, graph.enabled, graph.hops, graph.boost, \
                 graph.edge_types, graph.budget_pct, graph.degree_cap, graph.max_query_entities",
            ));
        }
    }
    Ok(())
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_value(v: &str) -> toml::Value {
    if let Some(b) = parse_bool(v) {
        return toml::Value::Boolean(b);
    }
    if let Ok(i) = v.parse::<i64>() {
        return toml::Value::Integer(i);
    }
    if let Ok(f) = v.parse::<f64>() {
        return toml::Value::Float(f);
    }
    toml::Value::String(v.to_string())
}

/// `set_in_table(tbl, "extract.model", v)` walks the dotted path, creating
/// intermediate tables as needed.
fn set_in_table(tbl: &mut toml::value::Table, dotted: &str, value: toml::Value) {
    let parts: Vec<&str> = dotted.split('.').collect();
    if parts.len() == 1 {
        tbl.insert(parts[0].to_string(), value);
        return;
    }
    let head = parts[0];
    let rest = &parts[1..].join(".");
    let entry = tbl
        .entry(head.to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    match entry {
        toml::Value::Table(inner) => set_in_table(inner, rest, value),
        other => {
            // Not a table — replace with one. (Caller already validated key.)
            let mut inner = toml::value::Table::new();
            set_in_table(&mut inner, rest, value);
            *other = toml::Value::Table(inner);
        }
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = tmp_path_for(path);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config.toml");
    parent.join(format!(".{name}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("projects")).unwrap();
        std::env::set_var("STATEFULMEMORY_DATA_DIR", dir.path());
        dir
    }

    fn read_global_config(dir: &tempfile::TempDir) -> toml::Value {
        toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap()
    }

    fn clear_env() {
        for k in [
            "STATEFULMEMORY_EXTRACT_ENABLED",
            "STATEFULMEMORY_EXTRACT_MODEL",
            "STATEFULMEMORY_EXTRACT_TIMEOUT_SECS",
            "STATEFULMEMORY_EXTRACT_WORKERS",
            "STATEFULMEMORY_RERANK_MODEL",
            "STATEFULMEMORY_RERANK_TIMEOUT_SECS",
            "STATEFULMEMORY_EMBED_WORKERS",
            "STATEFULMEMORY_EMBED_QUANTIZE",
            "STATEFULMEMORY_CONFLICT_ENABLED",
            "STATEFULMEMORY_CONFLICT_MODEL",
            "STATEFULMEMORY_CONFLICT_TIMEOUT_SECS",
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn set_get_roundtrip_global() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_dir();

        set(ConfigSetArgs {
            key: "extract.model".into(),
            value: "sonnet".into(),
            project: None,
        })
        .unwrap();
        set(ConfigSetArgs {
            key: "extract.enabled".into(),
            value: "true".into(),
            project: None,
        })
        .unwrap();

        let raw = read_global_config(&dir);
        assert_eq!(raw["extract"]["model"].as_str(), Some("sonnet"));
        assert_eq!(raw["extract"]["enabled"].as_bool(), Some(true));

        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn project_overrides_global_in_show() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let _dir = fresh_dir();

        set(ConfigSetArgs {
            key: "extract.model".into(),
            value: "haiku".into(),
            project: None,
        })
        .unwrap();
        set(ConfigSetArgs {
            key: "extract.model".into(),
            value: "sonnet".into(),
            project: Some("myrepo".into()),
        })
        .unwrap();

        let cfg = config::load_resolved(Some("myrepo"));
        assert_eq!(cfg.extract.model, config::ModelKind::Sonnet);

        let cfg_other = config::load_resolved(Some("other"));
        assert_eq!(cfg_other.extract.model, config::ModelKind::Haiku);

        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn set_creates_missing_file() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_dir();
        let p = dir.path().join("projects").join("brand-new.config.toml");
        assert!(!p.exists());
        set(ConfigSetArgs {
            key: "rerank.model".into(),
            value: "sonnet".into(),
            project: Some("brand-new".into()),
        })
        .unwrap();
        assert!(p.exists(), "file must be created on first set");
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn set_rejects_unknown_key() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let _d = fresh_dir();
        let err = set(ConfigSetArgs {
            key: "extract.flux_capacitor".into(),
            value: "1.21GW".into(),
            project: None,
        })
        .unwrap_err();
        assert!(err.contains("unknown config key"), "msg: {err}");
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn set_rejects_bad_value() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let _d = fresh_dir();
        let err = set(ConfigSetArgs {
            key: "extract.model".into(),
            value: "opus".into(),
            project: None,
        })
        .unwrap_err();
        assert!(
            err.contains("haiku") && err.contains("sonnet"),
            "msg: {err}"
        );
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn set_does_not_clobber_unrelated_sections() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_dir();

        // Global has extract.enabled=true.
        set(ConfigSetArgs {
            key: "extract.enabled".into(),
            value: "true".into(),
            project: None,
        })
        .unwrap();
        // Then change rerank.model. extract section must remain.
        set(ConfigSetArgs {
            key: "rerank.model".into(),
            value: "sonnet".into(),
            project: None,
        })
        .unwrap();

        let raw = read_global_config(&dir);
        assert_eq!(
            raw["extract"]["enabled"].as_bool(),
            Some(true),
            "extract section preserved"
        );
        assert_eq!(raw["rerank"]["model"].as_str(), Some("sonnet"));

        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn get_resolves_known_keys() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let _d = fresh_dir();
        set(ConfigSetArgs {
            key: "extract.workers".into(),
            value: "4".into(),
            project: None,
        })
        .unwrap();
        let cfg = config::load_resolved(None);
        assert_eq!(lookup(&cfg, "extract.workers").as_deref(), Some("4"));
        assert_eq!(
            lookup(&cfg, "extract.model").as_deref(),
            Some("haiku"),
            "default preserved when not set",
        );
        assert!(lookup(&cfg, "extract.flux").is_none());
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }
}
