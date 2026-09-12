// Generated with AI Coding Rules Hub
//! BGE-small-en-v1.5 sentence embedder running on candle-rs (pure Rust).
//!
//! Spec links: TS-2 (deterministic embeddings), EH-2 (HF download failure
//! surfaces a structured error), SC-7 (no fastembed/ort-sys dependency),
//! Plan §3.2 / §3.5.
//!
//! Why candle and not fastembed?
//!   `fastembed` (and any ort-backed embedder) pulls `ort-sys`, which
//!   downloads a precompiled ONNX runtime tarball from `parcel.pyke.io`
//!   during the build. That host is blocked by enterprise TLS interception,
//!   so the build fails before a single line of code runs. candle resolves
//!   and builds entirely from crates.io — no third-party fetch — and BGE
//!   inference on CPU is plenty fast for the retrieval path.
//!
//! Network policy:
//!   `try_new` resolves weights in this order:
//!   1. `MEMLAYER_BGE_MODEL_DIR` (explicit vendor path)
//!   2. `~/.memlayer-models/bge-small` when already complete
//!   3. `hf-hub` ≥0.4 (joins HuggingFace relative `/api/resolve-cache/…`
//!      redirects; 0.3 failed with `RelativeUrlWithoutBase`)
//!   4. `curl` into `~/.memlayer-models/bge-small` if hf-hub still fails
//!      (TLS interception, empty cache, etc.)
//!
//! Model: `BAAI/bge-small-en-v1.5` (BERT-style, 384-dim, 33M params).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use hf_hub::{api::sync::Api, Repo, RepoType};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer};

/// Repository id on HuggingFace Hub.
const MODEL_ID: &str = "BAAI/bge-small-en-v1.5";
/// Output dimensionality of `bge-small-en-v1.5` (matches its `hidden_size`).
const EMBED_DIM: usize = 384;
/// Absolute resolve URLs (curl follows relative redirects correctly).
const HF_RESOLVE_BASE: &str = "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main";
const MODEL_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

/// Trait for sentence embedders. Implementations must be thread-safe so the
/// retrieval path can share a single instance behind an `Arc`.
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts. Returns one f32 vector per input, in input order.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    /// Output dimensionality (constant per model).
    fn dim(&self) -> usize;
}

/// CPU-only BGE-small embedder backed by candle's `BertModel`.
///
/// Construction is heavy (downloads + memory-maps weights). `embed` is
/// cheap and reentrant: candle's `BertModel::forward` takes `&self`, and
/// our tokenizer state is never mutated post-construction.
pub struct BgeSmallEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

fn default_vendor_dir() -> PathBuf {
    dirs_next_home()
        .map(|h| h.join(".memlayer-models").join("bge-small"))
        .unwrap_or_else(|| PathBuf::from(".memlayer-models/bge-small"))
}

/// Prefer `dirs`-less home lookup to avoid a new crate dep.
fn dirs_next_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn model_paths_in(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    (
        dir.join("config.json"),
        dir.join("tokenizer.json"),
        dir.join("model.safetensors"),
    )
}

fn dir_has_model_files(dir: &Path) -> bool {
    let (cfg, tok, wts) = model_paths_in(dir);
    cfg.is_file() && tok.is_file() && wts.is_file()
}

fn require_model_files(dir: &Path) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let (cfg, tok, wts) = model_paths_in(dir);
    for p in [&cfg, &tok, &wts] {
        if !p.is_file() {
            anyhow::bail!(
                "BGE model dir {} is missing {}",
                dir.display(),
                p.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            );
        }
    }
    Ok((cfg, tok, wts))
}

