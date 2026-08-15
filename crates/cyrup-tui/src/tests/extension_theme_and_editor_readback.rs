//! SEAM-T01 / SEAM-T02 — the two interactive READ-BACK seams `LiveHostServices` never overrode.
//!
//! `LiveHostServices` is the only production `HostServices` backend, and it implemented none of
//! `editor_text`, `theme`, `theme_list`, `theme_by_name` or `set_theme`. Each therefore took its
//! trait default — `""`, `None`, `[]`, `None`, `Err(..)` — in EVERY mode, with no error and no log
//! line, and `cyrup-ext/src/host/live.rs` forwards `get-editor-text`, `theme-get`, `theme-get-json`,
//! `theme-list`, `theme-get-by-name` and `theme-set` to those same trait methods, so every WASM
//! guest inherited the same answers.
//!
//! Those defaults are RIGHT for headless and RPC — they are pi's `noOpUIContext`
//! (`core/extensions/runner.ts:253`, `:261-263` @v0.83.0) and pi's RPC mode
//! (`modes/rpc/rpc-mode.ts:248-252`, `:290-300` @v0.83.0). They are wrong for the interactive TUI,
//! which is the one mode pi binds all five to live state in (`createExtensionUIContext`,
//! `modes/interactive/interactive-mode.ts:2393`, `:2401-2417` @v0.84.2).
//!
//! Every assertion below reads through the PRODUCTION path — the `HostServices` trait method on a
//! real `LiveHostServices`, which is what `live.rs` calls — never through the mirror or the access
//! handle directly, so an unattached seam fails them exactly as it failed in production.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::Arc;

use cyrup_ext::host::HostServices;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use cyrup_resources::{builtin_themes, ResourceRegistry, ResourceSet, Theme};
use cyrup_session_svc::LiveHostServices;
use ratatui::backend::TestBackend;

use crate::{App, UiTheme};

fn services() -> Arc<LiveHostServices> {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    Arc::new(LiveHostServices::new(
        provider,
        cyrup_tools::Backend::default().proc,
        std::env::temp_dir(),
    ))
}

/// The theme half of a session's discovered resources. `cyrup-resources`' real `discover` seeds the
/// candidate list with `builtin_themes()` before anything on disk, so this is the same set a booted
/// session carries when the user has authored no themes of their own.
fn registry() -> Arc<ResourceRegistry> {
    Arc::new(ResourceRegistry {
        themes: ResourceSet::build(builtin_themes()),
        ..ResourceRegistry::default()
    })
}

/// An app + the seams installed exactly as `App::run` installs them, returning the switch channel
/// the run loop's `theme_switch_rx` arm would drain.
fn wired() -> (App<TestBackend>, Arc<LiveHostServices>, tokio::sync::mpsc::UnboundedReceiver<Theme>)
{
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let svc = services();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Theme>();
    app.install_extension_readbacks(&svc, registry(), tx);
    (app, svc, rx)
}

// ---------------------------------------------------------------------------
// SEAM-T02 — `editor_text`
// ---------------------------------------------------------------------------

/// The core of the defect: a guest reads the buffer the user is looking at.
///
/// PRE-FIX this fails on the very first assertion. `LiveHostServices` did not override
/// `editor_text`, so the trait default returned `String::new()` no matter what the editor held —
/// `assert_eq!("", "hello, extension")`.
#[test]
fn a_guest_reads_the_live_editor_buffer() {
    let (mut app, svc, _rx) = wired();
    app.editor_mut().set_text("hello, extension");
    app.draw().unwrap();

    assert_eq!(
        HostServices::editor_text(svc.as_ref()),
        "hello, extension",
        "pi `getEditorText: () => this.editor.getExpandedText?.() ?? this.editor.getText()` \
         (`interactive-mode.ts:2393`)"
    );
}

/// The read half must answer with the buffer EXPANDED, not with the collapsed `[paste #N …]`
/// markers the user sees — pi hands the extension `getExpandedText?.()` first and only falls back to
/// `getText()` (`interactive-mode.ts:2393` @v0.84.2).
///
/// PRE-FIX: `""`, like every other `editor_text` read.
#[test]
fn the_buffer_a_guest_reads_has_pastes_expanded() {
    let (mut app, svc, _rx) = wired();
    app.editor_mut().handle_paste("line one\nline two\nline three\nline four\nline five\nline six");
    app.draw().unwrap();

    let seen = HostServices::editor_text(svc.as_ref());
    assert!(
        seen.contains("line six"),
        "the guest gets the expanded content, not a `[paste #N …]` marker: {seen:?}"
    );
}

