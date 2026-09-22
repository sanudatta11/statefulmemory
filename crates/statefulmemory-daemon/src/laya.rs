//! HTTP client for the optional Laya System-1 sidecar.
//!
//! Soft-fail everywhere: timeouts, connection errors, low confidence, and
//! parse failures return `None` / `Err` so callers fall back to heuristic
//! (router) or Claude (decide/conflict). Never blocks save longer than
//! `timeout_ms`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use statefulmemory_core::config::LayaConfig;
use statefulmemory_core::paths;

/// One normalized answer from the sidecar.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct LayaAnswer {
    pub kind: String,
    pub value: Value,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PredictResponseBody {
    answers: HashMap<String, LayaAnswer>,
    #[serde(default)]
    latency_ms: f64,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LayaPredictResult {
    pub answers: HashMap<String, LayaAnswer>,
    pub latency_ms: f64,
    pub model: Option<String>,
}

/// Blocking HTTP client + short-lived tier cache.
pub struct LayaClient {
    url: String,
    timeout: Duration,
    min_confidence: f64,
    agent: ureq::Agent,
    tier_cache: Mutex<HashMap<String, (Instant, String, f64)>>,
    pub router_timeouts: AtomicU64,
    pub predict_ok: AtomicU64,
    pub predict_err: AtomicU64,
}

impl LayaClient {
    pub fn new(cfg: &LayaConfig) -> Self {
        let timeout = Duration::from_millis(cfg.timeout_ms.max(20));
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(50.min(cfg.timeout_ms.max(20))))
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        Self {
            url: cfg.url.trim_end_matches('/').to_string(),
            timeout,
            min_confidence: cfg.min_confidence,
            agent,
            tier_cache: Mutex::new(HashMap::new()),
            router_timeouts: AtomicU64::new(0),
            predict_ok: AtomicU64::new(0),
            predict_err: AtomicU64::new(0),
        }
    }

    /// Probe `/health`. Returns true when sidecar reports ok.
    pub fn health_ok(&self) -> bool {
        let url = format!("{}/health", self.url);
        match self.agent.get(&url).call() {
            Ok(resp) => {
                let body: Value = resp.into_json().unwrap_or(json!({}));
                body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
            }
            Err(_) => false,
        }
    }

    /// Construct from config when enabled. Soft-spawns the local sidecar if
    /// health fails (install leaves a venv under `~/.statefulmemory/laya-venv`).
    /// Returns `Some` whenever enabled so per-call soft-fail works while the
    /// sidecar is still loading models.
    pub fn try_from_config(cfg: &LayaConfig) -> Option<Arc<Self>> {
        if !cfg.enabled {
            return None;
        }
        let client = Self::new(cfg);
        if client.health_ok() {
            tracing::info!(url = %cfg.url, "laya sidecar ready");
        } else {
            match spawn_local_sidecar(cfg) {
                Ok(true) => tracing::info!(
                    url = %cfg.url,
                    "laya sidecar spawn issued — predicts soft-fail until /health is ok"
                ),
                Ok(false) => tracing::debug!(
                    url = %cfg.url,
                    "laya sidecar not healthy yet (spawn skipped)"
                ),
                Err(e) => tracing::warn!(
                    url = %cfg.url,
                    error = %e,
                    "laya sidecar spawn failed — calls will soft-fail to heuristic/Claude"
                ),
            }
        }
        Some(Arc::new(client))
    }

    pub fn predict(
        &self,
        state: &str,
        questions: &Value,
        model: &str,
    ) -> Result<LayaPredictResult, String> {
        let url = format!("{}/v1/predict", self.url);
        let body = json!({
            "state": state,
            "questions": questions,
            "model": model,
        });
        let started = Instant::now();
        let resp = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|e| {
                self.predict_err.fetch_add(1, Ordering::Relaxed);
                if started.elapsed() >= self.timeout {
                    self.router_timeouts.fetch_add(1, Ordering::Relaxed);
                }
                format!("laya predict: {e}")
            })?;
        let parsed: PredictResponseBody = resp.into_json().map_err(|e| {
            self.predict_err.fetch_add(1, Ordering::Relaxed);
            format!("laya decode: {e}")
        })?;
        self.predict_ok.fetch_add(1, Ordering::Relaxed);
        Ok(LayaPredictResult {
            answers: parsed.answers,
            latency_ms: parsed.latency_ms,
            model: parsed.model,
        })
    }

    /// Choice answer for `name` if present and confidence ≥ min.
    pub fn choice_value<'a>(
        &'a self,
        result: &'a LayaPredictResult,
        name: &str,
    ) -> Option<&'a str> {
        let ans = result.answers.get(name)?;
        if let Some(c) = ans.confidence {
            if c < self.min_confidence {
                return None;
            }
        }
        ans.value.as_str()
    }

    pub fn score_value(&self, result: &LayaPredictResult, name: &str) -> Option<f64> {
        let ans = result.answers.get(name)?;
        if let Some(c) = ans.confidence {
            if c < self.min_confidence {
                return None;
            }
        }
        ans.value
            .as_f64()
            .or_else(|| ans.value.as_i64().map(|i| i as f64))
    }

    pub fn noul_true(&self, result: &LayaPredictResult, name: &str) -> Option<bool> {
        let ans = result.answers.get(name)?;
        if let Some(c) = ans.confidence {
            if c < self.min_confidence {
                return None;
            }
        }
        match &ans.value {
            Value::Bool(b) => Some(*b),
            Value::String(s) => match s.to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => Some(true),
                "false" | "no" | "0" => Some(false),
                _ => None,
            },
            Value::Number(n) => n.as_f64().map(|f| f >= 0.5),
            _ => None,
        }
    }

    /// Cached router tier string (`easy`/`normal`/`hard`).
    pub fn cached_tier(&self, query: &str) -> Option<String> {
        let key = cache_key(query);
        let mut guard = self.tier_cache.lock();
        if let Some((at, tier, _)) = guard.get(&key) {
            if at.elapsed() < Duration::from_secs(30) {
                return Some(tier.clone());
            }
            guard.remove(&key);
        }
        None
    }

    pub fn put_tier_cache(&self, query: &str, tier: &str, conf: f64) {
        let key = cache_key(query);
        let mut guard = self.tier_cache.lock();
        if guard.len() > 512 {
            guard.clear();
        }
        guard.insert(key, (Instant::now(), tier.to_string(), conf));
    }
}

