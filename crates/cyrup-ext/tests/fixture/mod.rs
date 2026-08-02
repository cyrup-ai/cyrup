//! Shared helper for the live-guest COMPONENT tests: build (or locate) the `cyrup-ext-sdk` demo
//! extension as a `wasm32-wasip2` component. Mirrors `tests/wasm_component.rs`'s fixture builder;
//! set `CYRUP_EXT_FIXTURE_COMPONENT` to a prebuilt component to skip the nested build.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use cyrup_ext::{ExtMode, HostConfig};
use std::path::PathBuf;
use std::process::Command;

pub fn component() -> PathBuf {
    if let Ok(p) = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // A dedicated target dir so this nested build never contends with the outer workspace lock.
    let build_dir = std::env::temp_dir().join("cyrup-ext-fixture-target");
    let status = Command::new(&cargo)
        .args(["build", "-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2", "--target-dir"])
        .arg(&build_dir)
        .status()
        .expect("spawn cargo to build the wasm32-wasip2 fixture component");
    assert!(status.success(), "building cyrup-ext-sdk fixture component failed");

    let wasm = build_dir.join("wasm32-wasip2/debug/cyrup_ext_sdk.wasm");
    assert!(wasm.exists(), "fixture component not found at {}", wasm.display());
    wasm
}

pub fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: PathBuf::from(".") }
}
