//! Integration-test harness for the `memlayer` CLI binary and daemon.
//!
//! Two flavors are exposed:
//!
//! - [`spawn_daemon`] / [`connect`] — spawn a foreground daemon directly and
//!   talk to it over gRPC. Used by older RPC-level round-trip tests.
//! - [`CliEnv`] — full CLI-driven harness: tempdir data directory, optional
//!   pre-spawned daemon, and a [`CliEnv::cmd`] builder that wires
//!   `MEMLAYER_DATA_DIR`, `MEMLAYER_PROJECT`, and `MEMLAYER_LOG` so the binary
//!   under test is fully isolated from `~/.memlayer` and the developer's git
//!   remote. Used by the spec2-t8 test suite (TS-1 .. TS-28).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    let bin = locate_binary();
    let child = Command::new(bin)
        .args(["daemon", "start", "--foreground"])
        .env("MEMLAYER_DATA_DIR", data_dir.path())
        .env("MEMLAYER_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daemon");
    DaemonHandle { data_dir, child }
}

pub fn locate_binary() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_memlayer") {
        return PathBuf::from(p);
    }

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
    wait_for_socket(&sock, Duration::from_secs(5))
        .await
        .unwrap_or_else(|| panic!("daemon socket never appeared at {}", sock.display()));
    try_connect(&sock).await.expect("connect")
}

async fn wait_for_socket(sock: &Path, budget: Duration) -> Option<()> {
    let deadline = Instant::now() + budget;
    let mut delay = Duration::from_millis(10);
    loop {
        if sock.exists() {
            return Some(());
        }
        if Instant::now() > deadline {
            return None;
        }
        tokio::time::sleep(delay).await;
        delay = std::cmp::min(delay * 2, Duration::from_millis(640));
    }
}

async fn try_connect(sock: &Path) -> Result<MemlayerClient<Channel>, tonic::transport::Error> {
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

// ---------------------------------------------------------------------------
// CLI harness (spec2-t8)
// ---------------------------------------------------------------------------

/// CLI-driven test environment.
///
/// Each `CliEnv` owns a tempdir set as `MEMLAYER_DATA_DIR` so the daemon
/// stores its socket, log, project DBs, and tokens inside the tempdir. The
/// tempdir is cleaned up when the env is dropped.
///
/// Default project name is `test-proj` (set via `MEMLAYER_PROJECT`) so tests
/// don't depend on whatever git remote `cargo test` happens to inherit. Pass
/// `with_project` to override.
pub struct CliEnv {
    pub data_dir: tempfile::TempDir,
    pub binary: PathBuf,
    pub project: String,
    daemon: Option<Child>,
}

impl CliEnv {
    pub fn new() -> Self {
        Self {
            data_dir: tempfile::TempDir::new().expect("tempdir"),
            binary: locate_binary(),
            project: "test-proj".to_string(),
            daemon: None,
        }
    }

    pub fn with_project(mut self, name: impl Into<String>) -> Self {
        self.project = name.into();
        self
    }

    pub fn data_path(&self) -> &Path {
        self.data_dir.path()
    }

    pub fn socket_path(&self) -> PathBuf {
        self.data_path().join("daemon.sock")
    }

    pub fn log_path(&self) -> PathBuf {
        self.data_path().join("daemon.log")
    }

    pub fn tokens_db_path(&self) -> PathBuf {
        self.data_path().join("tokens.db")
    }

    /// Spawn the daemon in foreground mode against this env's data dir, then
    /// block until the UDS socket actually accepts connections (max 5 s).
    /// Polling for socket-file *existence* is not enough: tonic creates the
    /// listener fd before its acceptor task starts, so a test that issues
    /// an RPC immediately after socket-exists can hit "h2 protocol error"
    /// because the server isn't ready to handshake yet.
    pub fn spawn_daemon(&mut self) -> &mut Self {
        // Redirect daemon stderr to a file under the env's data dir so a
        // failing test can dump it via `env.daemon_stderr()` and see why
        // the daemon died (panics, RUST_BACKTRACE traces, etc.). Piping
        // to Stdio::piped() and not reading would buffer indefinitely;
        // a file means the bytes are available even after the test panic.
        let stderr_log = std::fs::File::create(self.data_path().join("daemon.stderr"))
            .expect("create daemon.stderr log");
        let child = Command::new(&self.binary)
            .args(["daemon", "start", "--foreground"])
            .env("MEMLAYER_DATA_DIR", self.data_path())
            .env("MEMLAYER_LOG", "warn")
            // RUST_BACKTRACE=1 makes panics in the daemon visible in the
            // captured stderr log, which is invaluable when the only
            // symptom client-side is "h2 protocol error".
            .env("RUST_BACKTRACE", "1")
            .stdout(Stdio::null())
            .stderr(stderr_log)
            .spawn()
            .expect("spawn daemon");
        self.daemon = Some(child);

        let sock = self.socket_path();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut delay = Duration::from_millis(10);

        // Phase 1: wait for the socket file to appear.
        while !sock.exists() {
            if Instant::now() > deadline {
                panic!("daemon socket never appeared at {}", sock.display());
            }
            std::thread::sleep(delay);
            delay = std::cmp::min(delay * 2, Duration::from_millis(160));
        }

        // Phase 2: wait until a client UnixStream::connect actually succeeds.
        // tonic creates the listener fd before its acceptor task is hooked
        // up; under parallel test load this gap is wide enough for a racing
        // RPC to land on a half-open socket and surface as h2 protocol error.
        let mut delay = Duration::from_millis(10);
        loop {
            match std::os::unix::net::UnixStream::connect(&sock) {
                Ok(_) => break,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(delay);
                    delay = std::cmp::min(delay * 2, Duration::from_millis(160));
                }
                Err(e) => panic!("daemon socket {} never accepted connections: {e}", sock.display()),
            }
        }
        self
    }

    /// Read whatever the daemon has written to its captured stderr file.
    /// Returns `None` if the file does not exist (e.g. spawn_daemon was
    /// never called). Useful inside test panic handlers.
    pub fn daemon_stderr(&self) -> Option<String> {
        std::fs::read_to_string(self.data_path().join("daemon.stderr")).ok()
    }

    /// Build a fresh `Command` invoking the `memlayer` binary, with the env's
    /// data dir + project name pre-wired and inherited env vars stripped so
    /// the test environment doesn't leak a developer's `MEMLAYER_*` overrides.
    pub fn cmd(&self) -> Command {
        let mut c = Command::new(&self.binary);
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.data_path())
            .env("MEMLAYER_DATA_DIR", self.data_path())
            .env("MEMLAYER_PROJECT", &self.project)
            .env("MEMLAYER_LOG", "warn")
            .current_dir(self.data_path());
        c
    }

    /// Same as [`cmd`] but does *not* set `MEMLAYER_PROJECT`. Use for tests
    /// that exercise the project-detection algorithm itself.
    pub fn cmd_no_project_env(&self) -> Command {
        let mut c = Command::new(&self.binary);
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.data_path())
            .env("MEMLAYER_DATA_DIR", self.data_path())
            .env("MEMLAYER_LOG", "warn")
            .current_dir(self.data_path());
        c
    }
}

