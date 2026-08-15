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
//! **This file mutates the process environment**, so it is its own test binary with ONE `#[test]`:
//! `std::env::set_var` is process-global and Rust 2024 makes it `unsafe` precisely because a sibling
//! test running on another thread would see the change. Same structure as
//! `tests/experimental_marker.rs`.

use cyrup_tui::share_viewer_url;

/// Set (or clear) an env var. Sound here because this binary runs exactly one test, single-threaded
/// with respect to any other reader of this variable.
fn set_env(key: &str, value: Option<&str>) {
    // SAFETY: single-test binary; no other thread reads or writes the environment concurrently.
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
