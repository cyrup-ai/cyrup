//! Command-execution + selector-trigger wiring tests (spec/tui/04 §2.3; gaps 2/11/1).
//!
//! Drive `App::handle_input` with a submitted slash line and assert the routed [`AppAction`]: in-crate
//! effects (open the dependency-free selectors, push info blocks, quit) vs. session/data-bound effects
//! surfaced as [`AppCommand`] for the run loop.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, AppAction, AppCommand, Entry, InputEvent, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// Submit `line` through the real editor → dispatch path, returning the resulting [`AppAction`].
fn submit(app: &mut App<TestBackend>, line: &str) -> AppAction {
    app.editor_mut().set_text(line);
    app.handle_input(&key(KeyCode::Enter))
}

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

#[test]
fn data_bound_commands_route_to_open_selector() {
    let mut app = new_app();
    // `/model` (no arg) routes to the model command with no search term (F6: exact-match/pre-filter is
    // resolved by the run loop against the live catalog; a bare `/model` opens the unfiltered picker).
    assert_eq!(
        submit(&mut app, "/model"),
        AppAction::Command(AppCommand::ModelCommand(None))
    );
    // `/model <text>` threads the argument (exact match → set directly; partial → pre-filtered picker).
    assert_eq!(
        submit(&mut app, "/model qwen"),
        AppAction::Command(AppCommand::ModelCommand(Some("qwen".to_string())))
    );
    assert_eq!(
        submit(&mut app, "/settings"),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Settings))
    );
    assert_eq!(
        submit(&mut app, "/tree"),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Tree))
    );
    assert_eq!(
        submit(&mut app, "/resume"),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Session))
    );
    // `/login [provider]` threads its argument the same way `/model` does (`handleLoginCommand`,
    // interactive-mode.ts:2810): the run loop resolves it against the live registry + credential
    // store, because a match may start the login outright with no picker at all.
    assert_eq!(
        submit(&mut app, "/login"),
        AppAction::Command(AppCommand::LoginCommand(None))
    );
    assert_eq!(
        submit(&mut app, "/login anthropic"),
        AppAction::Command(AppCommand::LoginCommand(Some("anthropic".to_string())))
    );
    assert_eq!(
        submit(&mut app, "/logout"),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Logout))
    );
}

#[test]
fn lifecycle_commands_route_with_arguments() {
    let mut app = new_app();
    assert_eq!(submit(&mut app, "/new"), AppAction::Command(AppCommand::NewSession));
    assert_eq!(
        submit(&mut app, "/compact tighten it"),
        AppAction::Command(AppCommand::Compact(Some("tighten it".to_string())))
    );
    assert_eq!(submit(&mut app, "/compact"), AppAction::Command(AppCommand::Compact(None)));
    assert_eq!(
        submit(&mut app, "/name my session"),
        AppAction::Command(AppCommand::SetName("my session".to_string()))
    );
    assert_eq!(
        submit(&mut app, "/export out.jsonl"),
        AppAction::Command(AppCommand::Export(Some("out.jsonl".to_string())))
    );
    assert_eq!(submit(&mut app, "/copy"), AppAction::Command(AppCommand::Copy));
    assert_eq!(submit(&mut app, "/session"), AppAction::Command(AppCommand::SessionInfo));
}

#[test]
fn quit_command_quits() {
    let mut app = new_app();
    assert_eq!(submit(&mut app, "/quit"), AppAction::Quit);
}

/// MIRROR of the `/hotkeys` envelope fix. `/changelog` (interactive-mode.ts:6067-6072) builds the
/// byte-identical component stack to `/hotkeys` (:6197-6203) — same `Spacer/DynamicBorder/Text(…,1,0)/
/// Spacer/Markdown(…,1,1)/DynamicBorder` — so the same `Entry::Block` correction must show up here:
/// the title inset one column, the body inset one column with a `paddingY` blank on each side.
#[test]
fn changelog_block_mirrors_the_hotkeys_envelope() {
    let mut app = new_app();
    assert_eq!(submit(&mut app, "/changelog"), AppAction::Redraw);
    app.draw().unwrap();
    let text = app.scrollback_text();
    let rows: Vec<&str> = text.lines().collect();
    let rule = "─".repeat(80);
    let top = rows.iter().position(|r| r.trim_end() == rule).expect("opening rule");
    assert_eq!(rows[top + 1], " What's New", "title inset by Text's paddingX 1:\n{text}");
    assert!(rows[top + 2].trim().is_empty(), "Spacer(1) after the title:\n{text}");
    assert!(rows[top + 3].trim().is_empty(), "Markdown paddingY blank:\n{text}");
    assert_eq!(
        rows[top + 4], " No changelog entries found.",
        "body inset by Markdown's paddingX 1:\n{text}"
    );
    assert!(rows[top + 5].trim().is_empty(), "trailing paddingY blank:\n{text}");
    assert_eq!(rows[top + 6].trim_end(), rule, "closing rule:\n{text}");
}

#[test]
fn hotkeys_command_pushes_a_scrollback_block_and_no_overlay() {
    let mut app = new_app();
    assert!(!app.overlay_open());
    assert_eq!(submit(&mut app, "/hotkeys"), AppAction::Redraw);
    // `handleHotkeysCommand` (interactive-mode.ts:6197-6203) calls `chatContainer.addChild(...)` six
    // times and never touches `ui.showOverlay`; the help is scrollback, exactly like `/changelog`
    // (:6067-6072). This assertion previously demanded the opposite.
    assert!(!app.overlay_open(), "/hotkeys must not open the overlay z-stack");
    assert!(
        app.state()
            .transcript
            .pending()
            .iter()
            .any(|e| matches!(e, Entry::Block { title, .. } if title == "Keyboard Shortcuts")),
        "a `Keyboard Shortcuts` scrollback block is pushed",
    );
}

#[test]
fn confirming_a_data_selector_emits_confirm_selection_command() {
    let mut app = new_app();
    // Open a data-bound model selector directly with injected rows (the run loop normally sources them).
    app.open_data_selector(
        SelectorKind::Model,
        vec![
            ("anthropic/opus".to_string(), "Claude Opus".to_string(), Some("anthropic".to_string())),
            ("openai/gpt".to_string(), "GPT".to_string(), Some("openai".to_string())),
        ],
        0,
    );
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Model));
    // Move to the 2nd row and confirm → AppAction::Command(ConfirmSelection{Model, "openai/gpt"}).
    app.handle_input(&key(KeyCode::Down));
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        action,
        AppAction::Command(AppCommand::ConfirmSelection {
            kind: SelectorKind::Model,
            value: "openai/gpt".to_string()
        })
    );
    // The selector closed on confirm.
    assert_eq!(app.active_selector_kind(), None);
}

#[test]
fn armin_easter_egg_pushes_a_half_block_art_block() {
    let mut app = new_app();
    assert_eq!(submit(&mut app, "/arminsayshi"), AppAction::Redraw);
    // The easter egg is a real rich transcript block (the XBM half-block art), not a status line.
    let block = app
        .state()
        .transcript
        .pending()
        .iter()
        .find_map(|e| match e {
            Entry::Block { title, markdown } if title == "Armin says hi!" => Some(markdown.clone()),
            _ => None,
        })
        .expect("armin art block pushed");
    assert!(
        block.contains('█') || block.contains('▀') || block.contains('▄'),
        "art uses half-block glyphs: {block:?}"
    );
}
