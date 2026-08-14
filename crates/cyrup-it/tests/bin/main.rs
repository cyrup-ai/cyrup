//! Seam tests drained from **`crates/cyrup` (9), `cyrup-sdk` (4), `cyrup-tui` (1),
//! `cyrup-provider` (1)** — 15 files.
//!
//! SCOPE, and why it is wider than the name. This target began as "the `cyrup` binary's own seam"
//! and still is, mostly: `one_shot_parity`, `signal_shutdown`, `tui_mode_flag`, `unknown_flag_exit`,
//! `extension_load_failure_exit`, `piped_stdin_trim`, `auth_credential_print`,
//! `list_models_overlay` each spawn the real `cyrup` executable and assert on what a shell would
//! see, and there is no in-process form of that assertion. The four `cyrup-sdk` files, the one
//! `cyrup-tui` file and the one `cyrup-provider` file landed here — rather than in `misc` — because
//! the migration is parallelised BY SOURCE CRATE and one agent owned all five of those crates;
//! putting them in `misc` would have put two agents in one directory at once. Nothing about them is
//! `cyrup`-binary-specific, and they can be lifted into `misc` (or into a `sdk` target of their own,
//! once `cyrup-sdk` crosses ~5 seam files) with a `git mv` and a line moved between two `main.rs`
//! files. See `unresolved` in the migration report.
//!
//! What each non-`cyrup` file is here FOR, since none of them spawn `cyrup`:
//!
//! * `embedder_seams`, `embedding`, `lifecycle`, `runtime` — `cyrup-sdk`'s public-API surface. The
//!   design (§7) keeps these EXTERNAL on purpose: they must see the crate exactly as an embedder
//!   does, through its `pub` surface, which a `#[cfg(test)]` module inside `cyrup-sdk` cannot do
//!   (it can reach private items, so it cannot prove the surface is complete). `embedder_seams`
//!   also opens a real loopback SSE server, which is a socket seam in its own right.
//! * `wasm_renderer_screen` — loads a LIVE `wasm32-wasip2` guest and asserts on terminal cells.
//! * `faux_not_in_normal_build` — shells out to `cargo tree` and asserts on the resolved feature
//!   graph. PROV-052's RED→GREEN guard; the instrument is a real subprocess.
//!
//! Migration notes:
//!
//! * `support::bins::cyrup()` replaces `env!("CARGO_BIN_EXE_cyrup")` at all 8 spawn sites, which
//!   stops compiling the moment the file leaves the `cyrup` package.
//! * !!! THE `faux` FEATURE !!! Five of these files drive a whole offline turn with
//!   `--model faux/faux-1`, which is selectable in the binary ONLY when the `cyrup` package's own
//!   `faux` feature is on (`crates/cyrup/src/provider.rs`'s `#[cfg(feature = "faux")]` arm). In
//!   `crates/cyrup/tests/` that was free: cargo resolves dev-dependencies for a test build, and
//!   `crates/cyrup/Cargo.toml`'s self-dev-dependency `cyrup = { path = ".", features = ["faux"] }`
//!   turned it on for exactly that build. `build.rs` here runs a NON-dev `cargo build -p cyrup`,
//!   which does not resolve dev-dependencies, so the feature had to be requested explicitly — it is,
//!   in `build.rs`'s `BINS` table. The same trap is live for the documented
//!   `CYRUP_IT_BIN_DIR="$PWD/target/debug"` shortcut: a plain `cargo build --workspace --bins`
//!   produces a `cyrup` WITHOUT `faux`, and these five tests then fail with pi's
//!   `formatNoModelsAvailableMessage()` instead of the faux transcript. Build the override binary as
//!   `cargo build -p cyrup --features faux --bin cyrup` (PROV-052 is not weakened: that is a
//!   private, test-only build, and the shipped-graph invariant `faux_not_in_normal_build` asserts is
//!   about `cargo tree -p cyrup --edges normal`, which no feature request here touches).
//! * The eight hand-rolled hermetic-child builders were deliberately NOT collapsed into
//!   `support::scratch::Scratch::command`. They are not interchangeable: `Scratch` puts `HOME` at
//!   `<root>/home` and always sets `CYRUP_HOME`, while every builder below sets `HOME` to the temp
//!   ROOT (the parent of `agent/`) and sets no `CYRUP_HOME` at all — so swapping them changes which
//!   directory the binary under test resolves its config from. That is a behaviour rewrite, not an
//!   import rewrite, and the migration brief forbids rewriting a test body. The collapse is still
//!   worth doing; it is a separate, deliberate change with its own verification, and
//!   `auth_credential_print`'s `env_clear` + allowlist (NOT a denylist of `*_API_KEY` names) is the
//!   shape to collapse ONTO, because the failure being guarded against is the binary falling through
//!   to a real agent session on whichever provider key happens to be exported.
//! * `piped_stdin_trim` is the file that hung the whole suite: a detached `__intercom-broker`
//!   grandchild inherited a harness pipe FD above 2, and `wait_with_output()` reads to EOF rather
//!   than to child exit. Its stdio handling is byte-for-byte as the fix left it, and
//!   `.config/nextest.toml`'s `leak-timeout` catches any recurrence by name.
//! * `signal_shutdown`'s real-signal waits are legitimately real-time; they are on the §5.4 KEEP
//!   list. Do not "fix" them with `tokio::time::pause`, which cannot help when the wait is on an
//!   OS event.
//! * Four files from these crates did NOT move and must not be moved casually — see
//!   `stayedInPlace` in the migration report. All four mutate the PROCESS environment
//!   (`std::env::set_var`, `unsafe` since edition 2024) and each one's soundness argument, written
//!   in its own module doc, is *"this is the only `#[test]` in this binary"*. Consolidating them
//!   into this 15-file binary destroys that argument outright.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

