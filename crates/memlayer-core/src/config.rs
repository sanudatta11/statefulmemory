//! Daemon-wide configuration loaded from env vars and an optional JSON file.
//!
//! Resolution order (PRD §13):
//! 1. Environment variables (highest priority).
//! 2. `~/.memlayer/config.json` (if present).
//! 3. Built-in defaults.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths;

/// All tunables resolved at daemon-start time.
///
/// Field defaults match PRD §13.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub data_dir: PathBuf,
    /// `tracing-subscriber` env-filter syntax. e.g., `info`, `memlayer=debug`.
    pub log_level: String,
    /// Dedupe window for the normalized-hash content match (PRD §5.8).
    /// `0` disables hash-based dedupe entirely.
    pub dedupe_window: Duration,
    /// Listen target. `None` = UDS at `~/.memlayer/daemon.sock`.
    /// `Some("tcp://host:port")` = TCP+TLS+token (FR2.2).
    pub listen: Option<String>,
    /// PEM cert path. Required when `listen` is TCP.
    pub tls_cert_path: Option<PathBuf>,
    /// PEM private-key path. Required when `listen` is TCP.
    pub tls_key_path: Option<PathBuf>,
    /// Maximum content length (chars) for `SaveObservation` (EC-2).
    pub max_content_chars: usize,
    /// Disk-full threshold in bytes (PRD §5.7).
    pub disk_full_threshold: u64,
    /// Disk-monitor poll interval.
    pub disk_poll_interval: Duration,
    /// Per-project connection LRU capacity (PRD §15.2 / §11.2).
    pub project_lru_capacity: usize,
    /// Read-pool max concurrency per project.
    pub read_pool_size: usize,
    /// Write-thread batch ceiling (rows).
    pub write_batch_max: usize,
    /// Write-thread batch ceiling (time).
    pub write_batch_window: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: paths::data_dir(),
            log_level: "debug".into(),
            dedupe_window: Duration::from_secs(60 * 60 * 24 * 30), // 30 days
            listen: None,
            tls_cert_path: None,
            tls_key_path: None,
            max_content_chars: 50_000,
            disk_full_threshold: 50 * 1024 * 1024, // 50 MB
            disk_poll_interval: Duration::from_secs(30),
            project_lru_capacity: 256,
            read_pool_size: 4,
            write_batch_max: 32,
            write_batch_window: Duration::from_millis(5),
        }
    }
}

/// Subset of `Config` deserialized from `~/.memlayer/config.json`. All fields optional.
#[derive(Debug, Default, Deserialize)]
struct FileOverrides {
    data_dir: Option<PathBuf>,
    log_level: Option<String>,
    dedupe_window_seconds: Option<u64>,
    listen: Option<String>,
    tls_cert_path: Option<PathBuf>,
    tls_key_path: Option<PathBuf>,
    max_content_chars: Option<usize>,
    disk_full_threshold_bytes: Option<u64>,
    disk_poll_interval_seconds: Option<u64>,
    project_lru_capacity: Option<usize>,
    read_pool_size: Option<usize>,
}