impl Default for CliEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CliEnv {
    fn drop(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// TCP-mode daemon harness (TS-15 / SC-22 / SC-23).
///
/// Spawns the daemon binary with `MEMLAYER_LISTEN=tcp://127.0.0.1:<port>`,
/// generated TLS cert/key, and a pre-seeded `tokens.db` containing whatever
/// admin/non-admin entries the test wants. Writes ca.pem/server.pem/
/// server-key.pem under `<data_dir>/certs/`.
pub struct TcpDaemon {
    pub data_dir: tempfile::TempDir,
    pub port: u16,
    pub ca_pem: Vec<u8>,
    pub binary: PathBuf,
    daemon: Child,
}

/// Process-wide mutex guarding the TCP port pick + daemon bind in
/// `TcpDaemon::spawn`. Without this two parallel tests can race on the
/// same kernel-assigned ephemeral port: thread A picks port N, drops the
/// listener, thread B's `pick_port` happens to get N too, and only one
/// daemon ends up bound.
pub static TCP_SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl TcpDaemon {
    /// Open `tokens.db` in the env's data dir before the daemon starts so the
    /// auth interceptor sees the seeded entries on its first lookup.
    pub fn pre_seed_token(data_dir: &Path, name: &str, secret_hex: &str, is_admin: bool) {
        use rusqlite::params;
        use sha2::{Digest, Sha256};
        let db_path = data_dir.join("tokens.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open tokens.db");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS tokens (
                 name       TEXT PRIMARY KEY,
                 sha256_hex TEXT NOT NULL,
                 is_admin   INTEGER NOT NULL DEFAULT 0,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 revoked_at TEXT
             )",
            [],
        )
        .expect("create tokens table");
        let mut h = Sha256::new();
        h.update(secret_hex.as_bytes());
        let hex_hash = hex::encode(h.finalize());
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO tokens (name, sha256_hex, is_admin, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, hex_hash, is_admin as i64, now],
        )
        .expect("insert seed token");
    }

    /// Generate a self-signed CA + leaf cert and write them under
    /// `<data_dir>/certs/`. Returns the path to the cert and key.
    pub fn generate_certs(data_dir: &Path) -> (PathBuf, PathBuf, Vec<u8>) {
        use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
        let cert_dir = data_dir.join("certs");
        std::fs::create_dir_all(&cert_dir).expect("mkdir certs");

        let ca_key = KeyPair::generate().expect("ca key");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "memlayer-test-ca");
        let ca_cert = ca_params.self_signed(&ca_key).expect("ca self-sign");

