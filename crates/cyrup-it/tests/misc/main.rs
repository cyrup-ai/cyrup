//! The tails: seam tests from the five crates with too few to earn a target of their own —
//! **`cyrup-sdk` (4), `cyrup-tui` (2), `cyrup-tools` (2), `cyrup-provider` (1),
//! `cyrup-ext-sdk` (1)**. 10 files.
//!
//! Representative members: `cyrup-sdk`'s embedder seams (loopback server), `cyrup-tui`'s
//! `wasm_renderer_screen` (live guest) and `native_shift_enter`, `cyrup-tools`' `build_tier1` and
//! `package_update_check` (real `cargo`/`git` subprocesses), `cyrup-provider`'s `remote_catalog`.
//!
//! **CURATION NOTE — read before adding anything.** `misc` is where a test suite goes to rot.
//! ripgrep annotates its own misc module with a note to stop adding to it; this is that note. The
//! bar for landing here is: the file is a genuine seam test AND its owning crate has fewer than
//! ~5 of them. The moment a crate crosses that line, it gets its own `[[test]]` target in
//! `Cargo.toml` and its files move out — cheap to do, and it keeps a segfault in one crate's seam
//! from taking the other four down with no report (§4 R6).
//!
//! Migration notes:
//!
//! * `build_tier1.rs:13-17` currently returns GREEN when the wasm toolchain is absent — a pass
//!   that proves nothing. `build.rs` now hard-fails with an actionable message instead; when this
//!   file lands, delete the skip rather than porting it.
//! * `cyrup-tui/src/native_modifiers.rs:62`'s `set_native_modifier_probe` is a first-writer-wins
//!   global asserted `.is_none()`, so `native_shift_enter` must remain the ONLY setter in whatever
//!   binary it lands in. If a second setter ever appears here, that test earns its own target and
//!   says why in its module doc — the wasmtime `rlimited-memory.rs` precedent.
//! * `cyrup-tools`' tests that shell out get `support::env::scrub` (they need the developer's
//!   `PATH` and toolchain), not the full `hermetic` clear.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;
