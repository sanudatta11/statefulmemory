// Generated stubs for `memlayer.proto`.
//
// `protoc` is not invoked here directly: tonic-build (since 0.12) calls into
// `prost-build`, which in turn shells out to a `protoc` resolved via the
// `PROTOC` env var or the `protoc-bin-vendored` crate. We use whatever is
// pre-installed on the build host (CI installs `protobuf-compiler`).

fn main() -> std::io::Result<()> {
    let proto_path = "../../proto/memlayer.proto";
    let proto_dir = "../../proto";
    println!("cargo:rerun-if-changed={proto_path}");

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&[proto_path], &[proto_dir])?;
    Ok(())
}
