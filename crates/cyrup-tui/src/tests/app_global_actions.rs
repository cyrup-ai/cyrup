//! **TUI-008** — the seven global app bindings `interactive-mode.ts:2608-2618` registers and cyrup
//! routed nowhere.
//!
//! The keymap half (id → [`Action`], and the default chords) is in `tests::keymap`. This file is
//! the half that matters to a user: pressing the key must reach the same destination pi's
//! `onAction` callback reaches. Every destination already existed in cyrup and had no key routed to
//! it, which is exactly why the gap was invisible — `/model`, `/tree`, `/fork`, `/resume`, `/new`
//! and `/copy` all worked, so nothing looked broken.
//!
//! ```ts
//! this.defaultEditor.onAction("app.model.select",    () => this.showModelSelector());        // :2608
//! this.defaultEditor.onAction("app.thinking.toggle", () => this.toggleThinkingBlockVisibility()); // :2610
//! this.defaultEditor.onAction("app.message.copy",    () => void this.handleCopyCommand());   // :2612
//! this.defaultEditor.onAction("app.session.new",     () => this.handleClearCommand());       // :2615
//! this.defaultEditor.onAction("app.session.tree",    () => this.showTreeSelector());         // :2616
//! this.defaultEditor.onAction("app.session.fork",    () => this.showUserMessageSelector());  // :2617
//! this.defaultEditor.onAction("app.session.resume",  () => this.showSessionSelector());      // :2618
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::selector::SelectorKind;
use crate::transcript::Entry;
use crate::{App, AppAction, AppCommand, InputEvent, UiTheme};
use ratatui::backend::TestBackend;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

fn ctrl(c: char) -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// Every `showStatus` line currently in the transcript, verbatim.
fn statuses(app: &App<TestBackend>) -> Vec<String> {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Status(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Ctrl+L opens the UNFILTERED model picker — `showModelSelector()` (`:2608`), which is what a bare
/// `/model` runs (`handleModelCommand(undefined)`, `:4175`), i.e. `ModelCommand(None)` and not
/// `ModelCommand(Some(""))`: an empty search term would open the picker pre-filtered to the empty
/// string, a different code path.
#[test]
fn ctrl_l_opens_the_model_selector_unfiltered() {
    let mut app = new_app();
    assert_eq!(
        app.handle_input(&ctrl('l')),
        AppAction::Command(AppCommand::ModelCommand(None))
    );
}

/// Ctrl+X copies the last assistant message — `void this.handleCopyCommand()` (`:2612`), the
/// identical handler `/copy` runs, so it must be the identical command.
#[test]
fn ctrl_x_runs_the_copy_command() {
    let mut app = new_app();
    assert_eq!(app.handle_input(&ctrl('x')), AppAction::Command(AppCommand::Copy));
}

/// Ctrl+T toggles reasoning-block visibility: flip, PERSIST (`settingsManager.setHideThinkingBlock`,
/// `:3836`), and show `Thinking blocks: hidden|visible` (`:3849`). All three, and the flip has to
/// survive as *state*, not just as a returned command — the second press must go back.
#[test]
fn ctrl_t_toggles_thinking_visibility_persists_it_and_says_so() {
    let mut app = new_app();
    assert!(!app.state().transcript.hide_thinking_block(), "fixture: visible to start");

    let first = app.handle_input(&ctrl('t'));
    assert_eq!(
        first,
        AppAction::Command(AppCommand::ApplySetting {
            id: "hideThinkingBlock".to_string(),
            value: "true".to_string(),
        }),
        "the flip is PERSISTED (`:3836`), not just applied to the view"
    );
    assert!(app.state().transcript.hide_thinking_block(), "live effect, before any run-loop turn");
    let status = statuses(&app);
    assert!(
        status.iter().any(|s| s == "Thinking blocks: hidden"),
        "`:3849` status, verbatim: {status:?}"
    );

    // Second press returns. Reading the live flag rather than a local bool is the point: a handler
    // that returned the command but never touched the transcript would flip to `true` forever.
    let second = app.handle_input(&ctrl('t'));
    assert_eq!(
        second,
        AppAction::Command(AppCommand::ApplySetting {
            id: "hideThinkingBlock".to_string(),
            value: "false".to_string(),
        })
    );
    assert!(!app.state().transcript.hide_thinking_block());
}

/// The four `defaultKeys: []` ids (`core/keybindings.ts:115-118`). They are unreachable by default
/// *by design*, so the only way to exercise them is the way a user would: bind them. Each must land
/// on the destination its `/`-command uses.
#[test]
fn the_four_unbound_session_ids_route_to_their_commands_once_bound() {
    let mut app = new_app();
    // Bound the way a user binds them — through `keybindings.json`, the exact path that used to
    // drop these ids silently.
    app.load_keybindings_json(
        r#"{"app.session.new": "ctrl+a", "app.session.tree": "ctrl+b",
            "app.session.fork": "ctrl+e", "app.session.resume": "ctrl+f"}"#,
    )
    .unwrap();

    assert_eq!(app.handle_input(&ctrl('a')), AppAction::Command(AppCommand::NewSession));
    assert_eq!(
        app.handle_input(&ctrl('b')),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Tree))
    );
    assert_eq!(
        app.handle_input(&ctrl('e')),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::UserMessage)),
        "`showUserMessageSelector` (`:2617`) is /fork, not /tree"
    );
    assert_eq!(
        app.handle_input(&ctrl('f')),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Session))
    );
}

