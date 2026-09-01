#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
//! TUI-063 — `CYRUP_SHARE_VIEWER_URL` actually reaches `/share`'s viewer link.
//!
//! ```ts
//! // pi v0.83.0 coding-agent/src/config.ts:502-508
//! const DEFAULT_SHARE_VIEWER_URL = "https://pi.dev/session/";
//! export function getShareViewerUrl(gistId: string): string {
//!     const baseUrl = process.env.PI_SHARE_VIEWER_URL || DEFAULT_SHARE_VIEWER_URL;
//!     return `${baseUrl}#${gistId}`;
//! }
//! ```
//!
//! `cyrup --help` advertises `CYRUP_SHARE_VIEWER_URL - Base URL for /share command`
//! (`crates/cyrup/src/cli.rs:1077`), and before this landed a grep over `crates/` returned that help
//! line and **nothing else**: `/share` printed the raw gist URL, so setting the variable produced no
//! effect and no diagnostic. This asserts the *reader* exists, which the pure
//! `share_viewer_url_from` unit tests in `src/app.rs` cannot.
//!
//! # Soundness: the criterion is the THREAD, not the binary
//!
//! `std::env::set_var` is `unsafe` in Rust 2024 because it races ANY concurrent `getenv` in the
//! process — not only a reader looking for the same key, and not only a sibling *test*. So the
//! condition is that nothing else in this process is running: this file holds exactly one
//! `#[test]`, which spawns no threads and starts no runtime.
//!
//! "It is its own test binary" is the weaker claim and it is not sufficient on its own — a binary
//! can hold two tests, and this workspace has already had one where consolidation silently voided
//! that argument. If a second test is ever added here, this mutation stops being sound, and no
//! lock fixes it.
//!
//! Same structure as `tests/experimental_marker.rs`.

// The workspace `clippy.toml` disallows process-env mutation; this file is one of the few places it
// is correct. Its whole subject is the thin env-READING wrapper over an injectable core
// (`share_viewer_url_from`), and that wrapper is exactly what an injected test cannot cover: a typo in the variable
// NAME inside it would pass every `_from` test in the workspace. One real-environment proof per
// wrapper, in a binary that holds one test, is the cheapest way to close that gap.
#![allow(clippy::disallowed_methods)]

use cyrup_tui::share_viewer_url;

/// Set (or clear) an env var.
///
/// SAFETY: this binary holds exactly one `#[test]`, which spawns no threads and starts no runtime,
/// so nothing in this process runs concurrently with the mutation. That is the condition `set_var`
/// requires — it races any concurrent `getenv` for any key, not merely a reader of this one.
fn set_env(key: &str, value: Option<&str>) {
    // SAFETY: the criterion is the THREAD, not the binary. `set_var` races any concurrent `getenv`
    // for ANY key — not merely a reader of this one — so it is sound only when nothing else in the
    // process is running. This binary holds one `#[test]`, which spawns no threads and starts no
    // runtime, so no other thread exists to race it. Adding a second test here re-creates the
    // race, and no lock fixes it.
    unsafe {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

#[test]
fn share_viewer_url_reads_cyrup_share_viewer_url_from_the_process_environment() {
    const VAR: &str = "CYRUP_SHARE_VIEWER_URL";

    set_env(VAR, None);
    assert_eq!(
        share_viewer_url("abc123"),
        "https://pi.dev/session/#abc123",
        "unset falls back to pi's `DEFAULT_SHARE_VIEWER_URL` (`config.ts:502`)"
    );

    set_env(VAR, Some("https://viewer.example/s/"));
    assert_eq!(
        share_viewer_url("abc123"),
        "https://viewer.example/s/#abc123",
        "the advertised variable must reach the link `/share` renders — this is the assertion that \
         fails if the reader is ever deleted again"
    );

    // JS `||` treats "" as unset (`config.ts:506`).
    set_env(VAR, Some(""));
    assert_eq!(share_viewer_url("abc123"), "https://pi.dev/session/#abc123");

    set_env(VAR, None);
}
