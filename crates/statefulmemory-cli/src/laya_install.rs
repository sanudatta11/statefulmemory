//! Install / ensure Laya System-1 sidecar prerequisites for `smem install`.
//!
//! Writes bundled FastAPI assets under `~/.statefulmemory/laya-sidecar/`,
//! creates a dedicated venv, `pip install`s deps, and best-effort starts the
//! sidecar so local (and multi-agent) installs work out of the box.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use statefulmemory_core::paths;

const SERVER_PY: &str = include_str!("../../../tools/laya-sidecar/server.py");
const REQUIREMENTS_TXT: &str = include_str!("../../../tools/laya-sidecar/requirements.txt");

#[derive(Debug, Clone)]
pub struct LayaInstallReport {
    pub assets_written: bool,
    pub venv_created: bool,
    pub deps_ok: bool,
    pub sidecar_running: bool,
    pub messages: Vec<String>,
    pub warnings: Vec<String>,
}

/// Full prerequisite setup used by `statefulmemory install` / `smem install`.
pub fn ensure_laya_prereqs() -> LayaInstallReport {
    let mut report = LayaInstallReport {
        assets_written: false,
        venv_created: false,
        deps_ok: false,
        sidecar_running: false,
        messages: Vec::new(),
        warnings: Vec::new(),
    };

    match write_assets() {
        Ok(wrote) => {
            report.assets_written = wrote;
            if wrote {
                report.messages.push(format!(
                    "Laya sidecar assets → {}",
                    paths::laya_sidecar_dir().display()
                ));
            } else {
                report
                    .messages
                    .push("Laya sidecar assets already up-to-date".into());
            }
        }
        Err(e) => {
            report
                .warnings
                .push(format!("could not write Laya sidecar assets: {e}"));
            return report;
        }
    }

    let python = match find_system_python() {
        Some(p) => p,
        None => {
            report.warnings.push(
                "python3 not found on PATH — install Python 3.10+ then re-run \
                 `smem install` (or `make laya-sidecar` from a checkout)"
                    .into(),
            );
            return report;
        }
    };

    match ensure_venv(&python) {
        Ok(created) => {
            report.venv_created = created;
            if created {
                report.messages.push(format!(
                    "Laya venv created → {}",
                    paths::laya_venv_dir().display()
                ));
            } else {
                report.messages.push("Laya venv already present".into());
            }
        }
        Err(e) => {
            report.warnings.push(format!("Laya venv setup failed: {e}"));
            return report;
        }
    }

    match pip_install_deps() {
        Ok(()) => {
            report.deps_ok = true;
            report
                .messages
                .push("Laya Python deps installed (laya, fastapi, uvicorn)".into());
        }
        Err(e) => {
            report.warnings.push(format!(
                "Laya pip install failed: {e} — sidecar will soft-disable until fixed"
            ));
            return report;
        }
    }

    let cfg = statefulmemory_core::config::load_resolved(None).laya;
    let probe = statefulmemory_daemon::laya::LayaClient::new(&cfg);
    if probe.health_ok() {
        report.sidecar_running = true;
        report.messages.push("Laya sidecar already healthy".into());
        return report;
    }

    match statefulmemory_daemon::laya::spawn_local_sidecar(&cfg) {
        Ok(true) => {
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(400));
                if probe.health_ok() {
                    report.sidecar_running = true;
                    break;
                }
            }
            if report.sidecar_running {
                report
                    .messages
                    .push(format!("Laya sidecar started ({})", cfg.url));
            } else {
                report.messages.push(
                    "Laya sidecar spawn issued — first model load may take a few minutes; \
                     daemon will soft-retry"
                        .into(),
                );
            }
        }
        Ok(false) => report
            .messages
            .push("Laya sidecar spawn skipped (already starting or missing venv)".into()),
        Err(e) => report
            .warnings
            .push(format!("could not start Laya sidecar: {e}")),
    }

    report
}

fn write_assets() -> std::io::Result<bool> {
    let dir = paths::laya_sidecar_dir();
    std::fs::create_dir_all(&dir)?;
    let mut changed = false;
    changed |= write_if_changed(&dir.join("server.py"), SERVER_PY)?;
    changed |= write_if_changed(&dir.join("requirements.txt"), REQUIREMENTS_TXT)?;
    #[cfg(unix)]
    {
        let launcher = dir.join("run.sh");
        let body = format!(
            "#!/usr/bin/env bash\n\
             set -euo pipefail\n\
             export USE_TF=\"${{USE_TF:-0}}\"\n\
             export LAYA_HOST=\"${{LAYA_HOST:-127.0.0.1}}\"\n\
             export LAYA_PORT=\"${{LAYA_PORT:-8765}}\"\n\
             PY=\"{py}\"\n\
             cd \"{dir}\"\n\
             exec \"$PY\" -m uvicorn server:app --host \"$LAYA_HOST\" --port \"$LAYA_PORT\"\n",
            py = paths::laya_venv_python().display(),
            dir = dir.display(),
        );
        changed |= write_if_changed(&launcher, &body)?;
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&launcher)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&launcher, perms)?;
    }
    Ok(changed)
}

fn write_if_changed(path: &Path, content: &str) -> std::io::Result<bool> {
    if path.exists() {
        if let Ok(existing) = std::fs::read_to_string(path) {
            if existing == content {
                return Ok(false);
            }
        }
    }
    std::fs::write(path, content)?;
    Ok(true)
}

fn find_system_python() -> Option<PathBuf> {
    for name in ["python3", "python"] {
        if let Ok(out) = Command::new(name).arg("--version").output() {
            if out.status.success() {
                return Some(PathBuf::from(name));
            }
        }
    }
    None
}

fn ensure_venv(system_python: &Path) -> Result<bool, String> {
    let venv = paths::laya_venv_dir();
    let py = paths::laya_venv_python();
    if py.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(paths::data_dir()).map_err(|e| e.to_string())?;
    let status = Command::new(system_python)
        .args(["-m", "venv"])
        .arg(&venv)
        .status()
        .map_err(|e| format!("venv create: {e}"))?;
    if !status.success() {
        return Err(format!("python -m venv failed (status {status})"));
    }
    if !py.exists() {
        return Err(format!("venv python missing at {}", py.display()));
    }
    Ok(true)
}

fn pip_install_deps() -> Result<(), String> {
    let py = paths::laya_venv_python();
    let req = paths::laya_sidecar_dir().join("requirements.txt");
    let _ = Command::new(&py)
        .args(["-m", "pip", "install", "--upgrade", "pip"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let out = Command::new(&py)
        .args(["-m", "pip", "install", "-r"])
        .arg(&req)
        .output()
        .map_err(|e| format!("pip: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(stderr.chars().take(400).collect());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_assets_nonempty() {
        assert!(SERVER_PY.contains("FastAPI"));
        assert!(REQUIREMENTS_TXT.contains("laya"));
    }
}
