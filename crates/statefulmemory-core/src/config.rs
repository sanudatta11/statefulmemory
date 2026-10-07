//! Daemon-wide configuration loaded from env vars and an optional JSON file.
//!
//! Resolution order (PRD §13):
//! 1. Environment variables (highest priority).
//! 2. `~/.statefulmemory/config.json` (if present).
//! 3. Built-in defaults.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths;

/// All tunables resolved at daemon-start time.
///
/// Field defaults match PRD §13.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub data_dir: PathBuf,
    /// `tracing-subscriber` env-filter syntax. e.g., `info`, `statefulmemory=debug`.
    pub log_level: String,
    /// Dedupe window for the normalized-hash content match (PRD §5.8).
    /// `0` disables hash-based dedupe entirely.
    pub dedupe_window: Duration,
    /// Listen target. `None` = UDS at `~/.statefulmemory/daemon.sock`.
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

/// Subset of `Config` deserialized from `~/.statefulmemory/config.json`. All fields optional.
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
        if let Ok(v) = std::env::var("STATEFULMEMORY_DATA_DIR") {
            cfg.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("STATEFULMEMORY_LOG") {
            cfg.log_level = v;
        }
        if let Ok(v) = std::env::var("STATEFULMEMORY_DEDUPE_WINDOW") {
            let secs: u64 = v
                .parse()
                .map_err(|e| Error::invalid(format!("STATEFULMEMORY_DEDUPE_WINDOW: {e}")))?;
            cfg.dedupe_window = Duration::from_secs(secs);
        }
        if let Ok(v) = std::env::var("STATEFULMEMORY_LISTEN") {
            cfg.listen = Some(v);
        }
        if let Ok(v) = std::env::var("STATEFULMEMORY_TLS_CERT") {
            cfg.tls_cert_path = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("STATEFULMEMORY_TLS_KEY") {
            cfg.tls_key_path = Some(PathBuf::from(v));
        }

        cfg.validate()?;
        Ok(cfg)
    }

    /// Returns `true` if the daemon should bind TCP rather than UDS.
    pub fn is_tcp_mode(&self) -> bool {
        self.listen
            .as_deref()
            .is_some_and(|s| s.starts_with("tcp://"))
    }

    fn validate(&self) -> Result<()> {
        if self.is_tcp_mode() && (self.tls_cert_path.is_none() || self.tls_key_path.is_none()) {
            return Err(Error::FailedPrecondition(
                "TCP mode requires both STATEFULMEMORY_TLS_CERT and STATEFULMEMORY_TLS_KEY".into(),
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

    #[test]
    fn graph_edge_types_default_excludes_co_occurs() {
        let g = GraphConfig::default();
        assert_eq!(
            g.edge_types,
            vec![
                "mentions".to_string(),
                "fixes".to_string(),
                "contradicts".to_string()
            ],
            "default traversal set is the three hand-curated relations"
        );
        assert!(
            !g.edge_types.iter().any(|t| t == "co_occurs"),
            "co_occurs is export-only (dumps keep it; default query set excludes it)"
        );
        // Toggle fields that gate graph behavior.
        assert!(
            GraphConfig::default().enabled,
            "graph default on (Phase 6.4; save-path indexing is fire-and-forget)"
        );
    }
}

// -----------------------------------------------------------------------------
// StatefulMemoryConfig — runtime tunables for the retrieval pipeline (SC-6).
//
// Loaded with a 3-level precedence:
//   env vars > per-project TOML > global TOML > built-in defaults
//
// Files:
//   - global:      ~/.statefulmemory/config.toml
//   - per-project: ~/.statefulmemory/projects/<name>.config.toml
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
pub struct StatefulMemoryConfig {
    pub extract: ExtractConfig,
    pub rerank: RerankConfig,
    pub embed: EmbedConfig,
    pub conflict: ConflictConfig,
    pub search: SearchConfig,
    pub verify: VerifyConfig,
    pub graph: GraphConfig,
    pub laya: LayaConfig,
    pub update: UpdateConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct UpdateConfig {
    pub enabled: bool,
    pub check_interval_hours: u64,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_hours: 24,
        }
    }
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
    /// `"local"` (default, MiniLM-style feature+embed CE), `"llm"` (agent CLI),
    /// or `"off"`.
    pub backend: String,
    /// Candidate pool for local CE before truncating to request limit.
    pub top_n: u32,
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            model: ModelKind::Haiku,
            timeout_secs: 5,
            backend: "local".into(),
            top_n: 16,
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
    /// When true, content longer than `chunk_words` also gets split into
    /// overlapping word-chunks, each embedded + stored separately
    /// (`observations_chunks` / `observations_vec_chunks`, V15), so dense
    /// search can max-pool chunk similarity for long observations instead of
    /// relying solely on the mean-pooled whole-document vector. Default
    /// false (opt-in; adds extra embed + write work per long observation).
    pub chunk_long_content: bool,
    /// Word-count threshold above which content is chunked (roughly maps to
    /// BGE-small's ~512 token window; words-not-tokens is a cheap proxy).
    pub chunk_words: usize,
    /// Overlap (in words) between consecutive chunks, so a fact split across
    /// a chunk boundary still has a chance to land fully inside one chunk.
    pub chunk_overlap_words: usize,
}

impl Default for EmbedConfig {
    fn default() -> Self {
        Self {
            workers: 2,
            quantize: false,
            chunk_long_content: false,
            chunk_words: 350,
            chunk_overlap_words: 50,
        }
    }
}

/// Config for the LLM-based conflict / supersession classifier (Spec 4).
/// On by default; disable with `statefulmemory config set conflict.enabled false`.
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
    /// When true, search/context use the configured rerank backend if the
    /// request omits an explicit `--rerank` / wire `rerank` field.
    pub rerank: bool,
    /// Time-decay lambda for hybrid scores: `exp(-lambda * age_days)`.
    /// Default `0.005` (half-life ≈138 days), matching the eval-tuned value;
    /// `0.0` disables decay.
    pub decay_lambda: f64,
    /// When > 0, `context` expands each hit with ±N same-session neighbors.
    pub evidence_window: u32,
    /// Cap hits per `observations.type` (0 = unlimited). Round-robins types
    /// so one noisy type cannot crowd out the rest.
    pub max_per_type: u32,
    /// `"adaptive"` (default) routes Easy/Normal/Hard; `"off"` always hybrid.
    pub router: String,
    /// When true, `context` packs the briefing with the slot-based packer
    /// (decisions / anchored-code / other budget shares) instead of the plain
    /// greedy token-budget packer. Default false (greedy, relevance-ordered).
    pub context_slot_pack: bool,
    /// When true, short/hard queries are expanded with an LLM-generated
    /// hypothetical answer (HyDE) before retrieval, to lift dense recall.
    /// Default false (opt-in; adds an LLM call to the query path).
    pub hyde: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            mode: "hybrid".into(),
            rerank: true,
            decay_lambda: 0.005,
            evidence_window: 0,
            max_per_type: 0,
            router: "adaptive".into(),
            context_slot_pack: false,
            hyde: false,
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

/// Optional Laya System-1 HTTP sidecar (Wave 4). Opt-in; soft-fails to
/// heuristic router / Claude decide+conflict when disabled or unreachable.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct LayaConfig {
    /// Master switch. Default false — must opt in.
    pub enabled: bool,
    /// Sidecar base URL (no trailing slash required).
    pub url: String,
    /// Hard timeout for each predict call (ms). Keep tight for router.
    pub timeout_ms: u64,
    /// Use Laya for Easy/Normal/Hard query routing.
    pub router: bool,
    /// Use Laya for `Decide` synthesis.
    pub decide: bool,
    /// Use Laya for conflict judge + resolve_pair.
    pub conflict: bool,
    /// Model nickname for decide/conflict (`typed-decisions`).
    pub model_decide: String,
    /// Model nickname for router (`english`).
    pub model_router: String,
    /// Below this confidence, treat Laya answer as failure → fallback.
    pub min_confidence: f64,
    /// Sidecar inference backend: `auto` | `mlx` | `torch`.
    /// `auto` prefers `laya-mlx` on Apple Silicon (Python ≥ 3.11) and falls
    /// back to the PyTorch `laya` package elsewhere.
    pub backend: String,
}

impl Default for LayaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "http://127.0.0.1:8765".into(),
            timeout_ms: 80,
            router: true,
            decide: true,
            conflict: true,
            model_decide: "typed-decisions".into(),
            model_router: "english".into(),
            min_confidence: 0.35,
            backend: "auto".into(),
        }
    }
}

