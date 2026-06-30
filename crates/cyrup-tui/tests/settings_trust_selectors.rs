//! `/settings`, `/trust`, and the `/share` loader chrome (spec/tui/05 §6; gaps 1 + 2).
//!
//! Drive the new editor-swap selectors through the real `App::open_boxed_selector` + `handle_input`
//! routing and assert the rendered `TestBackend` buffer (full-width rules, title, label↔value
//! columns, the `→` cursor, the trust header) plus the routing outcomes (a settings cycle emits an
//! `ApplySetting` command and updates the displayed value in place; a trust confirm carries the chosen
//! option index; the bordered loader occupies the editor slot while a long op runs).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{
    App, AppAction, AppCommand, InputEvent, SelectorKind, SettingRow, SettingsSelector,
    TrustSelector, UiTheme,
};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

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

fn settings_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    let rows = vec![
        SettingRow::toggle("terminal.showImages", "Show images", true),
        SettingRow::choice(
            "steeringMode",
            "Steering mode",
            "one-at-a-time",
            vec!["all".to_string(), "one-at-a-time".to_string()],
        ),
    ];
    app.open_boxed_selector(
        SelectorKind::Settings,
        Box::new(SettingsSelector::new("Settings", rows)),
    );
    app
}

#[test]
fn settings_selector_renders_title_rows_and_values() {
    let mut app = settings_app();
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Settings));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Settings"), "title shown: {text}");
    assert!(text.contains("Show images"), "row label shown: {text}");
    assert!(text.contains("true"), "current value shown: {text}");
    assert!(text.contains('─'), "dynamic border rule shown: {text}");
}

#[test]
fn settings_enter_cycles_in_place_and_emits_apply_command() {
    let mut app = settings_app();
    // Enter cycles the highlighted toggle `true → false`, applies it live, and keeps the slot open.
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        action,
        AppAction::Command(AppCommand::ApplySetting {
            id: "terminal.showImages".to_string(),
            value: "false".to_string(),
        })
    );
    // The slot is still the settings selector (apply does NOT close), and the value flipped.
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Settings));
    app.draw().unwrap();
    assert!(buf_text(&app).contains("false"), "displayed value updated in place");
}

#[test]
fn settings_choice_cycles_through_its_set() {
    let mut app = App::new(TestBackend::new(70, 12), UiTheme::dark()).unwrap();
    app.open_boxed_selector(
        SelectorKind::Settings,
        Box::new(SettingsSelector::new(
            "Settings",
            vec![SettingRow::choice(
                "transport",
                "Transport",
                "auto",
                vec!["auto".to_string(), "websocket".to_string(), "sse".to_string()],
            )],
        )),
    );
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        action,
        AppAction::Command(AppCommand::ApplySetting {
            id: "transport".to_string(),
            value: "websocket".to_string(),
        })
    );
}

#[test]
fn trust_selector_renders_header_options_and_cursor() {
    let mut app = App::new(TestBackend::new(70, 18), UiTheme::dark()).unwrap();
    let labels =
        vec!["Trust".to_string(), "Do not trust".to_string()];
    app.open_boxed_selector(
        SelectorKind::Trust,
        Box::new(TrustSelector::new(
            "/home/me/project",
            "none",
            false,
            labels,
            0,
        )),
    );
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Project trust"), "header title: {text}");
    assert!(text.contains("/home/me/project"), "cwd shown: {text}");
    assert!(text.contains("Saved decision: none"), "saved decision line: {text}");
    assert!(text.contains("Current session: untrusted"), "session trust line: {text}");
    assert!(text.contains("Trust"), "option label: {text}");
    assert!(text.contains('→'), "selection cursor: {text}");
}

#[test]
fn trust_confirm_carries_selected_option_index() {
    let mut app = App::new(TestBackend::new(70, 18), UiTheme::dark()).unwrap();
    app.open_boxed_selector(
        SelectorKind::Trust,
        Box::new(TrustSelector::new(
            "/p",
            "none",
            false,
            vec!["Trust".to_string(), "Do not trust".to_string()],
            0,
        )),
    );
    // Move down to "Do not trust" (index 1), then confirm.
    let _ = app.handle_input(&key(KeyCode::Down));
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        action,
        AppAction::Command(AppCommand::ConfirmSelection {
            kind: SelectorKind::Trust,
            value: "1".to_string(),
        })
    );
    // Confirm closed the slot and restored the editor.
    assert_eq!(app.active_selector_kind(), None);
}

#[test]
fn trust_esc_cancels_without_confirming() {
    let mut app = App::new(TestBackend::new(60, 14), UiTheme::dark()).unwrap();
    app.open_boxed_selector(
        SelectorKind::Trust,
        Box::new(TrustSelector::new("/p", "none", true, vec!["Trust".to_string()], 0)),
    );
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), None);
}

#[test]
fn bordered_loader_occupies_the_editor_slot_when_set() {
    let mut app = App::new(TestBackend::new(60, 12), UiTheme::dark()).unwrap();
    app.state_mut().loader =
        Some(cyrup_tui::BorderedLoader::plain("Creating gist…"));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Creating gist"), "loader message rendered: {text}");
}
