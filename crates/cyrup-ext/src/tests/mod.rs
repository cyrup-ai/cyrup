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
mod bash_operations_seam;
mod branch_change;
mod capability_handle_ownership;
mod command_dispatch;
mod entry_renderer;
mod env_surface_records;
mod event_kind_lockstep;
mod ext_fail_closed;
mod extension_flag_diagnostics;
mod extension_name_conflicts;
mod failed_load_is_transactional;
mod live_provider;
mod loader;
mod loader_direct_file;
mod malformed_manifest;
mod manifest_cache;
mod native_ctx_state;
mod native_dispatch;
mod nested_tool_events;
#[cfg(feature = "wasm-host")]
mod overlay_keys;
mod payload_and_seam_parity;
mod post_baseline_events;
mod project_trust_shortcircuit;
mod provider;
#[cfg(feature = "wasm-host")]
mod provider_refresh;
mod quarantined_native;
mod registration_validation;
mod seam_liveness;
mod startup_timings;
mod terminal_keys;
mod tool_exposure;
mod trust_gate_order;
mod wasm_host;
#[cfg(feature = "wasm-host")]
mod wat_guest;
mod wit_world_sync;