// ---- crates/cyrup: the shipped binary's argv / exit-code / stdio seam -------------------------
mod auth_credential_print;
mod extension_load_failure_exit;
mod list_models_overlay;
mod one_shot_parity;
mod package_update_check;
mod piped_stdin_trim;
/// Real `SIGTERM`/`SIGHUP` delivery. The predicate was this file's own `#![cfg(unix)]` before the
/// move; it lives here now so the inventory shows what is conditional.
#[cfg(unix)]
mod signal_shutdown;
mod tui_mode_flag;
mod unknown_flag_exit;

// ---- crates/cyrup-sdk: the embedder-facing public surface ------------------------------------
mod embedder_seams;
mod embedding;
mod lifecycle;
mod runtime;

// ---- crates/cyrup-tui: a live wasm32-wasip2 guest, drawn to terminal cells --------------------
mod wasm_renderer_screen;

// ---- crates/cyrup-provider: the PROV-052 Cargo-feature-graph guard ----------------------------
mod faux_not_in_normal_build;

// ==================================================================================================
// §4 R5, layer 3 — the ambient-environment guards.
//
// Layers 1 and 2 (hermetic children, injected config) cannot give you this: they make each CHILD
// safe, but say nothing about the harness process the tests run in. These two turn "a test quietly
// used a real API" into a named red at the top of the run instead of a surprise on an invoice.
// They are the only two `#[test]`s in this target that were not drained from a source crate.
//
// If they red on your machine, that is the guard working: `unset TOGETHER_API_KEY` (etc.) and
// re-run. Deleting them is a two-line change, but do it deliberately — `TOGETHER_API_KEY` being
// exported on the maintainer's box has ALREADY caused a test in this workspace to make a real
// network call, and an ambient `CYRUP_INTERCOM=1` has already leaked 13 broker processes out of a
// single run.
// ==================================================================================================

#[test]
fn no_ambient_provider_credentials() {
    support::env::assert_no_ambient_provider_credentials();
}

#[test]
fn no_ambient_feature_gates() {
    support::env::assert_no_ambient_feature_gates();
}