fn download_via_hf_hub() -> Result<(PathBuf, PathBuf, PathBuf)> {
    let api = Api::new().context("failed to initialize HuggingFace API client")?;
    let repo = api.repo(Repo::new(MODEL_ID.to_string(), RepoType::Model));
    let cfg = repo
        .get("config.json")
        .with_context(|| format!("download {MODEL_ID}/config.json from HuggingFace Hub"))?;
    let tok = repo
        .get("tokenizer.json")
        .with_context(|| format!("download {MODEL_ID}/tokenizer.json from HuggingFace Hub"))?;
    let wts = repo
        .get("model.safetensors")
        .with_context(|| format!("download {MODEL_ID}/model.safetensors from HuggingFace Hub"))?;
    Ok((cfg, tok, wts))
}

/// Download model files with system `curl`, which correctly follows HuggingFace's
/// relative `Location` redirects (hf-hub 0.3 passes those to ureq as absolute-less URLs).
fn download_via_curl(dest: &Path) -> Result<(PathBuf, PathBuf, PathBuf)> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("create BGE vendor dir {}", dest.display()))?;
    for name in MODEL_FILES {
        let out = dest.join(name);
        if out.is_file() {
            continue;
        }
        let url = format!("{HF_RESOLVE_BASE}/{name}");
        tracing::info!(%url, dest = %out.display(), "downloading BGE file via curl");
        let status = Command::new("curl")
            .args([
                "-fL",
                "--retry",
                "4",
                "--retry-delay",
                "2",
                "--progress-bar",
                "-o",
            ])
            .arg(&out)
            .arg(&url)
            .status()
            .context("spawn curl to download BGE model (is curl on PATH?)")?;
        if !status.success() {
            let _ = std::fs::remove_file(&out);
            anyhow::bail!(
                "curl failed downloading {name} from {url} (exit {status}); \
                 set MEMLAYER_BGE_MODEL_DIR after running \
                 crates/memlayer-eval/scripts/vendor_bge_model.sh"
            );
        }
    }
    require_model_files(dest)
}

fn resolve_model_paths() -> Result<(PathBuf, PathBuf, PathBuf)> {
    if let Ok(local_dir) = std::env::var("MEMLAYER_BGE_MODEL_DIR") {
        let dir = PathBuf::from(local_dir);
        tracing::info!(?dir, "loading BGE-small from MEMLAYER_BGE_MODEL_DIR");
        return require_model_files(&dir);
    }

    let vendor = default_vendor_dir();
    if dir_has_model_files(&vendor) {
        tracing::info!(?vendor, "loading BGE-small from default vendor dir");
        return require_model_files(&vendor);
    }

    match download_via_hf_hub() {
        Ok(paths) => Ok(paths),
        Err(hf_err) => {
            tracing::warn!(
                error = %hf_err,
                ?vendor,
                "hf-hub BGE download failed; falling back to curl vendor dir"
            );
            download_via_curl(&vendor).with_context(|| {
                format!(
                    "load BGE-small embedder (hf-hub failed: {hf_err}; curl fallback also failed)"
                )
            })
        }
    }
}