/// The sharpest framing of the defect, and the reason a missing read here is DATA LOSS rather than
/// a mere absence: the WRITE half already worked. A guest that sets the buffer and immediately
/// reads it back to modify it — the read-modify-write an editor extension exists to do — used to
/// get `""` and write that back over its own text.
///
/// Pi cannot observe this window at all: its `setEditorText` is a synchronous
/// `this.editor.setText(text)` (`interactive-mode.ts:2392`), so the next `getEditorText()` already
/// returns it. Cyrup's write is fire-and-forget over the `UiEffectSink`, so the read has to be made
/// coherent explicitly — `LiveHostServices::set_editor_text`'s replace arm publishes through the
/// mirror. Note there is deliberately NO `draw()` between the write and the read: the whole point is
/// that the guest does not yield to the run loop.
///
/// PRE-FIX: the write reached the effect sink (that half was never broken) and the read returned
/// `""`, so the appended result was `" — appended by the extension"` with the guest's own prefix
/// gone.
#[test]
fn a_guest_reads_back_the_text_it_just_wrote() {
    let (mut app, svc, _rx) = wired();
    app.editor_mut().set_text("original");
    app.draw().unwrap();

    svc.set_editor_text("guest wrote this", false);
    let readback = HostServices::editor_text(svc.as_ref());
    assert_eq!(readback, "guest wrote this", "read-after-write must not lose the write");

    // …and the read-modify-write that motivates the whole seam round-trips intact.
    svc.set_editor_text(&format!("{readback} — appended by the extension"), false);
    assert_eq!(
        HostServices::editor_text(svc.as_ref()),
        "guest wrote this — appended by the extension"
    );
}

/// The headless/RPC policy is preserved: with nothing attached, `editor_text` is pi's `""`
/// (`noOpUIContext.getEditorText: () => ""`, `core/extensions/runner.ts:253`; `rpc-mode.ts:248-252`,
/// "Synchronous method can't wait for RPC response").
///
/// This one PASSES pre-fix — it is a regression guard on the mode policy, not evidence of the fix.
#[test]
fn an_unattached_mirror_keeps_pis_headless_empty_string() {
    let svc = services();
    svc.set_editor_text("ignored", false);
    assert_eq!(HostServices::editor_text(svc.as_ref()), "");
}

// ---------------------------------------------------------------------------
// SEAM-T01 — the theme family
// ---------------------------------------------------------------------------

/// pi's `get theme()` (`interactive-mode.ts:2401-2403`) — the ACTIVE theme, which must track a live
/// switch rather than reporting a boot-time constant.
///
/// PRE-FIX: `None` on both assertions — `LiveHostServices` did not override `theme`.
#[test]
fn a_guest_reads_the_active_theme_name() {
    let (mut app, svc, _rx) = wired();
    app.draw().unwrap();
    assert_eq!(HostServices::theme(svc.as_ref()), Some("dark".to_string()));

    app.set_theme(UiTheme::builtin("light"));
    app.draw().unwrap();
    assert_eq!(
        HostServices::theme(svc.as_ref()),
        Some("light".to_string()),
        "the active theme is live, not a boot snapshot"
    );
}

/// pi `getAllThemes(): {name, path}[]` (`core/extensions/types.ts:269`), implemented by
/// `getAvailableThemesWithPaths()` (`theme/theme.ts:493-520`): name-sorted, deduped, and carrying
/// the path that lets a guest tell a file-backed theme from a compiled-in one.
///
/// PRE-FIX: `json!([])`, so a guest saw NO themes at all — including the two that always exist.
#[test]
fn a_guest_lists_every_available_theme_with_its_path() {
    let (mut app, svc, _rx) = wired();
    app.draw().unwrap();

    let listed = HostServices::theme_list(svc.as_ref());
    let rows = listed.as_array().expect("theme_list is an array");
    let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(names, vec!["dark", "light"], "name-sorted, like `theme.ts:519`'s localeCompare");
    // [CYRUP-DELTA] `theme.ts:506-508` synthesizes `<themesDir>/<name>.json` for a built-in; cyrup's
    // built-ins are compiled-in constants with no file, so `null` (the EXT-021 contract) is the only
    // honest answer — and it is what distinguishes them from a file-backed theme.
    assert!(rows.iter().all(|r| r["path"].is_null()), "compiled-in built-ins carry no path");
}

