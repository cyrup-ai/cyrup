//! In-crate unit tests (relocated from `crates/cyrup-ext/tests/`).
//!
//! Cargo compiles every file under `tests/` into its own integration-test BINARY and process;
//! at 310 such files workspace-wide the harness overhead dwarfed the ~2.4 minutes of actual test
//! execution. Every test here is in-process and needs no seam — no spawned binary, no
//! `CARGO_BIN_EXE_*`, no built wasm artifact — so it lives with the library it exercises and
//! compiles under `cargo check -p cyrup-ext --all-targets`.
//!
//! The genuinely seam-touching files (a live wasm guest built by a nested `cargo build
//! --target wasm32-wasip2`) stay in `crates/cyrup-ext/tests/`.
//!
//! Assertions are unchanged from the integration-test originals; only the crate self-reference
//! moved (`cyrup_ext::X` -> `crate::X`).

mod aggregation;
mod capability_handle_ownership;
mod command_dispatch;
mod entry_renderer;
mod env_surface_records;
mod ext_fail_closed;
mod extension_flag_diagnostics;
mod extension_name_conflicts;
mod loader;
mod loader_direct_file;
mod malformed_manifest;
mod manifest_cache;
mod native_ctx_state;
mod payload_and_seam_parity;
mod native_dispatch;
mod project_trust_shortcircuit;
mod seam_liveness;
mod trust_gate_order;
mod provider;
mod wasm_host;
mod wit_world_sync;