impl Config {
    /// Load config: defaults → file → env, last writer wins.
    pub fn load() -> Result<Self> {
        let mut cfg = Self::default();

        // File-level overrides (optional).
        let file_path = cfg.data_dir.join("config.json");
        if file_path.exists() {
            let bytes = std::fs::read(&file_path)?;
            let f: FileOverrides = serde_json::from_slice(&bytes)?;
            if let Some(v) = f.data_dir {
                cfg.data_dir = v;
            }
            if let Some(v) = f.log_level {
                cfg.log_level = v;
            }
            if let Some(v) = f.dedupe_window_seconds {
                cfg.dedupe_window = Duration::from_secs(v);
            }
            if let Some(v) = f.listen {
                cfg.listen = Some(v);
            }
            if let Some(v) = f.tls_cert_path {
                cfg.tls_cert_path = Some(v);
            }
            if let Some(v) = f.tls_key_path {
                cfg.tls_key_path = Some(v);
            }
            if let Some(v) = f.max_content_chars {
                cfg.max_content_chars = v;
            }
            if let Some(v) = f.disk_full_threshold_bytes {
                cfg.disk_full_threshold = v;
            }
            if let Some(v) = f.disk_poll_interval_seconds {
                cfg.disk_poll_interval = Duration::from_secs(v);
            }
            if let Some(v) = f.project_lru_capacity {
                cfg.project_lru_capacity = v;
            }
            if let Some(v) = f.read_pool_size {
                cfg.read_pool_size = v;
            }
        }

        // Environment overrides (PRD §13.2).
        if let Ok(v) = std::env::var("MEMLAYER_DATA_DIR") {
            cfg.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("MEMLAYER_LOG") {
            cfg.log_level = v;
        }
        if let Ok(v) = std::env::var("MEMLAYER_DEDUPE_WINDOW") {
            let secs: u64 = v
                .parse()
                .map_err(|e| Error::invalid(format!("MEMLAYER_DEDUPE_WINDOW: {e}")))?;
            cfg.dedupe_window = Duration::from_secs(secs);
        }
        if let Ok(v) = std::env::var("MEMLAYER_LISTEN") {
            cfg.listen = Some(v);
        }
        if let Ok(v) = std::env::var("MEMLAYER_TLS_CERT") {
            cfg.tls_cert_path = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("MEMLAYER_TLS_KEY") {
            cfg.tls_key_path = Some(PathBuf::from(v));
        }

        cfg.validate()?;
        Ok(cfg)
    }

    /// Returns `true` if the daemon should bind TCP rather than UDS.
    pub fn is_tcp_mode(&self) -> bool {
        self.listen.as_deref().is_some_and(|s| s.starts_with("tcp://"))
    }

    fn validate(&self) -> Result<()> {
        if self.is_tcp_mode()
            && (self.tls_cert_path.is_none() || self.tls_key_path.is_none())
        {
            return Err(Error::FailedPrecondition(
                "TCP mode requires both MEMLAYER_TLS_CERT and MEMLAYER_TLS_KEY".into(),
            ));
        }
        if self.project_lru_capacity == 0 {
            return Err(Error::invalid("project_lru_capacity must be > 0"));
        }
        if self.read_pool_size == 0 {
            return Err(Error::invalid("read_pool_size must be > 0"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.max_content_chars, 50_000);
        assert_eq!(c.project_lru_capacity, 256);
        assert!(!c.is_tcp_mode());
    }

    #[test]
    fn tcp_mode_requires_tls() {
        let c = Config {
            listen: Some("tcp://0.0.0.0:9090".into()),
            ..Config::default()
        };
        assert!(c.validate().is_err());
        let c2 = Config {
            listen: Some("tcp://0.0.0.0:9090".into()),
            tls_cert_path: Some(PathBuf::from("/x")),
            tls_key_path: Some(PathBuf::from("/y")),
            ..Config::default()
        };
        assert!(c2.validate().is_ok());
    }
}

// -----------------------------------------------------------------------------
// MemlayerConfig — runtime tunables for the retrieval pipeline (SC-6).
//
// Loaded with a 3-level precedence:
//   env vars > per-project TOML > global TOML > built-in defaults
//
// Files:
//   - global:      ~/.memlayer/config.toml
//   - per-project: ~/.memlayer/projects/<name>.config.toml
//
// Per-key overlay (not whole-file replace): a per-project file that only
// sets `[rerank]` does NOT erase global's `[extract]` settings. Implemented
// by parsing each layer to `toml::Value` and deep-merging tables.
//
// Distinct from the daemon `Config` (config.json) above: this config
// governs the embed/extract/rerank workers, not daemon-bind behavior.
// -----------------------------------------------------------------------------

/// Tunables for the retrieval-promotion pipeline (embed worker, extract
/// worker, optional reranker). Resolved at every save and at every query.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct MemlayerConfig {
    pub extract: ExtractConfig,
    pub rerank: RerankConfig,
    pub embed: EmbedConfig,
    pub conflict: ConflictConfig,
    pub search: SearchConfig,
    pub verify: VerifyConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ExtractConfig {
    /// Off by default — opt-in only.
    pub enabled: bool,
    pub model: ModelKind,
    pub timeout_secs: u64,
    pub workers: usize,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: ModelKind::Haiku,
            timeout_secs: 30,
            workers: 1,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct RerankConfig {
    pub model: ModelKind,
    pub timeout_secs: u64,
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            model: ModelKind::Haiku,
            timeout_secs: 5,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct EmbedConfig {
    pub workers: usize,
    /// When true, embeddings are stored as int8 BLOBs + per-row scale
    /// (384 bytes vs 1,536 bytes per obs). Dense search falls back to
    /// brute-force cosine in Rust rather than the vec0 vtable.
    pub quantize: bool,
}

impl Default for EmbedConfig {
    fn default() -> Self {
        Self { workers: 2, quantize: false }
    }
}

/// Config for the LLM-based conflict / supersession classifier (Spec 4).
/// On by default; disable with `memlayer config set conflict.enabled false`.
/// If the judge errors, the FTS5 title-match heuristic still runs.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ConflictConfig {
    /// When false the LLM judge is never called; the existing FTS5 heuristic
    /// runs unconditionally.
    pub enabled: bool,
    /// Which model *role* to use (`fast` / `capable`; `haiku` / `sonnet` still parse).
    pub model: ModelKind,
    /// Hard timeout per LLM call. On timeout the heuristic wins.
    pub timeout_secs: u64,
}

impl Default for ConflictConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: ModelKind::Haiku,
            timeout_secs: 5,
        }
    }
}

/// Default retrieval mode for search/context when the caller omits `mode`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SearchConfig {
    /// `"hybrid"` (default) or `"bm25"`.
    pub mode: String,
    /// When true, search/context use the configured rerank model if the
    /// request omits an explicit `--rerank` / wire `rerank` field.
    pub rerank: bool,
    /// Time-decay lambda for hybrid scores: `exp(-lambda * age_days)`.
    /// `0.0` (default) disables decay. Eval harness uses `0.005`.
    pub decay_lambda: f64,
    /// When > 0, `context` expands each hit with ±N same-session neighbors.
    pub evidence_window: u32,
    /// Cap hits per `observations.type` (0 = unlimited). Round-robins types
    /// so one noisy type cannot crowd out the rest.
    pub max_per_type: u32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            mode: "hybrid".into(),
            rerank: false,
            decay_lambda: 0.0,
            evidence_window: 0,
            max_per_type: 0,
        }
    }
}

