//! Seam tests drained from **`crates/cyrup-ext`** (13 files) and **`crates/cyrup-ext-sdk`** (1).
//!
//! What makes a test belong here: it loads a LIVE `wasm32-wasip2` guest component into the
//! Wasmtime Component Model host and dispatches real events across the boundary — the arch-08b
//! headline proof, plus discovery/loading, manifest capabilities, guest host-mode, and the wasm
//! provider path.
//!
//! Migration notes:
//!
//! * `support::bins::component()` / `component_bytes()` replace the 13 copies of
//!   `fixture_component()` in this crate. The nested
//!   `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` is gone from every one of them —
//!   `build.rs` does it once for the whole suite. Four of those copies (`discover_load.rs:25`,
//!   `guest_host_mode.rs:36`, `manifest_capabilities.rs:39`, `wasm_provider.rs:25`) passed no
//!   `--target-dir` at all and contended for the WORKSPACE build lock, which is the exact
//!   contention their eight siblings were written to avoid. THE ONE EXCEPTION is
//!   [`build_tier1`], whose subject under test IS the production build loop
//!   (`cyrup_ext::build::build_component_in`); its cargo invocation is the thing being asserted on,
//!   not fixture scaffolding, so it stays. See that module's own note.
//! * `CYRUP_EXT_FIXTURE_COMPONENT` still works as the escape hatch — now read in `build.rs`, one
//!   place instead of 22.
//! * `tests/fixture/mod.rs` became [`fixture`] in this directory: same `component()`/`cfg()`
//!   surface, so its four users only had `mod fixture;` rewritten to `use crate::fixture;`.
//! * **`#![cfg(feature = "wasm-host")]` was REMOVED from all 12 files that carried it.**
//!   `wasm-host` is `cyrup-ext`'s feature, not this crate's; `cyrup-it` declares a `wasm-host`
//!   feature that only forwards, and it is deliberately NOT in this target's `required-features`
//!   (see the `[[test]]` entry in Cargo.toml). Left in place, that attribute would evaluate
//!   against `cyrup-it`'s own default-off flag and silently compile every test in this target to
//!   nothing under `--features it` — the invisible skip the gate exists to prevent. `cyrup-ext`
//!   enables `wasm-host` in its own `default` (`crates/cyrup-ext/Cargo.toml:74`), so the host path
//!   is on regardless. This is the one edit here that is not a pure move; no assertion changed.
//! * This target is gated on `it` only, NOT `it, wasm-host` — for the reason above.
//!
//! NOT yet done, and deliberately not done as part of a relocation: `discover_load::temp_project`
//! and `manifest_capabilities::temp_project` still mint a nanosecond-stamped directory under
//! `std::env::temp_dir()` and clean it up with a best-effort `remove_dir_all` at the end of each
//! test. Converting them to `support::scratch::Scratch` means each test must hold the guard for
//! its own lifetime, which restructures every test body — beyond a mechanical rewrite.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

/// The live-guest component + `HostConfig` helper (was `crates/cyrup-ext/tests/fixture/mod.rs`).
mod fixture;

mod abi_fingerprint_invalidation;
mod build_tier1;
mod discover_load;
mod guest_host_mode;
mod manifest_capabilities;
mod wasm_bus_flag;
mod wasm_component;
mod wasm_ctx_state;
mod wasm_dispatch;
mod wasm_dynamic_tools;
mod wasm_provider;
mod wasm_renderer_routing;
mod wasm_thinking_level;
mod wasm_tool_result_usage;

/// §4 R5 layer 3: the suite's own process must not carry provider credentials into a run.
#[test]
fn no_ambient_provider_credentials() {
    support::env::assert_no_ambient_provider_credentials();
}