        let leaf_key = KeyPair::generate().expect("leaf key");
        let mut leaf_params =
            CertificateParams::new(vec!["localhost".to_string()]).expect("leaf params");
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, "localhost");
        let leaf_cert = leaf_params
            .signed_by(&leaf_key, &ca_cert, &ca_key)
            .expect("leaf sign");

        let ca_pem = ca_cert.pem();
        let server_pem = leaf_cert.pem();
        let server_key_pem = leaf_key.serialize_pem();
        std::fs::write(cert_dir.join("ca.pem"), &ca_pem).unwrap();
        std::fs::write(cert_dir.join("server.pem"), &server_pem).unwrap();
        std::fs::write(cert_dir.join("server-key.pem"), &server_key_pem).unwrap();
        (
            cert_dir.join("server.pem"),
            cert_dir.join("server-key.pem"),
            ca_pem.into_bytes(),
        )
    }

    /// Pick an ephemeral port by binding briefly and reading the assigned
    /// port. The TCP listener is dropped before this returns; the daemon
    /// then re-binds the same port. There is a TOCTOU window where another
    /// thread could grab the port — callers should hold
    /// [`TCP_SPAWN_LOCK`] across pick + spawn to keep concurrent
    /// `TcpDaemon::spawn` calls from racing each other.
    pub fn pick_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        l.local_addr().expect("local addr").port()
    }

    /// Spawn the daemon in TCP+TLS mode. Caller must seed tokens.db before
    /// calling if the daemon needs to authenticate any requests.
    ///
    /// Acquires [`TCP_SPAWN_LOCK`] for the duration of the bind+spawn dance
    /// so two parallel tests can't accidentally pick the same kernel-assigned
    /// port (the bind-then-drop-then-rebind path is inherently TOCTOU). The
    /// lock is released as soon as the daemon successfully binds, so the
    /// rest of the test runs concurrently. Will retry up to 5 times if the
    /// daemon fails to bind within the readiness window.
    pub fn spawn(data_dir: tempfile::TempDir) -> Self {
        let binary = locate_binary();
        let (cert, key, ca_pem) = Self::generate_certs(data_dir.path());

        // Serialize the bind/spawn dance with any other TcpDaemon::spawn so
        // one test's pick_port can't race another's bind.
        let _guard = TCP_SPAWN_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        // Daemon stderr is mirrored to a file so that a bind failure or
        // startup panic shows up in the panic message below instead of
        // disappearing into an unread pipe. RUST_BACKTRACE=1 makes panics
        // fully visible.
        let stderr_path = data_dir.path().join("daemon.stderr");

        for attempt in 1..=5 {
            let port = Self::pick_port();
            let listen = format!("tcp://127.0.0.1:{port}");

            // Truncate the stderr log between attempts so the panic
            // message below reflects only the most recent attempt.
            let stderr_log = std::fs::File::create(&stderr_path)
                .expect("create daemon.stderr log");
            let daemon = Command::new(&binary)
                .args(["daemon", "start", "--foreground"])
                .env("MEMLAYER_DATA_DIR", data_dir.path())
                .env("MEMLAYER_LISTEN", &listen)
                .env("MEMLAYER_TLS_CERT", &cert)
                .env("MEMLAYER_TLS_KEY", &key)
                .env("MEMLAYER_LOG", "warn")
                .env("RUST_BACKTRACE", "1")
                .stdout(Stdio::null())
                .stderr(stderr_log)
                .spawn()
                .expect("spawn tcp daemon");

            // Wait for the TCP port to accept connections.
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut bound = false;
            while Instant::now() < deadline {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    bound = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }

            if bound {
                return TcpDaemon { data_dir, port, ca_pem, binary, daemon };
            }

            // Daemon never bound — likely the port got grabbed in the TOCTOU
            // window, or the daemon panicked on startup. Reap the failed
            // child and try again with a fresh port.
            let mut daemon = daemon;
            let _ = daemon.kill();
            let _ = daemon.wait();
            if attempt == 5 {
                let stderr = std::fs::read_to_string(&stderr_path)
                    .unwrap_or_else(|e| format!("(could not read daemon.stderr: {e})"));
                panic!(
                    "TCP daemon never bound after 5 attempts; last attempted port {port}\n\
                     daemon.stderr (last attempt):\n{stderr}",
                );
            }
        }
        unreachable!("loop above always returns or panics");
    }

    pub fn endpoint(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }
}

impl Drop for TcpDaemon {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// Helper for tests that need to spin up a session to satisfy the FK on
/// observations. Returns the session id used.
pub fn start_session(env: &CliEnv, session_id: &str) {
    let out = env
        .cmd()
        .args(["session", "start", session_id])
        .output()
        .expect("session start");
    assert!(
        out.status.success(),
        "session start failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