/// Anchor verification / stale withdrawal (code-anchored memory).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct VerifyConfig {
    /// When false (default), `context` excludes stale / invalidated /
    /// unprovable observations. Search still returns them flagged.
    /// Unanchored observations are never filtered.
    pub serve_stale: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    #[default]
    Haiku,
    Sonnet,
}

impl ModelKind {
    /// Role passed to the agent CLI (`fast` / `capable`), not a vendor id.
    pub fn cli_model_id(&self) -> &'static str {
        match self {
            ModelKind::Haiku => "fast",
            ModelKind::Sonnet => "capable",
        }
    }

    pub fn as_lowercase(&self) -> &'static str {
        match self {
            ModelKind::Haiku => "haiku",
            ModelKind::Sonnet => "sonnet",
        }
    }
}

/// Parse a model role. `haiku`/`sonnet` remain as aliases for `fast`/`capable`.
fn parse_model(s: &str) -> Option<ModelKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "haiku" | "fast" | "flash" | "mini" | "small" => Some(ModelKind::Haiku),
        "sonnet" | "capable" | "pro" | "large" => Some(ModelKind::Sonnet),
        _ => None,
    }
}

pub fn memlayer_global_config_path() -> PathBuf {
    paths::data_dir().join("config.toml")
}

pub fn memlayer_project_config_path(project: &str) -> PathBuf {
    paths::data_dir()
        .join("projects")
        .join(format!("{project}.config.toml"))
}

