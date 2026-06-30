//! JSON keybindings loader tests (spec/tui/07 §3.9; `core/keybindings.ts:14-262`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{
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
    assert_eq!(Key::parse("esc").unwrap().label(), "esc");
    assert_eq!(Key::parse("shift+tab").unwrap().label(), "shift+tab");
    // The default interrupt key surfaces as the `esc` cancel hint (spec/tui/01 §6.1).
    assert_eq!(Keymap::default().key_label(Action::Interrupt).as_deref(), Some("esc"));
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
