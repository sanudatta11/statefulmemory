//! Shared test harness: spawns the daemon binary in a temp data dir and
//! returns a gRPC client connected over UDS.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use memlayer_proto::memlayer_client::MemlayerClient;
use tonic::transport::{Channel, Endpoint, Uri};

pub struct DaemonHandle {
    pub data_dir: tempfile::TempDir,
    pub child: Child,
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn the daemon binary in foreground mode against an isolated data dir.
pub fn spawn_daemon() -> DaemonHandle {
    let data_dir = tempfile::TempDir::new().expect("tempdir");
    // Locate the compiled binary. `cargo test` puts it at target/<profile>/memlayer.
    let bin = locate_binary();
    let child = Command::new(bin)
        .args(["daemon", "start", "--foreground"])
        .env("MEMLAYER_DATA_DIR", data_dir.path())
        .env("MEMLAYER_LOG", "warn")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn daemon");
    DaemonHandle { data_dir, child }
}

fn locate_binary() -> PathBuf {
    // Cargo populates CARGO_BIN_EXE_<name> for binary deps in [[bin]] crates
    // when the binary lives in the same package as the integration test.
    // memlayer-tests is its own package, so this is rarely populated; we
    // fall back to walking the workspace target dir.
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_memlayer") {
        return PathBuf::from(p);
    }

    // Walk up from the running test binary. The test binary lives at
    // `<target>/<profile>/deps/<test>-<hash>`, so its parent's parent is
    // `<target>/<profile>/`. This works even when CARGO_TARGET_DIR is set
    // or the workspace layout is non-standard.
    if let Ok(exe) = std::env::current_exe() {
        let mut cur: Option<&Path> = exe.parent();
        for _ in 0..4 {
            let dir = match cur {
                Some(d) => d,
                None => break,
            };
            let cand = dir.join("memlayer");
            if cand.exists() {
                return cand;
            }
            cur = dir.parent();
        }
    }

    // Last-resort fallback: walk up from the manifest dir to a `target` sibling.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let target = Path::new(&manifest).parent().unwrap().parent().unwrap().join("target");
    for profile in ["debug", "release"] {
        let cand = target.join(profile).join("memlayer");
        if cand.exists() {
            return cand;
        }
    }
    panic!(
        "could not locate `memlayer` binary; tried CARGO_BIN_EXE_memlayer, \
         walking up from current_exe(), and {target_debug}/memlayer / \
         {target_release}/memlayer. Run `cargo build -p memlayer-cli` first.",
        target_debug = target.join("debug").display(),
        target_release = target.join("release").display(),
    );
}

/// Poll the UDS until it accepts connections, up to 5 s (FR2.3).
pub async fn connect(handle: &DaemonHandle) -> MemlayerClient<Channel> {
    let sock = handle.data_dir.path().join("daemon.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut delay = Duration::from_millis(10);
    loop {
        if sock.exists() {
            match try_connect(&sock).await {
                Ok(c) => return c,
                Err(_) => {}
            }
        }
        if Instant::now() > deadline {
            panic!("daemon socket never appeared at {}", sock.display());
        }
        tokio::time::sleep(delay).await;
        delay = std::cmp::min(delay * 2, Duration::from_millis(640));
    }
}

async fn try_connect(sock: &Path) -> Result<MemlayerClient<Channel>, tonic::transport::Error> {
    // tonic UDS connection requires a custom Endpoint connector.
    let path = sock.to_path_buf();
    let endpoint = Endpoint::try_from("http://[::1]:50051")
        .expect("dummy endpoint")
        .connect_timeout(Duration::from_secs(2));
    let channel = endpoint
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(p).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await?;
    Ok(MemlayerClient::new(channel))
}
