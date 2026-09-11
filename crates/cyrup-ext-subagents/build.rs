//! Builds the V8 startup snapshot for the `workflowScript` runtime (WORKFLOW_1 §7).
//!
//! The extension (ops + the `extension!` declaration + `js/prelude.js`) lives in the
//! `cyrup-workflow-runtime` crate, not in this one: a build script can never import from the crate
//! whose build it is running, and that extension needs this crate's own internals
//! (`RunShared`, `LaunchEnvelope`, …) to do anything useful, so it cannot live here either. What
//! CAN live here is the call to `cyrup_workflow_runtime::build_snapshot()`, because that crate is
//! an ordinary, non-circular `[build-dependencies]` edge of this one — see that crate's module doc
//! for the full reasoning.
//!
//! `src/workflows/scripted/engine.rs` `include_bytes!`s the result back via `OUT_DIR`.

#![allow(clippy::expect_used, clippy::panic)] // a build script's only failure channel is a panic

fn main() {
    // Cargo already reruns this build script whenever `cyrup-workflow-runtime` (a
    // `[build-dependencies]` edge) changes, via its own unit-graph fingerprinting — that is
    // independent of `rerun-if-changed`, which exists only for file-system inputs a build script
    // reads directly. This build script reads none, so the only line needed is for itself.
    println!("cargo:rerun-if-changed=build.rs");

    let snapshot = cyrup_workflow_runtime::build_snapshot().unwrap_or_else(|error| {
        panic!("cyrup-ext-subagents build.rs: workflow snapshot build failed: {error:?}")
    });

    let out_dir = std::env::var_os("OUT_DIR").expect("cargo always sets OUT_DIR for build scripts");
    let out_path = std::path::Path::new(&out_dir).join("workflow.snapshot");
    std::fs::write(&out_path, snapshot).unwrap_or_else(|error| {
        panic!(
            "cyrup-ext-subagents build.rs: writing {}: {error}",
            out_path.display()
        )
    });
}