/// The display half of TUI-008. `/hotkeys` withheld three `**Other**` rows behind a
/// `[CYRUP-DELTA]` while the bindings were unported; with them ported the delta is void and the
/// table must be upstream's, **in upstream's order** (`interactive-mode.ts:5827-5842` @v0.83.0) —
/// `selectModel` after the cycle-models row, `toggleThinking` after `expandTools`, `copyMessage`
/// after `externalEditor`. Order is asserted, not just presence: three rows appended at the end
/// would satisfy a `contains` check and still not be pi's table.
#[test]
fn hotkeys_other_table_carries_the_three_restored_rows_in_upstream_order() {
    let app = new_app();
    let body = app.hotkeys_markdown_for_test();
    let other = body.split("**Other**").nth(1).expect("Other section");

    for (row, key_cell) in [
        ("| Open model selector |", "Ctrl+L"),
        ("| Toggle thinking block visibility |", "Ctrl+T"),
        ("| Copy last assistant message |", "Ctrl+X"),
    ] {
        assert!(other.contains(row), "missing row {row}:\n{other}");
        let line = other.lines().find(|l| l.contains(row)).unwrap();
        assert!(
            line.contains(key_cell),
            "the key cell must be the LIVE default, not empty — the delta's whole objection was an \
             empty cell advertising an unreachable shortcut: {line}"
        );
    }

    let at = |needle: &str| other.find(needle).unwrap_or_else(|| panic!("{needle} absent"));
    assert!(at("| Cycle models |") < at("| Open model selector |"), "`:5833` then `:5834`");
    assert!(
        at("| Open model selector |") < at("| Toggle tool output expansion |"),
        "`:5834` then `:5835`"
    );
    assert!(
        at("| Toggle tool output expansion |") < at("| Toggle thinking block visibility |"),
        "`:5835` then `:5836`"
    );
    assert!(
        at("| Toggle thinking block visibility |") < at("| Edit message in external editor |"),
        "`:5836` then `:5837`"
    );
    assert!(
        at("| Edit message in external editor |") < at("| Copy last assistant message |"),
        "`:5837` then `:5838`"
    );
    assert!(
        at("| Copy last assistant message |") < at("| Queue follow-up message |"),
        "`:5838` then `:5839`"
    );
}

/// MIRROR — rebinding through the real `keybindings.json` path moves the `/hotkeys` cell too, so
/// the row above is read from the live keymap and not frozen at compile time.
#[test]
fn a_rebind_moves_the_restored_hotkeys_cells() {
    let mut app = new_app();
    app.load_keybindings_json(r#"{"app.thinking.toggle": ["ctrl+alt+t", "f9"]}"#).unwrap();
    let body = app.hotkeys_markdown_for_test();
    let line = body
        .lines()
        .find(|l| l.contains("| Toggle thinking block visibility |"))
        .expect("row");
    // `alt` is rewritten to `option` on darwin (`keybinding-hints.ts:12-15`, ported at
    // `chrome.rs:41-47`), so the expected spelling is derived, never hardcoded — TUI-N10 is the
    // record of what hardcoding it costs. Derived with `cfg!`, NOT with `format_key_text`, so a
    // broken formatter cannot satisfy its own assertion.
    let alt = if cfg!(target_os = "macos") { "Option" } else { "Alt" };
    assert!(
        line.contains(&format!("Ctrl+{alt}+T/F9")),
        "every bound key, `/`-joined (`keybinding-hints.ts:29-36`): {line}"
    );
    assert!(!line.contains("Ctrl+T |"), "the replaced default must be gone: {line}");
}