/// pi `getTheme(name): Theme | undefined` (`core/extensions/types.ts:272`) — load a theme WITHOUT
/// switching to it, `undefined` when it does not resolve (`getThemeByName`, `theme.ts:671-677`).
///
/// PRE-FIX: `None` for every name, including ones that exist.
#[test]
fn a_guest_loads_one_theme_by_name_without_switching() {
    let (mut app, svc, _rx) = wired();
    app.draw().unwrap();

    let light = HostServices::theme_by_name(svc.as_ref(), "light").expect("`light` resolves");
    assert_eq!(light["name"], "light");
    assert!(
        light["colors"]["accent"].is_string(),
        "the guest gets the theme's colours, not just its name: {light}"
    );
    assert_eq!(
        HostServices::theme(svc.as_ref()),
        Some("dark".to_string()),
        "reading a theme by name must not switch to it"
    );
    assert!(HostServices::theme_by_name(svc.as_ref(), "no-such-theme").is_none());
}

/// EXT-066 added `theme-get-json` to the WIT world so a guest could read the ACTIVE theme's
/// COLOURS, and `cyrup-ext/src/host/live.rs` composes it from `theme()` + `theme_by_name()` rather
/// than from a third trait method. Both halves of that composition were dead, so the capability was
/// designed, signed into the world and shipped against a read that could only answer `None`. This
/// reproduces `live.rs::theme_get_json`'s exact composition.
///
/// PRE-FIX: `theme()` returned `None`, so the `?` short-circuited and `theme-get-json` answered
/// `None` for every guest, at every theme.
#[test]
fn ext_066_theme_get_json_composes_to_the_active_themes_colours() {
    let (mut app, svc, _rx) = wired();
    app.set_theme(UiTheme::builtin("light"));
    app.draw().unwrap();

    // `live.rs::theme_get_json`, line for line.
    let composed = HostServices::theme(svc.as_ref())
        .and_then(|name| HostServices::theme_by_name(svc.as_ref(), &name));
    let theme = composed.expect("the active theme's colours are readable");
    assert_eq!(theme["name"], "light");
    assert!(theme["colors"]["text"].is_string());
}

/// pi `setTheme(themeOrName)` (`interactive-mode.ts:2406-2417`): a resolvable name succeeds and
/// reaches the controller; an unresolvable one is `{success: false, error}` carrying the message
/// `loadThemeJson` throws (`Theme not found: {name}`, `theme.ts:622`).
///
/// PRE-FIX: BOTH names returned `Err("theme capability not granted")` — the trait default — so no
/// extension could ever switch the theme, and a guest could not distinguish "no such theme" from
/// "not allowed".
#[test]
fn a_guest_switches_the_theme_and_a_bad_name_reports_pis_error() {
    let (mut app, svc, mut rx) = wired();
    app.draw().unwrap();

    assert_eq!(HostServices::set_theme(svc.as_ref(), "light"), Ok(()));
    let switched = rx.try_recv().expect("the resolved theme reaches the run loop");
    assert_eq!(switched.key.as_str(), "light");

    assert_eq!(
        HostServices::set_theme(svc.as_ref(), "no-such-theme"),
        Err("Theme not found: no-such-theme".to_string()),
        "pi `theme.ts:622`, caught into `{{success: false, error}}` by `setTheme`"
    );
    assert!(rx.try_recv().is_err(), "a rejected name never reaches the run loop, so nothing repaints");
}

/// The mode policy for the theme family, preserved: unattached, all four are pi's headless answers
/// (`noOpUIContext`, `core/extensions/runner.ts:261-263`; RPC, `rpc-mode.ts:290-300`). This is also
/// why the switch does not ride the `UiEffect` sink — RPC installs that sink, and pi's RPC
/// `setTheme` is a hard-coded failure.
///
/// PRE-FIX the first three assertions passed — those trait defaults were already pi's values — but
/// the fourth FAILED: the unoverridden default reports `"theme capability not granted"`, which is
/// both wrong (reaching this backend means the grant WAS given) and not a string pi produces
/// anywhere. `LiveHostServices::set_theme` now answers `noOpUIContext`'s own wording instead.
#[test]
fn an_unattached_theme_seam_keeps_pis_headless_answers() {
    let svc = services();
    assert_eq!(HostServices::theme(svc.as_ref()), None);
    assert_eq!(HostServices::theme_list(svc.as_ref()), serde_json::json!([]));
    assert_eq!(HostServices::theme_by_name(svc.as_ref(), "dark"), None);
    assert_eq!(
        HostServices::set_theme(svc.as_ref(), "dark"),
        Err("UI not available".to_string()),
        "pi `noOpUIContext.setTheme` (`core/extensions/runner.ts:263`)"
    );
}