/// Resolve `MemlayerConfig` per the 3-level precedence (SC-6).
///
/// `project_name` is `None` when the daemon resolves at startup before any
/// save context is known; the daemon re-resolves with the project name at
/// save time so per-project overrides take effect for that observation.
pub fn load_resolved(project_name: Option<&str>) -> MemlayerConfig {
    // Layer 1: global TOML.
    let mut merged = toml::Value::Table(toml::value::Table::new());
    let global_path = memlayer_global_config_path();
    if global_path.exists() {
        match std::fs::read_to_string(&global_path) {
            Ok(text) => match toml::from_str::<toml::Value>(&text) {
                Ok(v) => merged = merge_toml_values(merged, v),
                Err(e) => tracing::warn!(
                    path = %global_path.display(),
                    error = %e,
                    "global memlayer config parse error — using defaults",
                ),
            },
            Err(e) => tracing::warn!(
                path = %global_path.display(),
                error = %e,
                "could not read global memlayer config",
            ),
        }
    }

    // Layer 2: per-project TOML overlay. Errors fall back to global only.
    if let Some(name) = project_name {
        let project_path = memlayer_project_config_path(name);
        if project_path.exists() {
            match std::fs::read_to_string(&project_path) {
                Ok(text) => match toml::from_str::<toml::Value>(&text) {
                    Ok(v) => merged = merge_toml_values(merged, v),
                    Err(e) => tracing::error!(
                        path = %project_path.display(),
                        error = %e,
                        "per-project memlayer config invalid — falling back to global",
                    ),
                },
                Err(e) => tracing::warn!(
                    path = %project_path.display(),
                    error = %e,
                    "could not read per-project memlayer config",
                ),
            }
        }
    }

    // Deserialize the merged value, ignoring unknown keys (forward-compat).
    let mut cfg: MemlayerConfig = match merged.try_into::<MemlayerConfig>() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "merged memlayer config deserialize failed — using defaults");
            MemlayerConfig::default()
        }
    };

    // Layer 3: env vars (highest).
    apply_memlayer_env_overrides(&mut cfg);
    cfg
}

/// Deep-merge TOML tables. Right wins for non-table values; tables merge
/// recursively so a per-project file that only sets `[rerank]` keeps the
/// global file's `[extract]` table intact.
pub fn merge_toml_values(a: toml::Value, b: toml::Value) -> toml::Value {
    use toml::Value;
    match (a, b) {
        (Value::Table(mut at), Value::Table(bt)) => {
            for (k, v) in bt {
                let merged = match at.remove(&k) {
                    Some(av) => merge_toml_values(av, v),
                    None => v,
                };
                at.insert(k, merged);
            }
            Value::Table(at)
        }
        (_, b) => b,
    }
}