/// Entity-graph / briefing-expansion layer (spec: graph-briefing).
/// Ships default-off; flips to default-on only after the CI multi-hop gate
/// (+5 pts) and p95 latency gate pass.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct GraphConfig {
    /// Master switch. When false, no graph reads/writes happen and search /
    /// context output is byte-identical to pre-graph behavior.
    pub enabled: bool,
    /// Max BFS hops from the query centroid (hard cap 2 everywhere).
    pub hops: u8,
    /// Post-fusion multiplier applied to graph-lift-only hits.
    pub boost: f64,
    /// Edge relations followed during traversal. `co_occurs` excluded by
    /// default (noise control).
    pub edge_types: Vec<String>,
    /// Max share of the briefing token budget spent on graph-expanded hits.
    pub budget_pct: f64,
    /// Skip traversal into entities with more than this many mentions.
    pub degree_cap: usize,
    /// Max number of schema/entity nodes supporting graph decision tree per query.
    pub max_query_entities: u8,
    /// Graph ranker: `"bfs"` (default hop-decay) or `"ppr"` (HippoRAG-style
    /// Personalized PageRank over entity edges).
    pub ranker: String,
    /// PPR continue-walk probability (HippoRAG uses 0.5).
    pub ppr_damping: f64,
    /// PPR power-iteration steps.
    pub ppr_iters: u32,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hops: 2,
            boost: 0.15,
            edge_types: vec!["mentions".into(), "fixes".into(), "contradicts".into()],
            budget_pct: 0.4,
            degree_cap: 256,
            max_query_entities: 3,
            ranker: "ppr".into(),
            ppr_damping: 0.5,
            ppr_iters: 20,
        }
    }
}