/// Best-effort background start of the install-managed local sidecar.
///
/// Returns `Ok(true)` if a new process was spawned, `Ok(false)` if skipped
/// (already running, missing venv, or non-loopback URL).
pub fn spawn_local_sidecar(cfg: &LayaConfig) -> Result<bool, String> {
    let url = cfg.url.trim().to_ascii_lowercase();
    let local = url.contains("127.0.0.1") || url.contains("localhost");
    if !local {
        return Ok(false);
    }

    let py = paths::laya_venv_python();
    let app_dir = paths::laya_sidecar_dir();
    if !py.exists() || !app_dir.join("server.py").exists() {
        return Ok(false);
    }

    let probe = LayaClient::new(cfg);
    if probe.health_ok() {
        return Ok(false);
    }

    if sidecar_pid_alive() {
        return Ok(false);
    }

    let port = port_from_url(&cfg.url).unwrap_or(8765);
    let host = "127.0.0.1";

    let mut cmd = std::process::Command::new(&py);
    cmd.args([
        "-m",
        "uvicorn",
        "server:app",
        "--host",
        host,
        "--port",
        &port.to_string(),
    ])
    .current_dir(&app_dir)
    .env("USE_TF", "0")
    .env("LAYA_BACKEND", cfg.backend.trim())
    .env("LAYA_HOST", host)
    .env("LAYA_PORT", port.to_string())
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // Detach so daemon stop does not kill the sidecar.
                let _ = nix::unistd::setsid();
                Ok(())
            });
        }
    }

    let child = cmd.spawn().map_err(|e| format!("spawn uvicorn: {e}"))?;
    let _ = std::fs::write(paths::laya_sidecar_pid_path(), child.id().to_string());
    Ok(true)
}

fn port_from_url(url: &str) -> Option<u16> {
    let after = url.split("://").nth(1)?;
    let hostport = after.split('/').next()?;
    let port = hostport.split(':').nth(1)?;
    port.parse().ok()
}

fn sidecar_pid_alive() -> bool {
    let path = paths::laya_sidecar_pid_path();
    let Ok(s) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(pid) = s.trim().parse::<i32>() else {
        return false;
    };
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let alive = kill(Pid::from_raw(pid), None as Option<Signal>).is_ok();
        if !alive {
            let _ = std::fs::remove_file(&path);
        }
        alive
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn cache_key(query: &str) -> String {
    let mut h = Sha256::new();
    h.update(query.trim().as_bytes());
    hex::encode(h.finalize())
}

/// Append a teacher / shadow training row (best-effort, never panics callers).
pub fn log_teacher(kind: &str, state: &str, questions: &Value, answers: &Value) {
    let dir = train_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(error = %e, "laya train dir create failed");
        return;
    }
    let path = dir.join(format!("{kind}.jsonl"));
    let row = json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "kind": kind,
        "state": truncate(state, 4000),
        "questions": questions,
        "answers": answers,
    });
    let line = match serde_json::to_string(&row) {
        Ok(s) => s,
        Err(_) => return,
    };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn train_dir() -> PathBuf {
    paths::data_dir().join("laya_train")
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_respects_min_confidence() {
        let cfg = LayaConfig {
            enabled: true,
            url: "http://127.0.0.1:9".into(),
            timeout_ms: 50,
            router: true,
            decide: true,
            conflict: true,
            model_decide: "typed-decisions".into(),
            model_router: "english".into(),
            min_confidence: 0.5,
            backend: "auto".into(),
        };
        let client = LayaClient::new(&cfg);
        let mut answers = HashMap::new();
        answers.insert(
            "tier".into(),
            LayaAnswer {
                kind: "choice".into(),
                value: json!("easy"),
                confidence: Some(0.2),
            },
        );
        let res = LayaPredictResult {
            answers,
            latency_ms: 1.0,
            model: None,
        };
        assert!(client.choice_value(&res, "tier").is_none());
    }

    #[test]
    fn port_from_url_parses() {
        assert_eq!(port_from_url("http://127.0.0.1:8765"), Some(8765));
        assert_eq!(port_from_url("http://localhost:9000/v1"), Some(9000));
    }
}
