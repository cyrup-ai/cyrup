//! The Wasmtime runtime must come up when the process's address space is limited.
//!
//! The pooling allocator reserves its slabs of address space when the engine is built. Measured on
//! the real binary before the fallback: under `ulimit -v` / `RLIMIT_AS` of 2, 4, 8, 16, 32, 64, 128,
//! 256 and 384 GiB `cyrup -p` exited 1 with `wasm engine init failed: failed to create memory pool
//! mapping`, for every extension (the native `codemode`, `mcp` and `flux` among them, none of which
//! needs wasm), and `--no-extensions` did not help; at 512 GiB, 768 GiB and 1 TiB it started.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::ExtError;
use crate::host::engine::{build_engine_on_demand, build_engine_with_fallback};

fn pool_failure() -> ExtError {
    ExtError::Engine("failed to create memory pool mapping".to_string())
}

#[test]
fn a_pool_that_cannot_be_built_falls_back_to_the_on_demand_engine() {
    let engine = build_engine_with_fallback(|| Err(pool_failure()), build_engine_on_demand);
    assert!(engine.is_ok(), "{:?}", engine.err());
}

#[test]
fn a_pool_that_builds_is_used_and_the_fallback_is_not_tried() {
    let tried = AtomicBool::new(false);
    let engine = build_engine_with_fallback(build_engine_on_demand, || {
        tried.store(true, Ordering::SeqCst);
        build_engine_on_demand()
    });
    assert!(engine.is_ok());
    assert!(!tried.load(Ordering::SeqCst));
}

#[test]
fn when_both_engines_fail_the_error_names_both_causes() {
    let err = build_engine_with_fallback(
        || Err(pool_failure()),
        || Err(ExtError::Engine("on-demand refused".to_string())),
    )
    .expect_err("neither engine could be built");
    let text = err.to_string();
    assert!(
        text.contains("failed to create memory pool mapping"),
        "{text}"
    );
    assert!(text.contains("on-demand refused"), "{text}");
}

/// Set in the child process this test starts under a limited address space.
const CHILD_ENV: &str = "CYRUP_EXT_ENGINE_LIMITED_CHILD";

/// The whole path, not the seam: `WasmRuntime::new()` in a process whose address space is limited
/// to 8 GiB, where the pooling engine cannot be built (the message above is what it reported at
/// every limit up to 384 GiB). The limit applies to a child of this test, because `RLIMIT_AS` is
/// per process and lowering it here would take every other test in the binary down with it.
#[cfg(unix)]
#[tokio::test]
async fn the_wasm_runtime_starts_under_a_limited_address_space() {
    if std::env::var_os(CHILD_ENV).is_some() {
        // The child: it is already limited. The pool must be out of reach, or this proves nothing.
        assert!(
            crate::host::build_engine().is_err(),
            "the limit did not stop the pooling engine; the test would pass without the fallback"
        );
        let runtime = crate::WasmRuntime::new();
        assert!(runtime.is_ok(), "{:?}", runtime.err());
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"ulimit -v 8388608 && exec "$0" --exact "$1" --nocapture"#)
        .arg(&exe)
        .arg("tests::engine_address_space::the_wasm_runtime_starts_under_a_limited_address_space")
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the limited child failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