/// Entity node kinds for the briefing graph (spec: graph-briefing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    File,
    Symbol,
    Concept,
    Person,
    Agent,
}

impl EntityKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityKind::File => "file",
            EntityKind::Symbol => "symbol",
            EntityKind::Concept => "concept",
            EntityKind::Person => "person",
            EntityKind::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "file" => Some(EntityKind::File),
            "symbol" => Some(EntityKind::Symbol),
            "concept" => Some(EntityKind::Concept),
            "person" => Some(EntityKind::Person),
            "agent" => Some(EntityKind::Agent),
            _ => None,
        }
    }
}

/// Edge relations between cues. `co_occurs` is written only for high-weight
/// pairs; graph traversal filters it out by default via
/// [`GraphConfig::edge_types`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeRelation {
    Mentions,
    Fixes,
    Contradicts,
    About,
    CoOccurs,
}

impl EdgeRelation {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeRelation::Mentions => "mentions",
            EdgeRelation::Fixes => "fixes",
            EdgeRelation::Contradicts => "contradicts",
            EdgeRelation::About => "about",
            EdgeRelation::CoOccurs => "co_occurs",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mentions" => Some(EdgeRelation::Mentions),
            "fixes" => Some(EdgeRelation::Fixes),
            "contradicts" => Some(EdgeRelation::Contradicts),
            "about" => Some(EdgeRelation::About),
            "co_occurs" => Some(EdgeRelation::CoOccurs),
            _ => None,
        }
    }
}

/// One entity row, shared across daemon/proto/CLI renderers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Entity {
    pub id: i64,
    pub kind: EntityKind,
    pub name: String,
    pub norm_name: String,
}