impl BgeSmallEmbedder {
    /// Load BGE-small-en-v1.5.
    ///
    /// **Path-resolution order:**
    /// 1. `MEMLAYER_BGE_MODEL_DIR` — explicit vendor path
    /// 2. `~/.memlayer-models/bge-small` if already populated
    /// 3. `hf-hub` (caches under `~/.cache/huggingface/`)
    /// 4. `curl` into `~/.memlayer-models/bge-small` when hf-hub fails
    ///    (e.g. HuggingFace relative redirect → `RelativeUrlWithoutBase`)
    ///
    /// Errors are wrapped with `anyhow::Context` so callers can surface a
    /// useful message (EH-2). Common causes:
    /// * Local override path doesn't contain all three required files.
    /// * No network and an empty HF / vendor cache.
    /// * Corporate proxy blocks `huggingface.co`. Vendor the model and
    ///   set `MEMLAYER_BGE_MODEL_DIR` to bypass.
    /// * Disk full while writing to the cache directory.
    pub fn try_new() -> Result<Self> {
        let (config_path, tokenizer_path, weights_path) = resolve_model_paths()?;

        let config_json =
            std::fs::read_to_string(&config_path).context("read BGE config.json")?;
        let config: Config =
            serde_json::from_str(&config_json).context("parse BGE config.json")?;

        // tokenizers' error type is not Send + Sync; map to anyhow via Display.
        let mut tokenizer =
            Tokenizer::from_file(&tokenizer_path).map_err(anyhow::Error::msg)?;

        // Force batch-longest padding so the encoder always produces uniform
        // length tensors for `encode_batch`. BGE was trained with
        // [PAD]/attention-mask, so this keeps inference faithful.
        if let Some(pp) = tokenizer.get_padding_mut() {
            pp.strategy = PaddingStrategy::BatchLongest;
        } else {
            tokenizer.with_padding(Some(PaddingParams {
                strategy: PaddingStrategy::BatchLongest,
                ..Default::default()
            }));
        }

        let device = Device::Cpu;
        // SAFETY: `from_mmaped_safetensors` is unsafe only because mapping
        // a file the OS later truncates would yield UB. We control the
        // path (HF cache) and never overwrite it during a process lifetime.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)
                .context("memory-map BGE weights via safetensors")?
        };
        let model = BertModel::load(vb, &config).context("load BertModel weights")?;

        tracing::debug!(model = MODEL_ID, dim = EMBED_DIM, "BGE-small loaded");
        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    /// Tokenize, run BertModel forward, mean-pool with attention mask, then
    /// L2-normalize. Returns one row per input.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // tokenizers wants owned Strings or &str — cloning to Vec<String>
        // keeps the lifetime story simple. Tokenization isn't the hot path.
        let inputs: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let encodings = self
            .tokenizer
            .encode_batch(inputs, true)
            .map_err(anyhow::Error::msg)
            .context("tokenize batch with BGE tokenizer")?;

        let batch = encodings.len();
        let seq = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0);

        // Build (B, T) input_ids and attention_mask tensors. token_type_ids
        // is all-zero for sentence inputs (BGE never uses pair encoding).
        let mut ids: Vec<u32> = Vec::with_capacity(batch * seq);
        let mut mask: Vec<u32> = Vec::with_capacity(batch * seq);
        for enc in &encodings {
            ids.extend_from_slice(enc.get_ids());
            mask.extend_from_slice(enc.get_attention_mask());
        }

        let input_ids = Tensor::from_vec(ids, (batch, seq), &self.device)
            .context("build input_ids tensor")?;
        let attention_mask = Tensor::from_vec(mask, (batch, seq), &self.device)
            .context("build attention_mask tensor")?;
        let token_type_ids = input_ids.zeros_like().context("build token_type_ids")?;

        let hidden = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .context("BertModel::forward")?;

        // Mean-pool, masked. Shapes:
        //   hidden:        (B, T, H)
        //   mask_f:        (B, T, 1)  via to_dtype + unsqueeze
        //   summed:        (B, H)
        //   counts:        (B, 1)     clamped to avoid div-by-zero
        let mask_f = attention_mask
            .to_dtype(DType::F32)?
            .unsqueeze(2)?;
        let summed = hidden.broadcast_mul(&mask_f)?.sum(1)?;
        let counts = mask_f.sum(1)?.clamp(1e-9_f64, f64::INFINITY)?;
        let pooled = summed.broadcast_div(&counts)?;

        // L2-normalize each row. After this, dot product == cosine similarity.
        let norms = pooled.sqr()?.sum_keepdim(1)?.sqrt()?;
        let normalized = pooled.broadcast_div(&norms)?;

        let out: Vec<Vec<f32>> = normalized.to_vec2::<f32>().context("to_vec2 output")?;
        debug_assert_eq!(out.len(), batch);
        debug_assert!(out.iter().all(|v| v.len() == EMBED_DIM));
        Ok(out)
    }
}

impl Embedder for BgeSmallEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed_batch(texts)
    }

    fn dim(&self) -> usize {
        EMBED_DIM
    }
}
