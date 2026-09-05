//! FLUX-004 — the status-overlay chord must not be a default keybinding of the TUI it runs in.
//!
//! Before this item the extension registered `ctrl+f`, which is `tui.editor.cursorRight`'s default
//! on both sides (pi `packages/tui/src/keybindings.ts:86-89` @v0.84.4 `["right", "ctrl+f"]`;
//! cyrup `crates/cyrup-tui/src/keymap.rs` `impl Default for EditorKeymap`). The extension-shortcut
//! tier is consulted BEFORE the editor (`crates/cyrup-tui/src/app/input.rs`), so the chord took
//! forward-char away from every emacs-motion user on a default install — with no diagnostic (at the
//! time nothing called `ExtensionRegistry::resolve_shortcuts`, EXT-039's residual, since closed by
//! `App::install_extension_shortcuts`) and no rebind path (an extension shortcut is not a
//! `Keybinding`). Upstream `flux_bootstrap/` @v0.0.40
//! registers no keybinding at all, so the chord is a cyrup design choice with no upstream line to
//! copy; the invariant it must satisfy is "bound by no default keymap", and that is what these
//! tests check against the REAL default tables rather than a hand-copied list.
//!
//! Red before: with `STATUS_OVERLAY_SHORTCUT = "ctrl+f"` (the constant introduced at the old
//! value first, so the failure is an assertion and not a compile error), three of the four fail —
//! `the_status_shortcut_is_bound_by_no_default_keymap` with `EditorKeymap::default() already
//! binds "ctrl+f" to Some(CursorRight)`, `..._resolves_with_no_conflict_...` with the registry's
//! own rule-3 diagnostic (`'ctrl+f' is built-in shortcut for tui.editor.cursorRight and
//! cyrup-flux. Using cyrup-flux.`), and `execute_shortcut_routes_...` because the retired chord
//! still reached the overlay. `the_extension_registers_exactly_the_constant_chord` passes on both
//! values and pins the registration / dispatch contract around the constant.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_ext::host::{NotifyKind, RecordingServices};
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig, HostCtx, NativeExtension};
use cyrup_flux::extension::{STATUS_OVERLAY_SHORTCUT, STATUS_OVERLAY_SHORTCUT_DESCRIPTION};
use cyrup_flux::resources::BundledRoot;
use cyrup_tui::crossterm::event::{KeyEvent, KeyModifiers};
use cyrup_tui::{
    AltScreenKeymap, AutocompleteKeymap, EditorKeymap, Key, Keymap, ModelsKeymap, SelectKeymap,
    SessionKeymap, TreeKeymap,
};

/// The chord as the TUI would see it arrive — `App::set_extension_shortcuts` parses the registered
/// id with `Key::parse` and matches presses against it, so a chord that does not parse there is
/// silently dropped rather than bound.
fn chord_event() -> KeyEvent {
    let key = Key::parse(STATUS_OVERLAY_SHORTCUT)
        .unwrap_or_else(|e| panic!("{STATUS_OVERLAY_SHORTCUT:?} must parse as a TUI key: {e}"));
    KeyEvent::new(key.code, key.mods)
}

fn command_ctx() -> HostCtx {
    HostCtx::command(ExtMode::Tui, true, std::env::temp_dir())
}

/// Every keymap the TUI consults on a key press, each at its DEFAULT table, must leave the chord
/// unbound — otherwise the extension tier (which fires before the editor) steals it, or, for a
/// selector-scoped table, the extension fires only while no selector is open and the same chord
/// means two things.
#[test]
fn the_status_shortcut_is_bound_by_no_default_keymap() {
    let ev = chord_event();
    assert!(
        ev.modifiers.contains(KeyModifiers::CONTROL),
        "a bare or shift-only chord would leak in as text"
    );
    assert_eq!(
        Keymap::default().action_for(&ev),
        None,
        "Keymap::default() already binds {STATUS_OVERLAY_SHORTCUT:?}"
    );
    assert_eq!(
        EditorKeymap::default().action_for(&ev),
        None,
        "EditorKeymap::default() already binds {STATUS_OVERLAY_SHORTCUT:?} to {:?}",
        EditorKeymap::default().action_for(&ev)
    );
    assert_eq!(
        SelectKeymap::default().action_for(&ev),
        None,
        "SelectKeymap::default() already binds {STATUS_OVERLAY_SHORTCUT:?}"
    );
    assert_eq!(
        AutocompleteKeymap::default().action_for(&ev),
        None,
        "AutocompleteKeymap::default() already binds {STATUS_OVERLAY_SHORTCUT:?}"
    );
    assert_eq!(
        ModelsKeymap::default().action_for(&ev),
        None,
        "ModelsKeymap::default() already binds {STATUS_OVERLAY_SHORTCUT:?}"
    );
    assert_eq!(
        SessionKeymap::default().action_for(&ev),
        None,
        "SessionKeymap::default() already binds {STATUS_OVERLAY_SHORTCUT:?}"
    );
    assert_eq!(
        TreeKeymap::default().action_for(&ev),
        None,
        "TreeKeymap::default() already binds {STATUS_OVERLAY_SHORTCUT:?}"
    );
    assert_eq!(
        AltScreenKeymap::default().action_for(&ev),
        None,
        "AltScreenKeymap::default() already binds {STATUS_OVERLAY_SHORTCUT:?}"
    );
}

