//! JSON keybindings loader tests (spec/tui/07 §3.9; `core/keybindings.ts:14-262`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{
    Action, EditorAction, EditorKeymap, Key, Keymap, ModelsAction, ModelsKeymap, SelectAction,
    SelectKeymap,
};

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

#[test]
fn keymap_merge_json_rebinds_global_actions() {
    let mut km = Keymap::default();
    // Default: Esc → Interrupt. Rebind interrupt to ctrl+g and exit to a two-key set.
    km.merge_json(r#"{ "app.interrupt": "ctrl+g", "app.exit": ["ctrl+x", "ctrl+q"] }"#).unwrap();

    // Esc no longer interrupts (the old key was dropped on rebind).
    assert_eq!(km.action_for(&key(KeyCode::Esc, KeyModifiers::NONE)), None);
    assert_eq!(
        km.action_for(&key(KeyCode::Char('g'), KeyModifiers::CONTROL)),
        Some(Action::Interrupt)
    );
    // Both exit keys resolve.
    assert_eq!(km.action_for(&key(KeyCode::Char('x'), KeyModifiers::CONTROL)), Some(Action::Quit));
    assert_eq!(km.action_for(&key(KeyCode::Char('q'), KeyModifiers::CONTROL)), Some(Action::Quit));
    // Untouched defaults survive (Ctrl+Z → Suspend).
    assert_eq!(
        km.action_for(&key(KeyCode::Char('z'), KeyModifiers::CONTROL)),
        Some(Action::Suspend)
    );
}

#[test]
fn select_and_editor_maps_merge_their_own_ids_only() {
    // A single document can carry app/select/editor ids; each map picks only its own.
    let doc = r#"{
        "app.interrupt": "ctrl+g",
        "tui.select.confirm": "ctrl+y",
        "editor.submit": ["ctrl+m"]
    }"#;

    let mut sk = SelectKeymap::default();
    sk.merge_json(doc).unwrap();
    assert_eq!(
        sk.action_for(&key(KeyCode::Char('y'), KeyModifiers::CONTROL)),
        Some(SelectAction::Confirm)
    );
    // The app id is ignored by the select map (no panic, no spurious binding).
    assert_eq!(sk.action_for(&key(KeyCode::Char('g'), KeyModifiers::CONTROL)), None);

    let mut ek = EditorKeymap::default();
    ek.merge_json(doc).unwrap();
    assert_eq!(
        ek.action_for(&key(KeyCode::Char('m'), KeyModifiers::CONTROL)),
        Some(EditorAction::Submit)
    );
}

#[test]
fn models_keymap_merges_app_models_ids_only() {
    // The scoped-models bespoke map (`app.models.*`, core/keybindings.ts:150-175) picks only its ids.
    let doc = r#"{
        "app.models.save": "ctrl+enter",
        "app.models.reorderUp": "shift+up",
        "tui.select.confirm": "ctrl+y"
    }"#;
    let mut mk = ModelsKeymap::default();
    mk.merge_json(doc).unwrap();
    // The rebound save key resolves; the old default (ctrl+s) was dropped.
    assert_eq!(
        mk.action_for(&key(KeyCode::Enter, KeyModifiers::CONTROL)),
        Some(ModelsAction::Save)
    );
    assert_eq!(mk.action_for(&key(KeyCode::Char('s'), KeyModifiers::CONTROL)), None);
    assert_eq!(mk.action_for(&key(KeyCode::Up, KeyModifiers::SHIFT)), Some(ModelsAction::ReorderUp));
    // Defaults untouched by the doc survive (Ctrl+A → enableAll).
    assert_eq!(
        mk.action_for(&key(KeyCode::Char('a'), KeyModifiers::CONTROL)),
        Some(ModelsAction::EnableAll)
    );
}

#[test]
fn key_label_round_trips_through_parse() {
    assert_eq!(Key::parse("ctrl+c").unwrap().label(), "ctrl+c");
    // Both spellings PARSE, but the label is the one upstream uses in its binding table and prints
    // verbatim in every hint: `"app.interrupt": { defaultKeys: "escape" }` (v0.84.1
    // `coding-agent/src/core/keybindings.ts:66`), `"tui.select.cancel": { defaultKeys: ["escape",
    // "ctrl+c"] }` (`tui/src/keybindings.ts:149-152`).
    assert_eq!(Key::parse("esc").unwrap().label(), "escape");
    assert_eq!(Key::parse("escape").unwrap().label(), "escape");
    assert_eq!(Key::parse("shift+tab").unwrap().label(), "shift+tab");
    // The default interrupt key surfaces as the `escape` cancel hint (spec/tui/01 §6.1).
    assert_eq!(Keymap::default().key_label(Action::Interrupt).as_deref(), Some("escape"));
}