fn apply_memlayer_env_overrides(cfg: &mut MemlayerConfig) {
    if let Ok(v) = std::env::var("MEMLAYER_EXTRACT_ENABLED") {
        cfg.extract.enabled = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("MEMLAYER_EXTRACT_MODEL") {
        if let Some(m) = parse_model(&v) {
            cfg.extract.model = m;
        } else {
            tracing::warn!(value = %v, "ignoring MEMLAYER_EXTRACT_MODEL: must be haiku or sonnet");
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_EXTRACT_TIMEOUT_SECS") {
        if let Ok(n) = v.parse() {
            cfg.extract.timeout_secs = n;
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_EXTRACT_WORKERS") {
        if let Ok(n) = v.parse() {
            cfg.extract.workers = n;
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_RERANK_MODEL") {
        if let Some(m) = parse_model(&v) {
            cfg.rerank.model = m;
        } else {
            tracing::warn!(value = %v, "ignoring MEMLAYER_RERANK_MODEL: must be haiku or sonnet");
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_RERANK_TIMEOUT_SECS") {
        if let Ok(n) = v.parse() {
            cfg.rerank.timeout_secs = n;
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_EMBED_WORKERS") {
        if let Ok(n) = v.parse() {
            cfg.embed.workers = n;
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_EMBED_QUANTIZE") {
        cfg.embed.quantize = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("MEMLAYER_CONFLICT_ENABLED") {
        cfg.conflict.enabled = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("MEMLAYER_CONFLICT_MODEL") {
        if let Some(m) = parse_model(&v) {
            cfg.conflict.model = m;
        } else {
            tracing::warn!(value = %v, "ignoring MEMLAYER_CONFLICT_MODEL: must be haiku or sonnet");
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_CONFLICT_TIMEOUT_SECS") {
        if let Ok(n) = v.parse() {
            cfg.conflict.timeout_secs = n;
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_SEARCH_MODE") {
        let m = v.trim().to_ascii_lowercase();
        if m == "hybrid" || m == "bm25" {
            cfg.search.mode = m;
        } else {
            tracing::warn!(value = %v, "ignoring MEMLAYER_SEARCH_MODE: must be hybrid or bm25");
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_VERIFY_SERVE_STALE") {
        cfg.verify.serve_stale = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("MEMLAYER_SEARCH_DECAY_LAMBDA") {
        if let Ok(n) = v.parse::<f64>() {
            cfg.search.decay_lambda = n;
        } else {
            tracing::warn!(value = %v, "ignoring MEMLAYER_SEARCH_DECAY_LAMBDA: not a float");
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_SEARCH_EVIDENCE_WINDOW") {
        if let Ok(n) = v.parse::<u32>() {
            cfg.search.evidence_window = n;
        } else {
            tracing::warn!(value = %v, "ignoring MEMLAYER_SEARCH_EVIDENCE_WINDOW: not an integer");
        }
    }
    if let Ok(v) = std::env::var("MEMLAYER_SEARCH_MAX_PER_TYPE") {
        if let Ok(n) = v.parse::<u32>() {
            cfg.search.max_per_type = n;
        } else {
            tracing::warn!(value = %v, "ignoring MEMLAYER_SEARCH_MAX_PER_TYPE: not an integer");
        }
    }
}

fn parse_bool_env(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Result of [`ensure_config_toml`]: create or deep-merge missing keys only.
#[derive(Debug, Clone)]
pub struct BootstrapReport {
    pub created: bool,
    pub merged_keys: Vec<String>,
    pub backend: String,
    pub search_mode: String,
    pub extract: bool,
    pub conflict: bool,
}

const INSTALL_DEFAULTS: &str = r#"
[search]
mode = "hybrid"
rerank = false

[extract]
enabled = true
model = "fast"
timeout_secs = 30
workers = 1

[conflict]
enabled = true
model = "fast"
timeout_secs = 5

[storage]
backend = "sqlite"
"#;

/// Create `dir/config.toml` or fill in missing keys. Existing values win.
pub fn ensure_config_toml(memlayer_dir: &std::path::Path) -> Result<BootstrapReport> {
    std::fs::create_dir_all(memlayer_dir)?;
    let path = memlayer_dir.join("config.toml");
    let defaults: toml::Value = toml::from_str(INSTALL_DEFAULTS)
        .map_err(|e| Error::internal(format!("install defaults: {e}")))?;
    let (existing, created) = if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        let v: toml::Value = toml::from_str(&text)
            .map_err(|e| Error::invalid(format!("parse {}: {e}", path.display())))?;
        (v, false)
    } else {
        (toml::Value::Table(toml::map::Map::new()), true)
    };
    let mut merged_keys = Vec::new();
    collect_missing_keys(&defaults, &existing, "", &mut merged_keys);
    // Existing keys win over install defaults.
    let merged = merge_toml_values(defaults, existing);
    let serialized = toml::to_string_pretty(&merged)
        .map_err(|e| Error::internal(format!("serialize config.toml: {e}")))?;
    std::fs::write(&path, serialized)?;

    let search_mode = merged
        .get("search")
        .and_then(|t| t.get("mode"))
        .and_then(|v| v.as_str())
        .unwrap_or("hybrid")
        .to_string();
    let extract = merged
        .get("extract")
        .and_then(|t| t.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let conflict = merged
        .get("conflict")
        .and_then(|t| t.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let backend = merged
        .get("storage")
        .and_then(|t| t.get("backend"))
        .and_then(|v| v.as_str())
        .unwrap_or("sqlite")
        .to_string();
    Ok(BootstrapReport {
        created,
        merged_keys,
        backend,
        search_mode,
        extract,
        conflict,
    })
}

fn collect_missing_keys(
    defaults: &toml::Value,
    existing: &toml::Value,
    prefix: &str,
    out: &mut Vec<String>,
) {
    let (Some(dt), Some(et)) = (defaults.as_table(), existing.as_table()) else {
        return;
    };
    for (k, dv) in dt {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match et.get(k) {
            None => out.push(path),
            Some(ev) if dv.is_table() && ev.is_table() => {
                collect_missing_keys(dv, ev, &path, out);
            }
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod memlayer_config_tests {
    use super::*;
    /// `MEMLAYER_*` env vars + `MEMLAYER_DATA_DIR` are process-global.
    /// Cargo runs tests in parallel by default; serialize through the crate
    /// lock so tests don't race on the env or the temp data dir.
    #[allow(unused_imports)]
    use crate::TEST_ENV_LOCK;
    fn clear_env() {
        for k in [
            "MEMLAYER_EXTRACT_ENABLED",
            "MEMLAYER_EXTRACT_MODEL",
            "MEMLAYER_EXTRACT_TIMEOUT_SECS",
            "MEMLAYER_EXTRACT_WORKERS",
            "MEMLAYER_RERANK_MODEL",
            "MEMLAYER_RERANK_TIMEOUT_SECS",
            "MEMLAYER_EMBED_WORKERS",
            "MEMLAYER_SEARCH_MODE",
        ] {
            std::env::remove_var(k);
        }
    }

    fn fresh_data_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("projects")).unwrap();
        std::env::set_var("MEMLAYER_DATA_DIR", dir.path());
        dir
    }

    #[test]
    fn default_extract_disabled_haiku() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let _d = fresh_data_dir();
        let cfg = load_resolved(None);
        assert!(!cfg.extract.enabled);
        assert_eq!(cfg.extract.model, ModelKind::Haiku);
        assert_eq!(cfg.rerank.model, ModelKind::Haiku);
        assert_eq!(cfg.embed.workers, 2);
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn precedence_env_overrides_project_overrides_global_overrides_default() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_data_dir();

        // Global: enable extract with haiku.
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[extract]
enabled = true
model = "fast"
timeout_secs = 30
"#,
        )
        .unwrap();

        // Per-project: override model to sonnet.
        std::fs::write(
            dir.path().join("projects").join("myrepo.config.toml"),
            r#"
[extract]
model = "sonnet"
"#,
        )
        .unwrap();

        // No env: project beats global.
        let cfg = load_resolved(Some("myrepo"));
        assert!(cfg.extract.enabled, "global enabled should still apply");
        assert_eq!(cfg.extract.model, ModelKind::Sonnet, "project model wins");
        assert_eq!(cfg.extract.timeout_secs, 30, "global timeout preserved");

        // Env: should override project.
        std::env::set_var("MEMLAYER_EXTRACT_MODEL", "haiku");
        let cfg = load_resolved(Some("myrepo"));
        assert_eq!(cfg.extract.model, ModelKind::Haiku, "env wins over project");

        // Other-project: only global applies.
        let cfg_other = load_resolved(Some("other"));
        assert_eq!(
            cfg_other.extract.model,
            ModelKind::Haiku,
            "env applies regardless of project_name"
        );

        clear_env();
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn invalid_per_project_falls_back_to_global() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_data_dir();

        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[extract]
enabled = true
model = "sonnet"
"#,
        )
        .unwrap();

        std::fs::write(
            dir.path().join("projects").join("myrepo.config.toml"),
            "this is not = valid TOML [[[",
        )
        .unwrap();

        let cfg = load_resolved(Some("myrepo"));
        assert!(cfg.extract.enabled);
        assert_eq!(cfg.extract.model, ModelKind::Sonnet, "global must apply");
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn unknown_keys_warn_but_dont_fail() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_data_dir();

        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[extract]
enabled = true
flux_capacitor = "1.21 GW"

[brand_new_section]
hello = "world"
"#,
        )
        .unwrap();

        let cfg = load_resolved(None);
        assert!(cfg.extract.enabled, "known keys still parse");
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn project_overlay_preserves_global_section() {
        // Sanity: per-project file with only [rerank] must not erase [extract].
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_data_dir();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[extract]
enabled = true
model = "sonnet"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("projects").join("myrepo.config.toml"),
            r#"
[rerank]
model = "sonnet"
"#,
        )
        .unwrap();
        let cfg = load_resolved(Some("myrepo"));
        assert!(cfg.extract.enabled);
        assert_eq!(cfg.extract.model, ModelKind::Sonnet);
        assert_eq!(cfg.rerank.model, ModelKind::Sonnet);
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn search_config_default_is_hybrid() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let c = MemlayerConfig::default();
        assert_eq!(c.search.mode, "hybrid");
        let _d = fresh_data_dir();
        let cfg = load_resolved(None);
        assert_eq!(cfg.search.mode, "hybrid");
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn conflict_config_default_is_enabled() {
        let c = MemlayerConfig::default();
        assert!(c.conflict.enabled);
    }

    #[test]
    fn bootstrap_creates_file_with_hybrid_and_extract() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".memlayer");
        let r = ensure_config_toml(&cfg_dir).unwrap();
        assert!(r.created);
        let raw = std::fs::read_to_string(cfg_dir.join("config.toml")).unwrap();
        assert!(raw.contains("mode = \"hybrid\""));
        assert!(raw.contains("enabled = true"));
        assert!(r.extract);
        assert!(r.conflict);
        assert_eq!(r.backend, "sqlite");
    }

    #[test]
    fn bootstrap_does_not_clobber_search_mode() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_dir = dir.path().join(".memlayer");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("config.toml"), "[search]\nmode = \"bm25\"\n").unwrap();
        let r = ensure_config_toml(&cfg_dir).unwrap();
        assert!(!r.created);
        let v: toml::Value =
            toml::from_str(&std::fs::read_to_string(cfg_dir.join("config.toml")).unwrap()).unwrap();
        assert_eq!(v["search"]["mode"].as_str(), Some("bm25"));
        assert_eq!(v["conflict"]["enabled"].as_bool(), Some(true));
        assert_eq!(v["extract"]["enabled"].as_bool(), Some(true));
    }

    #[test]
    fn model_roles_map_to_fast_capable_not_vendor_ids() {
        assert_eq!(parse_model("fast"), Some(ModelKind::Haiku));
        assert_eq!(parse_model("capable"), Some(ModelKind::Sonnet));
        assert_eq!(parse_model("flash"), Some(ModelKind::Haiku));
        assert_eq!(parse_model("pro"), Some(ModelKind::Sonnet));
        assert_eq!(ModelKind::Haiku.cli_model_id(), "fast");
        assert_eq!(ModelKind::Sonnet.cli_model_id(), "capable");
    }
}
