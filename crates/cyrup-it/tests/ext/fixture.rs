//! The live-guest COMPONENT fixture, shared by the modules in this target that were written
//! against `crates/cyrup-ext/tests/fixture/mod.rs`.
//!
//! It kept its name and its two functions so the four call sites (`wasm_ctx_state`,
//! `wasm_dynamic_tools`, `wasm_renderer_routing`, `wasm_tool_result_usage`) are unchanged apart
//! from `mod fixture;` becoming `use crate::fixture;`. What changed is [`component`]: the nested
//! `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into a fixed, never-cleaned
//! `std::env::temp_dir()/cyrup-ext-fixture-target` is gone. `crates/cyrup-it/build.rs` does that
//! build once for the entire suite, into `$OUT_DIR`, and exports the path as `CYRUP_IT_COMPONENT`;
//! `CYRUP_EXT_FIXTURE_COMPONENT` still overrides it, now honoured in one place instead of thirteen.
//!
//! This stayed a target-local module rather than moving to `tests/support/`: [`cfg`] is a
//! `cyrup_ext::HostConfig` constructor, which is specific to this crate's seam and would not be
//! usable from any other target.

use cyrup_ext::{ExtMode, HostConfig};
use std::path::PathBuf;

pub fn component() -> PathBuf {
    crate::support::bins::component()
}

pub fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: PathBuf::from(".") }
}