/// The registry-side rule that WOULD have flagged the old chord had the host called it (pi
/// `getShortcuts` rule 3, `extensions/runner.ts:517-522` @v0.83.0 — warn, extension wins):
/// resolved against `tui.editor.cursorRight`'s pi/cyrup default keys, the chord must produce no
/// diagnostic. EXT-039 has since wired that gate into production
/// (`App::install_extension_shortcuts` over `App::effective_keybindings`); this keeps the
/// registry-level check pinned to the two ids the retired chord actually collided with.
#[test]
fn the_status_shortcut_resolves_with_no_conflict_against_the_editor_defaults() {
    let reg = cyrup_ext::ExtensionRegistry::new();
    reg.register_shortcut(
        "cyrup-flux".into(),
        STATUS_OVERLAY_SHORTCUT,
        Some(STATUS_OVERLAY_SHORTCUT_DESCRIPTION.into()),
    )
    .unwrap();
    // `tui.editor.cursorRight`/`cursorLeft` as pi declares them (`packages/tui/src/keybindings.ts`
    // `:82-89` @v0.84.4) — the two emacs motions the old chord collided with.
    let keymap = vec![
        (
            "tui.editor.cursorLeft".to_string(),
            vec!["left".to_string(), "ctrl+b".to_string()],
        ),
        (
            "tui.editor.cursorRight".to_string(),
            vec!["right".to_string(), "ctrl+f".to_string()],
        ),
    ];
    let resolved = reg.resolve_shortcuts(&keymap).unwrap();
    assert_eq!(
        resolved,
        vec![(
            STATUS_OVERLAY_SHORTCUT.to_lowercase(),
            cyrup_core::ExtensionId::from("cyrup-flux")
        )]
    );
    assert!(
        reg.shortcut_diagnostics().unwrap().is_empty(),
        "the chord collides with an editor default: {:?}",
        reg.shortcut_diagnostics().unwrap()
    );
}

/// Loading the extension through the real host registers exactly the constant, with its
/// `/hotkeys` description — so the registration site cannot drift from the dispatch site.
#[tokio::test]
async fn the_extension_registers_exactly_the_constant_chord() {
    let scratch = tempfile::tempdir().unwrap();
    let host = ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: scratch.path().to_path_buf(),
    });
    let ext = cyrup_flux::flux_extension_with_root(BundledRoot::Vendored(
        scratch.path().join("resources"),
    ));
    host.load_native(ext).await.unwrap();
    assert_eq!(
        host.shortcut_specs(),
        vec![(
            STATUS_OVERLAY_SHORTCUT.to_string(),
            Some(STATUS_OVERLAY_SHORTCUT_DESCRIPTION.to_string())
        )]
    );
}

/// `execute_shortcut` opens the overlay for the constant and ONLY the constant. The recording
/// backend owns no terminal, so `open_overlay` answers `false` and the handler takes its documented
/// fallback — the plain `/flux/status` table on the `notify` channel — which is the observable.
#[tokio::test]
async fn execute_shortcut_routes_the_constant_and_ignores_the_retired_chord() {
    let scratch = tempfile::tempdir().unwrap();
    let ext = cyrup_flux::flux_extension_with_root(BundledRoot::Vendored(
        scratch.path().join("resources"),
    ));
    let services = Arc::new(RecordingServices::default());
    ext.set_host_services(services.clone());

    ext.execute_shortcut("ctrl+f", &command_ctx())
        .await
        .unwrap();
    assert!(
        services.notify_calls().is_empty(),
        "the retired chord must not reach the overlay: {:?}",
        services.notify_calls()
    );

    ext.execute_shortcut(STATUS_OVERLAY_SHORTCUT, &command_ctx())
        .await
        .unwrap();
    let calls = services.notify_calls();
    assert_eq!(calls.len(), 1, "one fallback table: {calls:?}");
    assert_eq!(calls[0].1, NotifyKind::Info);
    assert!(
        calls[0].0.contains("TODO") || calls[0].0.contains("no flux state"),
        "the fallback is the plain status table: {:?}",
        calls[0].0
    );
}