/// Normalize an entity display name to its lookup key.
/// Casefold + strip non-alphanumeric (keep `.`/`/`/`_`/`::` for paths).
pub fn normalize_entity_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_alphanumeric() || matches!(ch, '.' | '/' | '_' | ':') {
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

/// How a mention was mined (carried in `entity_mentions.source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MentionSource {
    Anchor,
    Backtick,
    Topic,
    Token,
}

impl MentionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            MentionSource::Anchor => "anchor",
            MentionSource::Backtick => "backtick",
            MentionSource::Topic => "topic",
            MentionSource::Token => "token",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelKind {
    #[default]
    Haiku,
    Sonnet,
}

impl Serialize for ModelKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.cli_model_id())
    }
}

impl<'de> Deserialize<'de> for ModelKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_model(&value).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown model role {value:?}; expected fast|capable (aliases: haiku|sonnet)"
            ))
        })
    }
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

pub fn statefulmemory_global_config_path() -> PathBuf {
    paths::data_dir().join("config.toml")
}

pub fn statefulmemory_project_config_path(project: &str) -> PathBuf {
    paths::data_dir()
        .join("projects")
        .join(format!("{project}.config.toml"))
}

// ---- Resolved-config cache (perf) -------------------------------------------
//
// `load_resolved` is called many times per request (and inside the write txn
// via `should_supersede`). The expensive part is the file read + TOML parse of
// the global + per-project layers; the env-override layer is cheap and must
// stay dynamic (operators and tests change env between calls). So we cache the
// FILE-merged config keyed by (global config path, project) and re-validate it
// by each file's (mtime, len) stamp, re-applying env overrides fresh on every
// call. Writing a config file changes its stamp, which invalidates the entry.

/// `(mtime, len)` fingerprint of a config file; `None` = absent/unreadable.
type FileStamp = Option<(SystemTime, u64)>;

fn file_stamp(path: &Path) -> FileStamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

struct CachedFiles {
    files_cfg: StatefulMemoryConfig,
    global_stamp: FileStamp,
    project_stamp: FileStamp,
}

fn config_cache() -> &'static Mutex<HashMap<String, CachedFiles>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedFiles>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Clear the resolved-config cache. Belt-and-suspenders invalidation for
/// in-process writers (stamp-based invalidation already covers file changes).
pub fn clear_statefulmemory_config_cache() {
    config_cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();
}

/// Layers 1+2 only (global TOML + per-project overlay), deserialized to
/// `StatefulMemoryConfig` BEFORE env overrides. This is the cacheable part.
fn load_merged_files(project_name: Option<&str>) -> StatefulMemoryConfig {
    // Layer 1: global TOML.
    let mut merged = toml::Value::Table(toml::value::Table::new());
    let global_path = statefulmemory_global_config_path();
    if global_path.exists() {
        match std::fs::read_to_string(&global_path) {
            Ok(text) => match toml::from_str::<toml::Value>(&text) {
                Ok(v) => merged = merge_toml_values(merged, v),
                Err(e) => tracing::warn!(
                    path = %global_path.display(),
                    error = %e,
                    "global statefulmemory config parse error — using defaults",
                ),
            },
            Err(e) => tracing::warn!(
                path = %global_path.display(),
                error = %e,
                "could not read global statefulmemory config",
            ),
        }
    }

    // Layer 2: per-project TOML overlay. Errors fall back to global only.
    if let Some(name) = project_name {
        let project_path = statefulmemory_project_config_path(name);
        if project_path.exists() {
            match std::fs::read_to_string(&project_path) {
                Ok(text) => match toml::from_str::<toml::Value>(&text) {
                    Ok(v) => merged = merge_toml_values(merged, v),
                    Err(e) => tracing::error!(
                        path = %project_path.display(),
                        error = %e,
                        "per-project statefulmemory config invalid — falling back to global",
                    ),
                },
                Err(e) => tracing::warn!(
                    path = %project_path.display(),
                    error = %e,
                    "could not read per-project statefulmemory config",
                ),
            }
        }
    }

    // Deserialize the merged value, ignoring unknown keys (forward-compat).
    match merged.try_into::<StatefulMemoryConfig>() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "merged statefulmemory config deserialize failed — using defaults");
            StatefulMemoryConfig::default()
        }
    }
}

