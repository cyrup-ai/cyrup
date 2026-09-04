#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
//! C15 of `docs/audits/2026-08-09-tui-presentation-fidelity.md` §3C — the footer's `• xp` experimental marker.
//!
//! ```ts
//! // pi v0.84.1 coding-agent/src/modes/interactive/components/footer.ts:162-164
//! if (areExperimentalFeaturesEnabled()) {
//!     statsParts.push(`${theme.fg("dim", "•")} ${theme.bold(theme.fg("warning", "xp"))}`);
//! }
//! ```
//! ```ts
//! // pi v0.84.1 coding-agent/src/core/experimental.ts:1-3
//! export function areExperimentalFeaturesEnabled(): boolean {
//!     return process.env.PI_EXPERIMENTAL === "1";
//! }
//! ```
//!
//! cyrup had the right SHAPE (`status.rs` built the two segments) but `set_experimental` had no
//! production caller — `grep` found only a test — so the marker was unreachable however the user
//! launched. `AppState::new` now answers the predicate, which is the port of upstream reading
//! `process.env` inside `render()`.
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

// The workspace `clippy.toml` disallows process-env mutation; this file is one of the few places it
// is correct. Its whole subject is the thin env-READING wrapper over an injectable core
// (`experimental_features_enabled_from`), and that wrapper is exactly what an injected test cannot cover: a typo in the variable
// NAME inside it would pass every `_from` test in the workspace. One real-environment proof per
// wrapper, in a binary that holds one test, is the cheapest way to close that gap.
#![allow(clippy::disallowed_methods)]

use cyrup_tui::{App, UiTheme, experimental_features_enabled, experimental_features_enabled_from};
use ratatui::backend::TestBackend;

/// Set (or clear) an env var. Sound here because this binary runs exactly one test, single-threaded
/// with respect to any other reader of these two variables.
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

fn footer_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    (0..buf.area.height)
        .map(|y| -> String {
            (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)))
                .map(|c| c.symbol())
                .collect()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The footer's line 2 — the `statsParts` row (`footer.ts:166`), identified by the `no-model`
/// right cluster it always carries when nothing has set a model.
fn stats_row(app: &App<TestBackend>) -> String {
    footer_text(app)
        .lines()
        .find(|l| l.contains("no-model"))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no stats row:\n{}", footer_text(app)))
}

#[test]
fn xp_marker_appears_when_experimental_features_are_enabled() {
    let restore_cyrup = std::env::var("CYRUP_EXPERIMENTAL").ok();
    let restore_pi = std::env::var("PI_EXPERIMENTAL").ok();

    // --- the pure predicate: `=== "1"`, nothing else counts (`experimental.ts:2`) -------------
    let env = |c: Option<&'static str>, p: Option<&'static str>| {
        move |k: &str| match k {
            "CYRUP_EXPERIMENTAL" => c.map(str::to_string),
            "PI_EXPERIMENTAL" => p.map(str::to_string),
            _ => None,
        }
    };
    assert!(experimental_features_enabled_from(env(Some("1"), None)));
    assert!(
        experimental_features_enabled_from(env(None, Some("1"))),
        "PI_* survives as fallback"
    );
    assert!(!experimental_features_enabled_from(env(None, None)));
    assert!(!experimental_features_enabled_from(env(Some("0"), None)));
    assert!(
        !experimental_features_enabled_from(env(Some("true"), None)),
        "only the literal `1`"
    );

    // --- the wiring: launching with the flag set must reach the footer ------------------------
    set_env("PI_EXPERIMENTAL", None);
    set_env("CYRUP_EXPERIMENTAL", Some("1"));
    assert!(experimental_features_enabled(), "sanity: the env is armed");

    let mut app = App::new(TestBackend::new(100, 12), UiTheme::dark()).unwrap();
    assert!(
        app.state().status.experimental,
        "`AppState::new` must answer `areExperimentalFeaturesEnabled()` — before this fix nothing \
         in production ever called `set_experimental`, so the marker was unreachable"
    );
    app.draw().unwrap();
    let text = footer_text(&app);
    assert!(
        text.contains("• xp"),
        "the `• xp` marker must be on the footer's line 2:\n{text}"
    );
    assert!(
        stats_row(&app).contains("• xp"),
        "…on the stats row specifically:\n{text}"
    );

    // --- MIRROR: with the flag off the marker stays off ---------------------------------------
    set_env("CYRUP_EXPERIMENTAL", None);
    let mut off = App::new(TestBackend::new(100, 12), UiTheme::dark()).unwrap();
    assert!(!off.state().status.experimental);
    off.draw().unwrap();
    let off_text = footer_text(&off);
    // Scoped to the stats row (`footer.ts:166` `statsParts.join(" ")`) rather than the whole
    // screen: the startup block's closing `onboarding` line — "Cyrup can e**xp**lain its own
    // features…" (`interactive-mode.ts:947-950`) — contains the substring `xp`, so a whole-buffer
    // search cannot distinguish the marker from ordinary prose. The row under test is unchanged.
    assert!(
        !stats_row(&off).contains("xp"),
        "no marker without the flag:\n{off_text}"
    );
    assert!(
        !off_text.contains("• xp"),
        "…and the marker glyph pair appears nowhere:\n{off_text}"
    );
    // …and the rest of the footer is untouched — the context segment still renders (C1).
    assert!(
        off_text.contains("0.0%/0"),
        "the other segments are unaffected:\n{off_text}"
    );

    set_env("CYRUP_EXPERIMENTAL", restore_cyrup.as_deref());
    set_env("PI_EXPERIMENTAL", restore_pi.as_deref());
}
