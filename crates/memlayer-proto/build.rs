// Generated stubs for `memlayer.proto`.
//
// `protoc` is resolved via `PROTOC` if set, otherwise from PATH. Debian/Ubuntu
// LTS still ship protobuf 3.12.x, which requires
// `--experimental_allow_proto3_optional` for proto3 `optional` fields. Newer
// protoc (≥ 3.15, and the post-3.21 `22+` scheme) accepts `optional` natively
// and may reject that flag — so we only pass it when needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_path = "../../proto/memlayer.proto";
    let proto_dir = "../../proto";
    println!("cargo:rerun-if-changed={proto_path}");
    println!("cargo:rerun-if-env-changed=PROTOC");

    let protoc = resolve_protoc()?;
    eprintln!(
        "memlayer-proto: using protoc {} ({})",
        protoc_version_string(&protoc).unwrap_or_else(|| "unknown".into()),
        protoc.display()
    );

    let mut builder = tonic_build::configure()
        .build_server(true)
        .build_client(true);

    if needs_proto3_optional_flag(&protoc_version_string(&protoc).unwrap_or_default()) {
        builder = builder.protoc_arg("--experimental_allow_proto3_optional");
    }

    builder.compile_protos(&[proto_path], &[proto_dir])?;
    Ok(())
}

fn resolve_protoc() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    if let Some(from_env) = std::env::var_os("PROTOC") {
        let path = std::path::PathBuf::from(from_env);
        if path.is_file() || which_ok(&path) {
            return Ok(path);
        }
        return Err(format!(
            "PROTOC is set to '{}', but that binary was not found. \
             Install protobuf-compiler or point PROTOC at a working protoc.",
            path.display()
        )
        .into());
    }

    which("protoc").ok_or_else(|| {
        "protoc not found on PATH. Install protobuf-compiler \
         (apt: protobuf-compiler, brew: protobuf) or set PROTOC. \
         Tip: run `make prereqs`."
            .into()
    })
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if which_ok(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn which_ok(path: &std::path::Path) -> bool {
    // Accept absolute paths and bare names resolved via PATH (e.g. PROTOC=protoc).
    std::process::Command::new(path)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn protoc_version_string(protoc: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(protoc)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // Formats: "libprotoc 3.12.4" or "libprotoc 25.1"
    String::from_utf8(output.stdout)
        .ok()?
        .split_whitespace()
        .nth(1)
        .map(str::to_owned)
}

/// Return true when `protoc` is older than 3.15 (needs the experimental flag).
fn needs_proto3_optional_flag(version: &str) -> bool {
    let mut parts = version.split('.');
    let major: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    // After 3.21 the project switched to 22+ (no leading 3.).
    match major {
        0..=2 => true,
        3 => minor < 15,
        _ => false,
    }
}