/// Resolve `StatefulMemoryConfig` per the 3-level precedence (SC-6).
///
/// `project_name` is `None` when the daemon resolves at startup before any
/// save context is known; the daemon re-resolves with the project name at
/// save time so per-project overrides take effect for that observation.
///
/// The file layers (global + per-project TOML) are cached per project and
/// re-validated by each file's `(mtime, len)` stamp, so repeated calls avoid
/// re-reading and re-parsing TOML. Env overrides are applied fresh every call.
pub fn load_resolved(project_name: Option<&str>) -> StatefulMemoryConfig {
    let global_path = statefulmemory_global_config_path();
    let global_stamp = file_stamp(&global_path);
    let project_stamp = match project_name {
        Some(name) => file_stamp(&statefulmemory_project_config_path(name)),
        None => None,
    };
    // Key on the resolved global path (encodes data_dir) + project, so a
    // changed STATEFULMEMORY_DATA_DIR never serves another dir's config.
    let key = format!("{}|{}", global_path.display(), project_name.unwrap_or(""));

    // Fast path: cached file-layer config whose stamps still match.
    let cached = {
        let guard = config_cache().lock().unwrap_or_else(|p| p.into_inner());
        guard.get(&key).and_then(|e| {
            if e.global_stamp == global_stamp && e.project_stamp == project_stamp {
                Some(e.files_cfg.clone())
            } else {
                None
            }
        })
    };

    let files_cfg = match cached {
        Some(c) => c,
        None => {
            let c = load_merged_files(project_name);
            let mut guard = config_cache().lock().unwrap_or_else(|p| p.into_inner());
            guard.insert(
                key,
                CachedFiles {
                    files_cfg: c.clone(),
                    global_stamp,
                    project_stamp,
                },
            );
            c
        }
    };

    // Layer 3: env vars (highest) — always applied fresh.
    let mut cfg = files_cfg;
    apply_statefulmemory_env_overrides(&mut cfg);
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

fn apply_statefulmemory_env_overrides(cfg: &mut StatefulMemoryConfig) {
    if let Ok(v) = std::env::var("STATEFULMEMORY_EXTRACT_ENABLED") {
        cfg.extract.enabled = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_EXTRACT_MODEL") {
        if let Some(m) = parse_model(&v) {
            cfg.extract.model = m;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_EXTRACT_MODEL: must be haiku or sonnet");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_EXTRACT_TIMEOUT_SECS") {
        if let Ok(n) = v.parse() {
            cfg.extract.timeout_secs = n;
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_EXTRACT_WORKERS") {
        if let Ok(n) = v.parse() {
            cfg.extract.workers = n;
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_RERANK_MODEL") {
        if let Some(m) = parse_model(&v) {
            cfg.rerank.model = m;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_RERANK_MODEL: must be haiku or sonnet");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_RERANK_TIMEOUT_SECS") {
        if let Ok(n) = v.parse() {
            cfg.rerank.timeout_secs = n;
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_RERANK_BACKEND") {
        let m = v.trim().to_ascii_lowercase();
        if m == "local" || m == "llm" || m == "off" {
            cfg.rerank.backend = m;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_RERANK_BACKEND: must be local, llm, or off");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_SEARCH_ROUTER") {
        let m = v.trim().to_ascii_lowercase();
        if m == "adaptive" || m == "off" {
            cfg.search.router = m;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_SEARCH_ROUTER: must be adaptive or off");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_EMBED_WORKERS") {
        if let Ok(n) = v.parse() {
            cfg.embed.workers = n;
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_EMBED_QUANTIZE") {
        cfg.embed.quantize = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_EMBED_CHUNK_LONG_CONTENT") {
        cfg.embed.chunk_long_content = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_EMBED_CHUNK_WORDS") {
        if let Ok(n) = v.parse::<usize>() {
            cfg.embed.chunk_words = n.max(1);
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_EMBED_CHUNK_OVERLAP_WORDS") {
        if let Ok(n) = v.parse::<usize>() {
            cfg.embed.chunk_overlap_words = n;
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_CONFLICT_ENABLED") {
        cfg.conflict.enabled = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_CONFLICT_MODEL") {
        if let Some(m) = parse_model(&v) {
            cfg.conflict.model = m;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_CONFLICT_MODEL: must be haiku or sonnet");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_CONFLICT_TIMEOUT_SECS") {
        if let Ok(n) = v.parse() {
            cfg.conflict.timeout_secs = n;
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_SEARCH_MODE") {
        let m = v.trim().to_ascii_lowercase();
        if m == "hybrid" || m == "bm25" {
            cfg.search.mode = m;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_SEARCH_MODE: must be hybrid or bm25");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_VERIFY_SERVE_STALE") {
        cfg.verify.serve_stale = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_SEARCH_DECAY_LAMBDA") {
        if let Ok(n) = v.parse::<f64>() {
            cfg.search.decay_lambda = n;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_SEARCH_DECAY_LAMBDA: not a float");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_SEARCH_EVIDENCE_WINDOW") {
        if let Ok(n) = v.parse::<u32>() {
            cfg.search.evidence_window = n;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_SEARCH_EVIDENCE_WINDOW: not an integer");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_SEARCH_MAX_PER_TYPE") {
        if let Ok(n) = v.parse::<u32>() {
            cfg.search.max_per_type = n;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_SEARCH_MAX_PER_TYPE: not an integer");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_SEARCH_CONTEXT_SLOT_PACK") {
        cfg.search.context_slot_pack = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_SEARCH_HYDE") {
        cfg.search.hyde = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_GRAPH_ENABLED") {
        cfg.graph.enabled = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_GRAPH_HOPS") {
        if let Ok(n) = v.parse::<u8>() {
            cfg.graph.hops = n.min(2);
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_GRAPH_HOPS: not an integer (max 2)");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_GRAPH_BOOST") {
        if let Ok(n) = v.parse::<f64>() {
            cfg.graph.boost = n;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_GRAPH_BOOST: not a float");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_GRAPH_RANKER") {
        let m = v.trim().to_ascii_lowercase();
        if m == "bfs" || m == "ppr" {
            cfg.graph.ranker = m;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_GRAPH_RANKER: must be bfs or ppr");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_LAYA_ENABLED") {
        cfg.laya.enabled = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_LAYA_URL") {
        let u = v.trim();
        if !u.is_empty() {
            cfg.laya.url = u.to_string();
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_LAYA_TIMEOUT_MS") {
        if let Ok(n) = v.parse::<u64>() {
            cfg.laya.timeout_ms = n;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_LAYA_TIMEOUT_MS: not an integer");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_LAYA_ROUTER") {
        cfg.laya.router = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_LAYA_DECIDE") {
        cfg.laya.decide = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_LAYA_CONFLICT") {
        cfg.laya.conflict = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_LAYA_MIN_CONFIDENCE") {
        if let Ok(n) = v.parse::<f64>() {
            cfg.laya.min_confidence = n;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_LAYA_MIN_CONFIDENCE: not a float");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_LAYA_BACKEND") {
        let b = v.trim().to_ascii_lowercase();
        if b == "auto" || b == "mlx" || b == "torch" {
            cfg.laya.backend = b;
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_LAYA_BACKEND: must be auto, mlx, or torch");
        }
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_UPDATE_ENABLED") {
        cfg.update.enabled = parse_bool_env(&v);
    }
    if let Ok(v) = std::env::var("STATEFULMEMORY_UPDATE_INTERVAL_HOURS") {
        if let Ok(n) = v.parse::<u64>() {
            cfg.update.check_interval_hours = n.max(1);
        } else {
            tracing::warn!(value = %v, "ignoring STATEFULMEMORY_UPDATE_INTERVAL_HOURS: must be an integer");
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
    pub laya: bool,
}

const INSTALL_DEFAULTS: &str = r#"
[search]
mode = "hybrid"
rerank = true
router = "adaptive"

[rerank]
backend = "local"
top_n = 16

[extract]
enabled = true
model = "fast"
timeout_secs = 30
workers = 1

[conflict]
enabled = true
model = "fast"
timeout_secs = 5

[laya]
enabled = true
url = "http://127.0.0.1:8765"
timeout_ms = 80
router = true
decide = true
conflict = true
model_decide = "typed-decisions"
model_router = "english"
min_confidence = 0.35
backend = "auto"

[storage]
backend = "sqlite"
"#;

/// Create `dir/config.toml` or fill in missing keys. Existing values win.
pub fn ensure_config_toml(statefulmemory_dir: &std::path::Path) -> Result<BootstrapReport> {
    std::fs::create_dir_all(statefulmemory_dir)?;
    let path = statefulmemory_dir.join("config.toml");
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
    // A fresh config.toml may change what load_resolved returns in-process.
    clear_statefulmemory_config_cache();

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
    let laya = merged
        .get("laya")
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
        laya,
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
mod statefulmemory_config_tests {
    use super::*;
    /// `STATEFULMEMORY_*` env vars + `STATEFULMEMORY_DATA_DIR` are process-global.
    /// Cargo runs tests in parallel by default; serialize through the crate
    /// lock so tests don't race on the env or the temp data dir.
    #[allow(unused_imports)]
    use crate::TEST_ENV_LOCK;
    fn clear_env() {
        for k in [
            "STATEFULMEMORY_EXTRACT_ENABLED",
            "STATEFULMEMORY_EXTRACT_MODEL",
            "STATEFULMEMORY_EXTRACT_TIMEOUT_SECS",
            "STATEFULMEMORY_EXTRACT_WORKERS",
            "STATEFULMEMORY_RERANK_MODEL",
            "STATEFULMEMORY_RERANK_TIMEOUT_SECS",
            "STATEFULMEMORY_RERANK_BACKEND",
            "STATEFULMEMORY_SEARCH_ROUTER",
            "STATEFULMEMORY_EMBED_WORKERS",
            "STATEFULMEMORY_CONFLICT_ENABLED",
            "STATEFULMEMORY_CONFLICT_MODEL",
            "STATEFULMEMORY_CONFLICT_TIMEOUT_SECS",
            "STATEFULMEMORY_SEARCH_MODE",
            "STATEFULMEMORY_GRAPH_ENABLED",
            "STATEFULMEMORY_GRAPH_HOPS",
            "STATEFULMEMORY_GRAPH_BOOST",
            "STATEFULMEMORY_GRAPH_RANKER",
            "STATEFULMEMORY_LAYA_BACKEND",
            "STATEFULMEMORY_UPDATE_ENABLED",
            "STATEFULMEMORY_UPDATE_INTERVAL_HOURS",
        ] {
            std::env::remove_var(k);
        }
    }

    fn fresh_data_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("projects")).unwrap();
        std::env::set_var("STATEFULMEMORY_DATA_DIR", dir.path());
        dir
    }

    #[test]
    fn default_extract_disabled_haiku() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let _d = fresh_data_dir();
        let cfg = load_resolved(None);
        assert!(!cfg.extract.enabled);
        assert_eq!(cfg.extract.model, ModelKind::Haiku);
        assert_eq!(cfg.rerank.model, ModelKind::Haiku);
        assert_eq!(cfg.embed.workers, 2);
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn model_role_aliases_preserve_other_config_values() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_data_dir();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[extract]
enabled = true
model = "fast"
timeout_secs = 17
workers = 3

[conflict]
enabled = false
model = "capable"
timeout_secs = 9

[laya]
enabled = true
"#,
        )
        .unwrap();

        let cfg = load_resolved(None);
        assert!(cfg.extract.enabled);
        assert_eq!(cfg.extract.model, ModelKind::Haiku);
        assert_eq!(cfg.extract.timeout_secs, 17);
        assert_eq!(cfg.extract.workers, 3);
        assert!(!cfg.conflict.enabled);
        assert_eq!(cfg.conflict.model, ModelKind::Sonnet);
        assert_eq!(cfg.conflict.timeout_secs, 9);
        assert!(cfg.laya.enabled);

        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn laya_backend_env_accepts_valid_rejects_invalid() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let _d = fresh_data_dir();

        let cfg = load_resolved(None);
        assert_eq!(cfg.laya.backend, "auto");

        std::env::set_var("STATEFULMEMORY_LAYA_BACKEND", "mlx");
        let cfg = load_resolved(None);
        assert_eq!(cfg.laya.backend, "mlx");

        std::env::set_var("STATEFULMEMORY_LAYA_BACKEND", "TORCH");
        let cfg = load_resolved(None);
        assert_eq!(cfg.laya.backend, "torch", "case-insensitive normalize");

        std::env::set_var("STATEFULMEMORY_LAYA_BACKEND", "cuda");
        let cfg = load_resolved(None);
        assert_eq!(
            cfg.laya.backend, "auto",
            "invalid value ignored — fresh load falls back to default"
        );

        clear_env();
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn precedence_env_overrides_project_overrides_global_overrides_default() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
        std::env::set_var("STATEFULMEMORY_EXTRACT_MODEL", "haiku");
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
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn invalid_per_project_falls_back_to_global() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn unknown_keys_warn_but_dont_fail() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn project_overlay_preserves_global_section() {
        // Sanity: per-project file with only [rerank] must not erase [extract].
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn search_config_default_is_hybrid() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let c = StatefulMemoryConfig::default();
        assert_eq!(c.search.mode, "hybrid");
        let _d = fresh_data_dir();
        let cfg = load_resolved(None);
        assert_eq!(cfg.search.mode, "hybrid");
        assert!(cfg.search.rerank);
        assert_eq!(cfg.rerank.backend, "local");
        assert_eq!(cfg.search.router, "adaptive");
        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn conflict_config_default_is_enabled() {
        let c = StatefulMemoryConfig::default();
        assert!(c.conflict.enabled);
    }

    #[test]
    fn config_cache_reflects_file_create_and_delete() {
        // The resolved-config cache must invalidate when the config file
        // appears or disappears (present<->absent stamp change), and env
        // overrides must still apply fresh on top of a cached file layer.
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        let dir = fresh_data_dir();
        super::clear_statefulmemory_config_cache();

        // No file → defaults (extract disabled).
        assert!(!load_resolved(None).extract.enabled);

        // Write a global config enabling extract → cache must pick it up.
        std::fs::write(dir.path().join("config.toml"), "[extract]\nenabled = true\n").unwrap();
        assert!(
            load_resolved(None).extract.enabled,
            "cache must reflect a newly-written config file"
        );

        // Env override still applies fresh over the cached file layer.
        std::env::set_var("STATEFULMEMORY_EXTRACT_ENABLED", "0");
        assert!(
            !load_resolved(None).extract.enabled,
            "env override must apply over a cache hit"
        );
        std::env::remove_var("STATEFULMEMORY_EXTRACT_ENABLED");

        // Delete the file → back to defaults (present → absent stamp change).
        std::fs::remove_file(dir.path().join("config.toml")).unwrap();
        assert!(
            !load_resolved(None).extract.enabled,
            "cache must reflect config file deletion"
        );

        std::env::remove_var("STATEFULMEMORY_DATA_DIR");
    }

    #[test]
    fn bootstrap_creates_file_with_hybrid_and_extract() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".statefulmemory");
        let r = ensure_config_toml(&cfg_dir).unwrap();
        assert!(r.created);
        let raw = std::fs::read_to_string(cfg_dir.join("config.toml")).unwrap();
        assert!(raw.contains("mode = \"hybrid\""));
        assert!(raw.contains("enabled = true"));
        assert!(r.extract);
        assert!(r.conflict);
        assert!(r.laya);
        assert_eq!(r.backend, "sqlite");
        assert!(raw.contains("[laya]"));
    }

    #[test]
    fn bootstrap_does_not_clobber_search_mode() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_dir = dir.path().join(".statefulmemory");
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
        assert_eq!(
            serde_json::to_string(&ModelKind::Haiku).unwrap(),
            "\"fast\""
        );
        assert_eq!(
            serde_json::to_string(&ModelKind::Sonnet).unwrap(),
            "\"capable\""
        );
    }
}
