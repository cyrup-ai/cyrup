//! cyrup — the CLI binary (arch-11; conformance: func-11). The sole `anyhow` boundary and the
//! only binary in the workspace.
//!
//! Scaffold: the real entrypoint is `#[tokio::main(flavor = "multi_thread")]` with clap arg
//! parsing, mode selection (R-11-001), service wiring, and OS signal handling (arch-11 §3/§5).
//! This placeholder keeps the workspace producing a runnable binary.

fn main() {
    println!(
        "cyrup {} — scaffold. See spec/architecture/arch-11-runtime-modes-and-sdk.md.",
        env!("CARGO_PKG_VERSION")
    );
}
