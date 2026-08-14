//! Feature #1 — `/model` selector, proven by an **assembled** `App` render (the TUI lesson: a
//! per-widget test is not enough; render the WHOLE app through `TestBackend` with the selector OPEN in
//! the input slot and inspect the buffer). Asserts the buffer carries the fuzzy **search box**, a `✓`
//! on the **active** model, and a `[provider]` badge per row — the pieces the audit found missing when
//! `/model` degraded to a bare titled list (Pi `model-selector.ts:229-283`, spec/tui/05 §5.2).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, InputEvent, ModelEntry, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// The whole rendered buffer as text.
fn buf_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

fn catalog() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            id: "claude-opus-4-6".into(),
            name: "Claude Opus 4.6".into(),
            provider: "anthropic".into(),
            current: true,
            scoped: false,
        },
        ModelEntry {
            id: "claude-sonnet-4-6".into(),
            name: "Claude Sonnet 4.6".into(),
            provider: "anthropic".into(),
            current: false,
            scoped: false,
        },
        ModelEntry {
            id: "gpt-5.1".into(),
            name: "GPT 5.1".into(),
            provider: "openai".into(),
            current: false,
            scoped: false,
        },
        ModelEntry {
            id: "gemini-3-pro".into(),
            name: "Gemini 3 Pro".into(),
            provider: "google".into(),
            current: false,
            scoped: false,
        },
    ]
}

#[test]
fn assembled_model_selector_open_shows_search_check_and_provider_badges() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    // Seed a model footer as the binary does, then open `/model` the way the run loop does
    // (`App::open_model_selector`) — the selector swaps into the editor slot over the live app.
    app.status_mut().set_model("anthropic/claude-opus-4-6");
    app.open_model_selector(catalog(), None);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Model));
    app.draw().unwrap();

    let screen = buf_text(&app);
    // S31: the fuzzy search box above the list is `Input.render`'s shared, unstyled `"> "` at
    // column 0 (`input.ts:380`) — `model-selector.ts:118` adds the `Input` as a bare container
    // child, so nothing insets or colours it. The accent `" \u{258f}"…"\u{258f}"` bars this used to
    // assert are a cyrup invention: U+258F appears in no pi TUI source.
    assert!(
        screen.lines().any(|l| l.starts_with("> ")),
        "search box `\"> \"` prompt missing from assembled buffer:\n{screen}"
    );
    assert!(!screen.contains('\u{258f}'), "no U+258F bars anywhere:\n{screen}");
    // A `[provider]` badge on every provider (Pi `:251`, muted).
    assert!(screen.contains("[anthropic]"), "provider badge [anthropic] missing:\n{screen}");
    assert!(screen.contains("[openai]"), "provider badge [openai] missing:\n{screen}");
    assert!(screen.contains("[google]"), "provider badge [google] missing:\n{screen}");
    // The `✓` marks the active model (Pi `:252`, success).
    assert!(screen.contains('✓'), "active-model check `✓` missing:\n{screen}");
    // The `→` selection cursor + `Model Name:` footer (Pi `:249`, `:282`).
    assert!(screen.contains('→'), "selection cursor `→` missing:\n{screen}");
    assert!(screen.contains("Model Name:"), "model-name footer missing:\n{screen}");
    // The active model was sorted to the top and is the initial highlight.
    assert!(screen.contains("claude-opus-4-6"), "active model row missing:\n{screen}");
}

#[test]
fn assembled_model_selector_typing_filters_the_live_render() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.open_model_selector(catalog(), None);
    // Type `gpt` into the embedded search box; the assembled render must narrow to the openai row.
    for c in "gpt".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    app.draw().unwrap();
    let screen = buf_text(&app);
    assert!(screen.contains("gpt-5.1"), "typed query did not surface the gpt row:\n{screen}");
    assert!(
        !screen.contains("gemini-3-pro"),
        "fuzzy filter did not drop the non-matching gemini row:\n{screen}"
    );
    // The typed query is echoed in the search box.
    assert!(screen.contains("gpt"), "typed query missing from the search box:\n{screen}");
}
