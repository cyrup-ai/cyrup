//! Seam tests drained from **`crates/cyrup-session-svc`** — 11 files.
//!
//! What makes a test belong here: an assembled `AgentSession` that loads a LIVE wasm extension and
//! routes guest-registered slash commands through the real run path
//! (`_tryExecuteExtensionCommand`, agent-session.ts:1148-1172), plus the guest HTTP capability
//! tests that stand up a loopback server.
//!
//! Migration notes:
//!
//! * The 10 `fixture_component()` copies in this crate all built into ONE fixed path,
//!   `std::env::temp_dir()/cyrup-session-svc-fixture-target`, so they serialized on each other's
//!   cargo build lock and never cleaned up. `support::bins::component()` replaces all ten.
//! * `install_noop.rs` splits at the module boundary: test 1 is pure and belongs in `src/`, only
//!   `mod wasm_ext` belongs here. Take the module, not the file.
//! * 43 hand-rolled `fixture()`s and 41 `base_config()`s live in this crate's old test files. They
//!   belong in `cyrup-test-support`, NOT in this target's `mod support` — a source crate's
//!   `#[cfg(test)]` module can use them there and cannot use them here.
//! * Servers bind `:0` and read the assignment back (`wasm_http.rs:67` already does); no fixed
//!   ports (§4 R4).
//! * Gated on `it` only, not `it, wasm-host` — see the `[[test]]` note in Cargo.toml.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

// ---------------------------------------------------------------------------------------------
// The drained files. One module per source file, same name, so `git log --follow` and every
// in-repo reference to `crates/cyrup-session-svc/tests/<name>.rs` still leads somewhere obvious.
//
// NOTE ON `#![cfg(feature = "wasm-host")]`: nine of these files opened with it and none of them
// does now. It named cyrup-session-svc's OWN feature, which that crate turns on in its `default`,
// so it was unconditionally true. Re-spelled here it would name *cyrup-it's* `wasm-host`, which
// `--features it` does NOT enable — so the tests would compile away to nothing and the target
// would report a cheerful green having run none of them. That is the exact invisible skip the
// gate exists to prevent; see the `[[test]]` comment in crates/cyrup-it/Cargo.toml.
// ---------------------------------------------------------------------------------------------

mod gap11_event_tier_verify;
mod install_noop;
mod late_tools;
mod model_registry;
mod wasm_active_tools;
mod wasm_compaction_override;
mod wasm_exec;
mod wasm_http;
mod wasm_proc;
mod wasm_slash_command;
mod wasm_ui_dialogs;