#[test]
fn malformed_keybindings_json_errors_cleanly() {
    let mut km = Keymap::default();
    assert!(km.merge_json("not json").is_err(), "garbage json should error");
    assert!(km.merge_json("[1,2,3]").is_err(), "a non-object document should error");
    assert!(
        km.merge_json(r#"{ "app.exit": "ctrl+nope+bad+" }"#).is_err(),
        "an invalid key spec should error"
    );
    // Unknown ids are silently ignored (forward-compat), not an error.
    assert!(km.merge_json(r#"{ "some.future.binding": "ctrl+a" }"#).is_ok());
}

// ---- TUI-028 / TUI-035: the id namespace -----------------------------------------------------

/// TUI-028 — pi's canonical editor/input ids must resolve.
///
/// cyrup shipped a bare `editor.*` namespace that matches **neither** pi's current
/// `tui.editor.*` / `tui.input.*` (`packages/tui/src/keybindings.ts:9-32`, already the spelling at
/// the ported baseline `v0.83.0`) **nor** pi's legacy bare names (`cursorUp`, `pageUp`, `newLine`,
/// … — the left column of `coding-agent/src/core/keybindings.ts:209-269`
/// `KEYBINDING_NAME_MIGRATIONS`). So all 24 editor/input bindings written from either era of pi's
/// documentation were silently inert: `merge_json` ignores an unknown id with no error and no
/// diagnostic, and the defaults just stayed.
#[test]
fn pis_canonical_editor_and_input_ids_resolve() {
    for (id, want) in [
        ("tui.editor.cursorWordLeft", EditorAction::CursorWordLeft),
        ("tui.editor.deleteToLineEnd", EditorAction::DeleteToLineEnd),
        ("tui.editor.undo", EditorAction::Undo),
        ("tui.editor.pageUp", EditorAction::PageUp),
        ("tui.input.newLine", EditorAction::NewLine),
        ("tui.input.submit", EditorAction::Submit),
        ("tui.input.tab", EditorAction::Tab),
    ] {
        assert_eq!(EditorAction::from_id(id), Some(want), "pi's id `{id}` must resolve");
    }
}

/// pi's LEGACY bare names — the keys `KEYBINDING_NAME_MIGRATIONS` maps
/// (`core/keybindings.ts:209-269`) — are what a pi user's older `keybindings.json` carries, and pi
/// migrates them forward on every load (`migrateKeybindingsConfig`, `:289-309`).
#[test]
fn pis_legacy_bare_names_resolve_too() {
    assert_eq!(EditorAction::from_id("cursorUp"), Some(EditorAction::CursorUp));
    assert_eq!(EditorAction::from_id("pageUp"), Some(EditorAction::PageUp));
    assert_eq!(EditorAction::from_id("newLine"), Some(EditorAction::NewLine));
}

/// The shipped-cyrup `editor.*` spellings stay accepted, so a config written against cyrup's own
/// released id list does not break — the same do-not-break-a-shipped-config promise pi's migration
/// table makes.
#[test]
fn cyrups_shipped_editor_ids_stay_accepted_as_aliases() {
    assert_eq!(EditorAction::from_id("editor.cursorLeft"), Some(EditorAction::CursorLeft));
    assert_eq!(EditorAction::from_id("editor.submit"), Some(EditorAction::Submit));
}

/// A `tui.editor.*` rebind actually takes effect through `merge_json`, end to end.
#[test]
fn a_tui_editor_id_rebinds_the_live_editor_map() {
    let mut ek = EditorKeymap::default();
    ek.merge_json(r#"{"tui.editor.cursorWordLeft": "alt+h"}"#).unwrap();
    assert_eq!(
        ek.action_for(&key(KeyCode::Char('h'), KeyModifiers::ALT)),
        Some(EditorAction::CursorWordLeft)
    );
}

/// TUI-028 — the autocomplete popup resolves through pi's `tui.select.*` / `tui.input.tab`, not
/// through an invented `tui.autocomplete.*` family. Upstream has no such family at all: the popup
/// reuses the select actions (`packages/tui/src/components/editor.ts:664-712` @v0.83.0). Rebinding
/// `tui.select.up` used to move selector highlights but NOT the popup, so one user-visible action
/// needed two different config keys.
#[test]
fn the_autocomplete_popup_resolves_through_pis_select_ids() {
    use crate::AutocompleteAction;
    assert_eq!(AutocompleteAction::from_id("tui.select.up"), Some(AutocompleteAction::Previous));
    assert_eq!(AutocompleteAction::from_id("tui.select.down"), Some(AutocompleteAction::Next));
    assert_eq!(AutocompleteAction::from_id("tui.input.tab"), Some(AutocompleteAction::Accept));
    assert_eq!(
        AutocompleteAction::from_id("tui.select.confirm"),
        Some(AutocompleteAction::AcceptSubmit)
    );
    assert_eq!(AutocompleteAction::from_id("tui.select.cancel"), Some(AutocompleteAction::Cancel));
    // The invented spellings stay as aliases.
    assert_eq!(
        AutocompleteAction::from_id("tui.autocomplete.previous"),
        Some(AutocompleteAction::Previous)
    );
}

/// TUI-035 — `tui.editor.historyPrevious` / `historyNext`, added in the `v0.83.0..v0.84.1` window
/// (`packages/tui/src/keybindings.ts:11-12`, `:68-75` @v0.84.1; absent from `v0.83.0`). Default
/// `defaultKeys: []`, so nothing is bound until the user says so.
#[test]
fn the_history_ids_resolve_and_are_unbound_by_default() {
    assert_eq!(
        EditorAction::from_id("tui.editor.historyPrevious"),
        Some(EditorAction::HistoryPrevious)
    );
    assert_eq!(EditorAction::from_id("tui.editor.historyNext"), Some(EditorAction::HistoryNext));
    let ek = EditorKeymap::default();
    for code in [KeyCode::Char('p'), KeyCode::Char('n')] {
        assert!(
            !matches!(
                ek.action_for(&key(code, KeyModifiers::CONTROL)),
                Some(EditorAction::HistoryPrevious) | Some(EditorAction::HistoryNext)
            ),
            "pi's defaults are empty for the history actions"
        );
    }
}

/// The dedicated history actions browse UNCONDITIONALLY, not only at a buffer edge — upstream's
/// comment is "Dedicated history actions always browse entries instead of moving the cursor"
/// (`packages/tui/src/components/editor.ts:766-777` @v0.84.1), and the two arms sit ahead of the
/// cursor-movement block. RED at HEAD: `EditorAction` had no history variants at all, so a user who
/// wanted shell-style recall on ctrl+p/ctrl+n, or Up/Down to be pure caret motion, had no way to
/// say so.
#[test]
fn a_bound_history_action_recalls_from_the_middle_of_a_multi_line_buffer() {
    use crate::{EditorOutcome, InputEditor};
    let mut ed = InputEditor::new();
    ed.push_history("earlier prompt");
    ed.merge_keybindings_json(r#"{"tui.editor.historyPrevious": "ctrl+p"}"#).unwrap();
    // A two-line buffer with the caret on the SECOND line: Up would move the caret, not recall.
    ed.set_text("one\ntwo");
    let out = ed.handle_key(&key(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert!(matches!(out, EditorOutcome::Edited));
    assert_eq!(ed.text(), "earlier prompt", "the history action must browse regardless of caret");
}

/// TUI-051 — `/reload` must actually re-read `<agent_dir>/keybindings.json`, and must RESTORE the
/// default for an entry the user deleted.
///
/// RED at HEAD: `grep -rn load_keybindings_json crates | grep -v /tests/` returned exactly two
/// lines — the definition and one boot-path call in `crates/cyrup/src/main.rs` — so nothing
/// re-read the file, while both the command's own description (`commands.rs`) and the handler's
/// in-source comment claimed it did. Pi calls `this.keybindings.reload()` inside
/// `handleReloadCommand` (`interactive-mode.ts:5386` @v0.83.0) → `loadFromFile` → `rebuild()`,
/// which REPLACES rather than merges: `userKeys === undefined ? normalizeKeys(defaultKeys) :
/// normalizeKeys(userKeys)` (`packages/tui/src/keybindings.ts:187-191`). cyrup's merge-only path
/// could never un-bind.
#[test]
fn reload_rereads_the_file_and_restores_a_deleted_entrys_default() {
    use crate::{App, UiTheme};
    use ratatui::backend::TestBackend;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybindings.json");
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();

    std::fs::write(&path, r#"{"app.tools.expand": "ctrl+e"}"#).unwrap();
    app.reload_keybindings_from(dir.path()).unwrap();
    assert_eq!(
        app.state().keymap.action_for(&key(KeyCode::Char('e'), KeyModifiers::CONTROL)),
        Some(Action::ToolsExpand)
    );

    // Rewrite the file: the new chord is live and the old one is gone.
    std::fs::write(&path, r#"{"app.tools.expand": "ctrl+y"}"#).unwrap();
    app.reload_keybindings_from(dir.path()).unwrap();
    assert_eq!(
        app.state().keymap.action_for(&key(KeyCode::Char('y'), KeyModifiers::CONTROL)),
        Some(Action::ToolsExpand)
    );
    assert_ne!(
        app.state().keymap.action_for(&key(KeyCode::Char('e'), KeyModifiers::CONTROL)),
        Some(Action::ToolsExpand),
        "the previous user chord must not survive"
    );

    // Delete the entry entirely: the STOCK default comes back (`keybindings.ts:187-191`).
    std::fs::write(&path, "{}").unwrap();
    app.reload_keybindings_from(dir.path()).unwrap();
    assert_eq!(
        app.state().keymap.action_for(&key(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Some(Action::ToolsExpand),
        "a removed entry must fall back to its default, not keep its last user chord"
    );
}

/// A missing file is not an error — it means "no user bindings" (pi's `loadFromFile` returns `{}`).
#[test]
fn reload_with_no_keybindings_file_leaves_the_defaults_in_place() {
    use crate::{App, UiTheme};
    use ratatui::backend::TestBackend;
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();
    app.reload_keybindings_from(dir.path()).expect("a missing file is not an error");
    assert_eq!(
        app.state().keymap.action_for(&key(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Some(Action::ToolsExpand)
    );
}
