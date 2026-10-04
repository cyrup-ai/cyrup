//! The pointer over a selector that replaced the editor, driven the way a terminal drives it: a
//! real fullscreen `App` on a `TestBackend`, a frame drawn, a cell found from the rendered text,
//! and `Down`/`Up`/`ScrollDown`/`Moved` reports sent through `App::handle_input`.
//!
//! A press highlights the row under the pointer, a click activates it exactly as `Enter` would, a
//! wheel notch moves the highlight one row without wrapping, and hover moves nothing (pi's
//! `SelectList.handleMouse` / `SettingsList.handleMouse`, `select-list.ts:109-143`,
//! `settings-list.ts:179-220` @v1.0.0). Everything that is not a row of the list — rules, titles,
//! the search box, the `(i/N)` readout, hints, blanks — is not the selector's to act on.
//!
//! Row positions are never hard-coded: each test finds the row by its label in the drawn buffer and
//! checks the cell lies inside `regions.slot`, the rectangle the pointer layer resolves against.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent};
use ratatui::crossterm::event::{MouseEventKind as Kind, MouseEventKind};
use ratatui::layout::Position;

use crate::{
    App, AppAction, AppCommand, ConfigKind, ConfigRow, ConfigScope, ConfigSelector, InputEvent,
    ModelEntry, OAuthMode, OAuthSelector, SelectorKind, SessionRow, SessionSelector, SettingRow,
    SettingsSelector, TreeNode, TreeSelector, TrustSelector, UiTheme, UserMessageRow,
    UserMessageSelector,
};
use cyrup_config::login::{AuthType, LoginProviderOption};
use cyrup_core::ProviderId;

const COLS: u16 = 80;
const ROWS: u16 = 44;

fn fullscreen_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app
}

/// The text of every screen row of the frame just drawn.
fn screen(app: &mut App<TestBackend>) -> Vec<String> {
    app.draw().unwrap();
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    let buf = alt.backend_for_test().buffer().clone();
    (0..ROWS)
        .map(|y| {
            (0..COLS)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// The screen row whose text contains `needle`.
fn row_of(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` is not on screen: {rows:#?}"))
}

/// The cell of the first occurrence of `needle` in the drawn frame, which must lie in the slot the
/// pointer layer resolves a selector click against.
fn cell(app: &mut App<TestBackend>, needle: &str) -> (u16, u16) {
    let rows = screen(app);
    let y = row_of(&rows, needle);
    let byte = rows[y].find(needle).unwrap();
    let x = rows[y][..byte].chars().count();
    let at = (x as u16, y as u16);
    let slot = app.state_mut().regions.slot;
    assert!(
        slot.contains(Position::new(at.0, at.1)),
        "`{needle}` at {at:?} is outside the slot {slot:?}"
    );
    at
}

/// Whether the row holding `needle` carries the highlight cursor `glyph`.
fn marked(app: &mut App<TestBackend>, needle: &str, glyph: &str) -> bool {
    let rows = screen(app);
    rows[row_of(&rows, needle)].trim_start().starts_with(glyph)
}

fn mouse(kind: MouseEventKind, at: (u16, u16)) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column: at.0,
        row: at.1,
        modifiers: KeyModifiers::NONE,
    })
}

fn press(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    app.handle_input(&mouse(Kind::Down(MouseButton::Left), at))
}

fn release(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    app.handle_input(&mouse(Kind::Up(MouseButton::Left), at))
}

/// A press and a release on the same cell — the gesture the renderer turns into a click. Answers
/// the release's action, which is where a click's effect comes out.
fn click(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    press(app, at);
    release(app, at)
}

fn wheel(app: &mut App<TestBackend>, down: bool, at: (u16, u16)) -> AppAction {
    let kind = if down {
        Kind::ScrollDown
    } else {
        Kind::ScrollUp
    };
    app.handle_input(&mouse(kind, at))
}

fn hover(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    app.handle_input(&mouse(Kind::Moved, at))
}

fn press_key(app: &mut App<TestBackend>, code: KeyCode) {
    app.handle_input(&InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn confirm(kind: SelectorKind, value: &str) -> AppAction {
    AppAction::Command(AppCommand::ConfirmSelection {
        kind,
        value: value.to_string(),
    })
}

// ------------------------------------------------------------------------------------- /model --

fn catalog(n: usize) -> Vec<ModelEntry> {
    (1..=n)
        .map(|i| ModelEntry {
            id: format!("model-{i:02}"),
            name: format!("Model Number {i}"),
            provider: "acme".into(),
            current: i == 1,
            scoped: false,
        })
        .collect()
}

fn model_app(n: usize) -> App<TestBackend> {
    let mut app = fullscreen_app();
    app.open_model_selector(catalog(n), None);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Model));
    app
}

/// TUI-107's Verify: open `/model`, click row 3, and that model is selected.
#[test]
fn clicking_the_third_model_row_selects_that_model() {
    let mut app = model_app(5);
    let third = cell(&mut app, "model-03 [");
    let action = click(&mut app, third);
    assert_eq!(action, confirm(SelectorKind::Model, "acme/model-03"));
    assert_eq!(app.active_selector_kind(), None, "the picker closed");
}

#[test]
fn a_press_on_a_model_row_highlights_it_without_confirming() {
    let mut app = model_app(5);
    assert!(marked(&mut app, "model-01 [", "→"));
    let third = cell(&mut app, "model-03 [");
    press(&mut app, third);
    assert!(
        marked(&mut app, "model-03 [", "→"),
        "the pressed row is lit"
    );
    assert!(!marked(&mut app, "model-01 [", "→"));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Model));
}

#[test]
fn what_is_not_a_model_row_ignores_the_pointer() {
    let mut app = model_app(5);
    // Control: the pointer is live — a press on a model row lights it.
    let second = cell(&mut app, "model-02 [");
    press(&mut app, second);
    assert!(marked(&mut app, "model-02 [", "→"), "control press");

    let rows = screen(&mut app);
    let slot = app.state_mut().regions.slot;
    let search = rows
        .iter()
        .position(|r| r.trim_end() == ">")
        .expect("the search row");
    let name_footer = row_of(&rows, "Model Name:");
    for y in [
        slot.y as usize,
        search,
        search + 1,
        name_footer,
        name_footer + 1,
    ] {
        let at = (2, y as u16);
        assert!(slot.contains(Position::new(at.0, at.1)));
        press(&mut app, at);
        assert_eq!(release(&mut app, at), AppAction::None, "row {y}: {rows:#?}");
        assert_eq!(app.active_selector_kind(), Some(SelectorKind::Model));
        assert!(marked(&mut app, "model-02 [", "→"), "row {y} moved nothing");
    }
}

#[test]
fn hovering_a_model_row_never_moves_the_highlight() {
    let mut app = model_app(5);
    let third = cell(&mut app, "model-03 [");
    hover(&mut app, third);
    assert!(marked(&mut app, "model-01 [", "→"));
    assert!(!marked(&mut app, "model-03 [", "→"));
    // Control: the same cell, pressed, does move it.
    press(&mut app, third);
    assert!(marked(&mut app, "model-03 [", "→"));
}

#[test]
fn the_wheel_steps_the_model_highlight_and_stops_at_the_ends() {
    let mut app = model_app(3);
    let over = cell(&mut app, "model-02 [");
    // Up from the first row does not wrap to the last.
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "model-01 [", "→"));
    wheel(&mut app, true, over);
    assert!(marked(&mut app, "model-02 [", "→"));
    wheel(&mut app, true, over);
    wheel(&mut app, true, over);
    assert!(marked(&mut app, "model-03 [", "→"), "clamped at the last");
}

/// A wheel notch over the rule or the search box is not over the list, so the document behind the
/// dock gets it and the highlight stays.
#[test]
fn a_wheel_notch_over_the_chrome_leaves_the_model_highlight_alone() {
    let mut app = model_app(3);
    let slot = app.state_mut().regions.slot;
    let rule = (2, slot.y);
    wheel(&mut app, true, rule);
    assert!(marked(&mut app, "model-01 [", "→"));
    // Control: over a model row the same notch moves the highlight.
    let over = cell(&mut app, "model-02 [");
    wheel(&mut app, true, over);
    assert!(marked(&mut app, "model-02 [", "→"));
}

/// The list is windowed around the highlight: with the highlight deep in a long catalog the rows on
/// screen are absolute indices 5..15, and a click lands on what the cell shows.
#[test]
fn a_scrolled_model_list_maps_cells_to_absolute_indices() {
    let mut app = model_app(15);
    for _ in 0..12 {
        press_key(&mut app, KeyCode::Down);
    }
    let rows = screen(&mut app);
    assert!(
        rows.iter().all(|r| !r.contains("model-05 [")),
        "the window starts after row 5: {rows:#?}"
    );
    let at = cell(&mut app, "model-14 [");
    assert_eq!(
        click(&mut app, at),
        confirm(SelectorKind::Model, "acme/model-14")
    );
}

/// Pressing a row recentres the window, sliding other rows under the pointer; the click that ends
/// the gesture still activates the row the press went down on (pi's `mousePressedIndex`).
#[test]
fn the_click_confirms_the_row_the_press_went_down_on() {
    let mut app = model_app(15);
    for _ in 0..12 {
        press_key(&mut app, KeyCode::Down);
    }
    let at = cell(&mut app, "model-07 [");
    press(&mut app, at);
    // The window slid: the cell now shows a different model.
    let rows = screen(&mut app);
    assert!(
        !rows[usize::from(at.1)].contains("model-07 ["),
        "the press slid the window under the pointer: {rows:#?}"
    );
    assert_eq!(
        release(&mut app, at),
        confirm(SelectorKind::Model, "acme/model-07")
    );
}

// ----------------------------------------------------------------------------- list selectors --

#[test]
fn clicking_a_show_images_row_applies_it() {
    let mut app = fullscreen_app();
    app.state_mut().show_images = true;
    app.open_selector(SelectorKind::ShowImages);
    let no = cell(&mut app, "No ");
    press(&mut app, no);
    assert!(marked(&mut app, "No ", "→"));
    release(&mut app, no);
    assert!(!app.state_mut().show_images, "No was activated");
    assert_eq!(app.active_selector_kind(), None);
}

/// The theme picker previews on every move, so a press and a wheel notch re-theme the UI and a
/// click persists the choice.
#[test]
fn the_theme_picker_previews_on_press_and_wheel_and_applies_on_click() {
    let mut app = fullscreen_app();
    app.open_selector(SelectorKind::Theme);
    let rows = screen(&mut app);
    let slot = app.state_mut().regions.slot;
    let names: Vec<String> = rows
        .iter()
        .skip(usize::from(slot.y))
        .filter_map(|r| r.trim_start_matches("→ ").split_whitespace().next())
        .filter(|n| crate::UiTheme::builtin(n).name == *n)
        .map(str::to_string)
        .collect();
    assert!(names.len() >= 2, "builtin themes are listed: {rows:#?}");
    let start = app.state_mut().theme.name.clone();
    let other = names.iter().find(|n| **n != start).unwrap().clone();

    let at = cell(&mut app, &other);
    press(&mut app, at);
    assert_eq!(app.state_mut().theme.name, other, "press previews");
    // Hover leaves the preview where it is.
    let again = cell(&mut app, &start);
    hover(&mut app, again);
    assert_eq!(app.state_mut().theme.name, other);

    // A wheel notch previews the next theme down the list, as an arrow key would.
    let after = names.iter().position(|n| *n == other).unwrap() + 1;
    let over = cell(&mut app, &other);
    if let Some(next) = names.get(after) {
        wheel(&mut app, true, over);
        assert_eq!(app.state_mut().theme.name, *next, "wheel previews");
        wheel(&mut app, false, over);
        assert_eq!(app.state_mut().theme.name, other);
    }

    let at = cell(&mut app, &other);
    let action = release(&mut app, at);
    assert_eq!(
        action,
        AppAction::Command(AppCommand::ApplySetting {
            id: "theme".into(),
            value: other.clone(),
        })
    );
    assert_eq!(app.active_selector_kind(), None);
}

#[test]
fn a_data_list_with_hint_and_spacers_hits_its_own_rows() {
    let mut app = fullscreen_app();
    let rows = vec![
        ("none".to_string(), "No summary".to_string(), None),
        ("sum".to_string(), "Summarize".to_string(), None),
        (
            "custom".to_string(),
            "Summarize with custom prompt".to_string(),
            None,
        ),
    ];
    app.open_data_selector(SelectorKind::BranchSummary, rows, 0);
    let screen_rows = screen(&mut app);
    // The title and the hint row are not options.
    let title = (2, row_of(&screen_rows, "Summarize branch?") as u16);
    assert_eq!(click(&mut app, title), AppAction::None);
    let hint = (2, row_of(&screen_rows, "navigate") as u16);
    assert_eq!(click(&mut app, hint), AppAction::None);
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::BranchSummary)
    );

    let custom = cell(&mut app, "Summarize with custom prompt");
    assert_eq!(
        click(&mut app, custom),
        confirm(SelectorKind::BranchSummary, "custom")
    );
}

#[test]
fn clicking_a_thinking_level_applies_it() {
    let mut app = fullscreen_app();
    app.state_mut().available_thinking_levels = ["off", "low", "high"].map(str::to_string).to_vec();
    app.state_mut().thinking_level = "low".into();
    app.open_selector(SelectorKind::Thinking);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Thinking));

    let rows = screen(&mut app);
    // Neither the title nor the cycle sentence is a level.
    let title = (2, row_of(&rows, "Thinking Level") as u16);
    assert_eq!(click(&mut app, title), AppAction::None);

    let high = cell(&mut app, "high");
    press(&mut app, high);
    assert!(marked(&mut app, "Deep reasoning", "→"));
    assert_eq!(
        release(&mut app, high),
        AppAction::Command(AppCommand::SetThinking("high".into()))
    );
    assert_eq!(app.active_selector_kind(), None);
}

#[test]
fn the_wheel_steps_the_thinking_highlight_without_wrapping() {
    let mut app = fullscreen_app();
    app.state_mut().available_thinking_levels = ["off", "low", "high"].map(str::to_string).to_vec();
    app.state_mut().thinking_level = "off".into();
    app.open_selector(SelectorKind::Thinking);
    let over = cell(&mut app, "low");
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "No reasoning", "→"), "no wrap upward");
    for _ in 0..4 {
        wheel(&mut app, true, over);
    }
    assert!(
        marked(&mut app, "Deep reasoning", "→"),
        "clamped at the last"
    );
}

#[test]
fn a_submenu_step_confirms_the_clicked_row() {
    let mut app = fullscreen_app();
    app.state_mut().pending_model_thinking = Some("acme/model-01".into());
    app.open_submenu_child_selector(
        SelectorKind::ModelThinkingLevel,
        "Per-Model Thinking Level".into(),
        "Pick a level",
        1,
        1,
        false,
        crate::ColumnLayout::SLASH,
        vec![
            ("low".into(), "low".into(), Some("Light".into())),
            ("high".into(), "high".into(), Some("Deep".into())),
        ],
        0,
        None,
    );
    let rows = screen(&mut app);
    let title = (2, row_of(&rows, "Per-Model Thinking Level") as u16);
    assert_eq!(click(&mut app, title), AppAction::None);
    let high = cell(&mut app, "high");
    assert_eq!(
        click(&mut app, high),
        AppAction::Command(AppCommand::SetModelThinkingLevel {
            model: "acme/model-01".into(),
            level: "high".into(),
        })
    );
}

// ------------------------------------------------------------------------------------ /settings --

fn settings_app(extra: usize) -> App<TestBackend> {
    let mut app = fullscreen_app();
    let mut rows = vec![
        SettingRow::toggle("terminal.showImages", "Show images", true),
        SettingRow::choice(
            "steeringMode",
            "Steering mode",
            "one-at-a-time",
            vec!["all".to_string(), "one-at-a-time".to_string()],
        ),
        SettingRow::submenu("theme", "Theme", "dark", "theme"),
    ];
    for i in 0..extra {
        rows.push(SettingRow::toggle(
            format!("flag.{i}"),
            format!("Flag {i:02}"),
            false,
        ));
    }
    app.open_boxed_selector(
        SelectorKind::Settings,
        Box::new(SettingsSelector::new("Settings", rows)),
    );
    app
}

#[test]
fn clicking_a_toggle_row_cycles_it_in_place() {
    let mut app = settings_app(0);
    let at = cell(&mut app, "Show images");
    press(&mut app, at);
    assert!(marked(&mut app, "Show images", "→"));
    assert_eq!(
        release(&mut app, at),
        AppAction::Command(AppCommand::ApplySetting {
            id: "terminal.showImages".into(),
            value: "false".into(),
        })
    );
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Settings));
    let rows = screen(&mut app);
    assert!(rows[row_of(&rows, "Show images")].contains("false"));
}

#[test]
fn clicking_a_submenu_row_opens_the_submenu() {
    let mut app = settings_app(0);
    let at = cell(&mut app, "Theme");
    click(&mut app, at);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Theme));
}

#[test]
fn the_settings_search_row_and_hint_are_not_rows() {
    let mut app = settings_app(0);
    // Control: the pointer is live — a press on a setting lights it.
    let steering = cell(&mut app, "Steering mode");
    press(&mut app, steering);
    assert!(marked(&mut app, "Steering mode", "→"), "control press");

    let rows = screen(&mut app);
    let slot = app.state_mut().regions.slot;
    let search = rows
        .iter()
        .position(|r| r.trim_end() == ">")
        .expect("the search row");
    let hint = row_of(&rows, "Type to search");
    for y in [slot.y as usize, search, search + 1, hint] {
        let at = (2, y as u16);
        press(&mut app, at);
        assert_eq!(release(&mut app, at), AppAction::None, "row {y}: {rows:#?}");
        assert!(
            marked(&mut app, "Steering mode", "→"),
            "row {y} moved nothing"
        );
    }
}

#[test]
fn the_settings_wheel_moves_without_wrapping_and_hover_does_not_move() {
    let mut app = settings_app(0);
    let over = cell(&mut app, "Steering mode");
    hover(&mut app, over);
    assert!(marked(&mut app, "Show images", "→"));
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "Show images", "→"), "no wrap upward");
    for _ in 0..5 {
        wheel(&mut app, true, over);
    }
    assert!(marked(&mut app, "Theme", "→"), "clamped at the last");
    // The search row is the search box's, not the list's.
    let rows = screen(&mut app);
    let search = rows.iter().position(|r| r.trim_end() == ">").unwrap();
    wheel(&mut app, false, (2, search as u16));
    assert!(marked(&mut app, "Theme", "→"));
}

#[test]
fn a_scrolled_settings_list_maps_cells_to_absolute_indices() {
    let mut app = settings_app(12);
    let over = cell(&mut app, "Show images");
    for _ in 0..13 {
        wheel(&mut app, true, over);
    }
    let rows = screen(&mut app);
    assert!(
        rows.iter().all(|r| !r.contains("Show images")),
        "the window moved past the first rows: {rows:#?}"
    );
    let at = cell(&mut app, "Flag 09");
    assert_eq!(
        click(&mut app, at),
        AppAction::Command(AppCommand::ApplySetting {
            id: "flag.9".into(),
            value: "true".into(),
        })
    );
}

// ----------------------------------------------------------------------------------- /resume --

fn session_rows() -> Vec<SessionRow> {
    ["alpha", "bravo", "charlie"]
        .iter()
        .enumerate()
        .map(|(i, name)| SessionRow {
            path: format!("/s/{name}.jsonl"),
            label: format!("session {name}"),
            name: None,
            desc: Some(format!("{} msgs", i + 1)),
            search_text: format!("{name} session {name}"),
            recency: 10 - i as u128,
        })
        .collect()
}

#[test]
fn clicking_a_session_row_resumes_it() {
    let mut app = fullscreen_app();
    app.open_boxed_selector(
        SelectorKind::Session,
        Box::new(SessionSelector::new(session_rows())),
    );
    let rows = screen(&mut app);
    // The header and the hint rows are not sessions.
    let header = (2, row_of(&rows, "Resume Session") as u16);
    assert_eq!(click(&mut app, header), AppAction::None);

    let at = cell(&mut app, "session bravo");
    press(&mut app, at);
    assert!(marked(&mut app, "session bravo", "›"));
    assert_eq!(
        release(&mut app, at),
        confirm(SelectorKind::Session, "/s/bravo.jsonl")
    );
}

#[test]
fn the_session_wheel_does_not_wrap_and_hover_does_not_move() {
    let mut app = fullscreen_app();
    app.open_boxed_selector(
        SelectorKind::Session,
        Box::new(SessionSelector::new(session_rows())),
    );
    let over = cell(&mut app, "session bravo");
    hover(&mut app, over);
    assert!(marked(&mut app, "session alpha", "›"));
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "session alpha", "›"), "no wrap upward");
    for _ in 0..5 {
        wheel(&mut app, true, over);
    }
    assert!(marked(&mut app, "session charlie", "›"), "clamped");
}

/// While a delete is waiting for its yes/no, a click is not a resume.
#[test]
fn a_pending_delete_confirmation_owns_the_dialog() {
    let mut app = fullscreen_app();
    app.open_boxed_selector(
        SelectorKind::Session,
        Box::new(SessionSelector::new(session_rows())),
    );
    // Control: before the confirmation is armed a press lights a row.
    let bravo = cell(&mut app, "session bravo");
    press(&mut app, bravo);
    assert!(marked(&mut app, "session bravo", "›"), "control press");
    // Ctrl+D asks "Delete session?" about the highlighted row.
    app.handle_input(&InputEvent::Key(KeyEvent::new(
        KeyCode::Char('d'),
        KeyModifiers::CONTROL,
    )));
    let rows = screen(&mut app);
    assert!(
        rows.iter().any(|r| r.contains("Delete session?")),
        "{rows:#?}"
    );
    let charlie = cell(&mut app, "session charlie");
    assert_eq!(click(&mut app, charlie), AppAction::None);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Session));
    assert!(
        marked(&mut app, "session bravo", "›"),
        "the click moved nothing"
    );
}

// -------------------------------------------------------------------------------------- /tree --

fn tree_app(nodes: usize) -> App<TestBackend> {
    let mut app = fullscreen_app();
    let nodes = (0..nodes)
        .map(|i| TreeNode::message(format!("e{i}"), 0, format!("entry number {i:02}")))
        .collect();
    app.open_boxed_selector(SelectorKind::Tree, Box::new(TreeSelector::new(nodes)));
    app
}

#[test]
fn clicking_a_tree_row_navigates_to_that_entry() {
    let mut app = tree_app(6);
    let rows = screen(&mut app);
    // The header, the search line and the help line are not entries.
    for needle in ["Session Tree", "Type to search"] {
        let at = (2, row_of(&rows, needle) as u16);
        assert_eq!(click(&mut app, at), AppAction::None, "{needle}");
    }
    let at = cell(&mut app, "entry number 03");
    press(&mut app, at);
    assert!(marked(&mut app, "entry number 03", "›"));
    assert_eq!(release(&mut app, at), confirm(SelectorKind::Tree, "e3"));
}

#[test]
fn the_tree_wheel_does_not_wrap_and_hover_does_not_move() {
    let mut app = tree_app(4);
    let over = cell(&mut app, "entry number 02");
    hover(&mut app, over);
    assert!(marked(&mut app, "entry number 00", "›"));
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "entry number 00", "›"), "no wrap upward");
    for _ in 0..6 {
        wheel(&mut app, true, over);
    }
    assert!(marked(&mut app, "entry number 03", "›"), "clamped");
}

/// The window can be larger than the body a short slot leaves; every painted row maps to its own
/// entry and nothing past the body is hit.
#[test]
fn a_long_tree_maps_painted_rows_to_absolute_entries() {
    let mut app = tree_app(60);
    for _ in 0..30 {
        press_key(&mut app, KeyCode::Down);
    }
    let rows = screen(&mut app);
    assert!(
        rows.iter().all(|r| !r.contains("entry number 00")),
        "scrolled: {rows:#?}"
    );
    let at = cell(&mut app, "entry number 31");
    assert_eq!(click(&mut app, at), confirm(SelectorKind::Tree, "e31"));
}

#[test]
fn the_label_editor_owns_the_tree_body() {
    let mut app = tree_app(4);
    // Control: before the editor opens a press lights an entry.
    let two = cell(&mut app, "entry number 02");
    press(&mut app, two);
    assert!(marked(&mut app, "entry number 02", "›"), "control press");

    app.handle_input(&InputEvent::Key(KeyEvent::new(
        KeyCode::Char('L'),
        KeyModifiers::SHIFT,
    )));
    let rows = screen(&mut app);
    assert!(
        rows.iter().any(|r| r.contains("Label (empty to remove)")),
        "the label editor is open: {rows:#?}"
    );
    let at = (2, row_of(&rows, "Label (empty to remove)") as u16);
    assert_eq!(click(&mut app, at), AppAction::None);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Tree));
    // Leaving the editor shows the highlight where it was.
    press_key(&mut app, KeyCode::Esc);
    assert!(marked(&mut app, "entry number 02", "›"));
}

// ------------------------------------------------------------------------------ /login, /logout --

fn provider(id: &str, name: &str) -> LoginProviderOption {
    LoginProviderOption {
        id: ProviderId::from(id),
        name: name.to_string(),
        auth_type: AuthType::ApiKey,
        method_name: None,
        login_label: None,
        supports_login: true,
        status: None,
    }
}

fn login_app(search: Option<&str>) -> App<TestBackend> {
    let mut app = fullscreen_app();
    let options = [
        provider("anthropic", "Anthropic"),
        provider("openai", "OpenAI"),
        provider("zeta", "Zeta Cloud"),
    ];
    app.open_boxed_selector(
        SelectorKind::Login,
        Box::new(OAuthSelector::new(
            OAuthMode::Login,
            &options,
            search.map(str::to_string),
        )),
    );
    app
}

#[test]
fn clicking_a_provider_row_carries_its_option_index() {
    let mut app = login_app(None);
    let rows = screen(&mut app);
    let title = (2, row_of(&rows, "Select provider to configure") as u16);
    assert_eq!(click(&mut app, title), AppAction::None);
    let at = cell(&mut app, "OpenAI");
    press(&mut app, at);
    assert!(marked(&mut app, "OpenAI", "→"));
    assert_eq!(release(&mut app, at), confirm(SelectorKind::Login, "1"));
}

/// The fuzzy filter reorders and narrows the rows; the confirm value is still the index into the
/// ORIGINAL options.
#[test]
fn a_filtered_provider_list_confirms_the_original_index() {
    let mut app = login_app(Some("zeta"));
    let at = cell(&mut app, "Zeta Cloud");
    assert_eq!(click(&mut app, at), confirm(SelectorKind::Login, "2"));
}

#[test]
fn the_provider_wheel_does_not_wrap() {
    let mut app = login_app(None);
    let over = cell(&mut app, "OpenAI");
    hover(&mut app, over);
    assert!(marked(&mut app, "Anthropic", "→"));
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "Anthropic", "→"));
    for _ in 0..5 {
        wheel(&mut app, true, over);
    }
    assert!(marked(&mut app, "Zeta Cloud", "→"));
}

// -------------------------------------------------------------------------------------- /trust --

#[test]
fn clicking_a_trust_option_confirms_its_index() {
    let mut app = fullscreen_app();
    app.open_boxed_selector(
        SelectorKind::Trust,
        Box::new(TrustSelector::new(
            "/home/me/project",
            "none",
            false,
            vec![
                "Trust this folder".to_string(),
                "Trust the parent folder".to_string(),
                "Do not trust".to_string(),
            ],
            0,
        )),
    );
    let rows = screen(&mut app);
    // The header block is not an option.
    for needle in ["Project trust", "/home/me/project", "Saved decision"] {
        let at = (3, row_of(&rows, needle) as u16);
        assert_eq!(click(&mut app, at), AppAction::None, "{needle}");
    }
    let at = cell(&mut app, "Trust the parent folder");
    press(&mut app, at);
    assert!(marked(&mut app, "Trust the parent folder", "→"));
    assert_eq!(release(&mut app, at), confirm(SelectorKind::Trust, "1"));
}

#[test]
fn the_trust_wheel_stops_at_the_ends() {
    let mut app = fullscreen_app();
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
    let over = cell(&mut app, "Do not trust");
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "Trust", "→"));
    wheel(&mut app, true, over);
    wheel(&mut app, true, over);
    assert!(marked(&mut app, "Do not trust", "→"));
}

// --------------------------------------------------------------------------------------- /fork --

fn fork_app() -> App<TestBackend> {
    let mut app = fullscreen_app();
    let rows = ["first question", "second question", "third question"]
        .iter()
        .enumerate()
        .map(|(i, text)| UserMessageRow {
            id: format!("e{}", i + 1),
            text: (*text).to_string(),
        })
        .collect();
    app.open_boxed_selector(
        SelectorKind::UserMessage,
        Box::new(UserMessageSelector::new(rows, Some("e1"))),
    );
    app
}

/// Each message is three rows — the text, `Message i of N`, a blank. The first two belong to the
/// message; the blank between messages is nobody's.
#[test]
fn a_fork_message_answers_on_its_text_and_its_position_row_but_not_the_gap() {
    let mut app = fork_app();
    let rows = screen(&mut app);
    let text_y = row_of(&rows, "second question") as u16;
    let slot = app.state_mut().regions.slot;
    assert!(slot.contains(Position::new(2, text_y)));
    assert!(
        rows[usize::from(text_y) + 1].contains("Message 2 of 3"),
        "{rows:#?}"
    );
    assert!(rows[usize::from(text_y) + 2].trim().is_empty(), "{rows:#?}");

    let gap = (2, text_y + 2);
    assert_eq!(click(&mut app, gap), AppAction::None);
    assert!(
        marked(&mut app, "first question", "›"),
        "the gap moved nothing"
    );

    let position = (2, text_y + 1);
    press(&mut app, position);
    assert!(marked(&mut app, "second question", "›"));
    assert_eq!(
        release(&mut app, position),
        confirm(SelectorKind::UserMessage, "e2")
    );
}

#[test]
fn clicking_the_text_of_a_fork_message_forks_from_it() {
    let mut app = fork_app();
    let at = cell(&mut app, "third question");
    assert_eq!(
        click(&mut app, at),
        confirm(SelectorKind::UserMessage, "e3")
    );
}

#[test]
fn the_fork_header_and_wheel() {
    let mut app = fork_app();
    let rows = screen(&mut app);
    let title = (2, row_of(&rows, "Fork from Message") as u16);
    assert_eq!(click(&mut app, title), AppAction::None);
    let over = cell(&mut app, "second question");
    hover(&mut app, over);
    assert!(marked(&mut app, "first question", "›"));
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "first question", "›"), "no wrap upward");
    for _ in 0..5 {
        wheel(&mut app, true, over);
    }
    assert!(marked(&mut app, "third question", "›"), "clamped");
}

// ------------------------------------------------------------------------------ /scoped-models --

fn scoped_app(ids: &[&str]) -> App<TestBackend> {
    let mut app = fullscreen_app();
    let catalog = ids
        .iter()
        .map(|id| {
            (
                (*id).to_string(),
                format!("Name of {id}"),
                "acme".to_string(),
                None,
            )
        })
        .collect();
    app.open_checkbox_selector(catalog, None);
    app
}

#[test]
fn clicking_a_scoped_model_toggles_it_and_keeps_the_dialog_open() {
    let mut app = scoped_app(&["m-one", "m-two", "m-three"]);
    let rows = screen(&mut app);
    let title = (2, row_of(&rows, "Model Configuration") as u16);
    assert_eq!(click(&mut app, title), AppAction::None);

    let at = cell(&mut app, "m-two [");
    press(&mut app, at);
    assert!(marked(&mut app, "m-two [", "→"));
    // Toggling out of "all enabled" starts an explicit set with only that model.
    assert_eq!(release(&mut app, at), AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ScopedModels));
    let rows = screen(&mut app);
    assert!(rows[row_of(&rows, "m-two [")].contains('✓'), "{rows:#?}");
    assert!(rows[row_of(&rows, "m-one [")].contains('✗'), "{rows:#?}");
    assert!(rows[row_of(&rows, "m-three [")].contains('✗'), "{rows:#?}");
}

#[test]
fn a_wrapped_scoped_model_row_is_one_item_and_shifts_the_rows_below_it() {
    let long = "q".repeat(100);
    let mut app = scoped_app(&["m-one", &long, "m-three"]);
    // Highlight the wrapped model first: its name is wrapped under the list, which grows the
    // bottom-anchored dock, and the rows must not move between the press and the release below.
    press_key(&mut app, KeyCode::Down);
    let rows = screen(&mut app);
    // The long id cannot fit beside its cursor, so the item takes three rows: the cursor, the id
    // cut at the width, the rest of the id and the badge.
    let start = row_of(&rows, "m-one [") + 1;
    assert!(rows[start + 1].starts_with("qqqq"), "{rows:#?}");
    let three = row_of(&rows, "m-three [");
    assert_eq!(
        three,
        start + 3,
        "the wrapped item took three rows: {rows:#?}"
    );

    // Clicking the LAST row of the wrapped item toggles that model, not the one below it.
    let last_row = (2, (start + 2) as u16);
    click(&mut app, last_row);
    let rows = screen(&mut app);
    let ticks = |rows: &[String]| -> Vec<bool> {
        ["m-one [", "q [acme]", "m-three ["]
            .iter()
            .map(|n| rows[row_of(rows, n)].contains('✓'))
            .collect()
    };
    assert_eq!(ticks(&rows), [false, true, false], "{rows:#?}");

    // The row below the wrapped item lands on its own item.
    let at = cell(&mut app, "m-three [");
    click(&mut app, at);
    let rows = screen(&mut app);
    assert_eq!(ticks(&rows), [false, true, true], "{rows:#?}");
}

#[test]
fn the_scoped_models_wheel_does_not_wrap() {
    let mut app = scoped_app(&["m-one", "m-two", "m-three"]);
    let over = cell(&mut app, "m-two [");
    hover(&mut app, over);
    assert!(marked(&mut app, "m-one [", "→"));
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "m-one [", "→"));
    for _ in 0..5 {
        wheel(&mut app, true, over);
    }
    assert!(marked(&mut app, "m-three [", "→"));
}

// ------------------------------------------------------------------------------------- /config --

fn config_row(scope: ConfigScope, kind: ConfigKind, name: &str, base: &str) -> ConfigRow {
    ConfigRow {
        scope,
        kind,
        display_name: name.to_string(),
        pattern: format!("{}/{name}", kind.key()),
        base_dir: base.to_string(),
        enabled: false,
    }
}

fn config_app() -> App<TestBackend> {
    let mut app = fullscreen_app();
    let rows = vec![
        config_row(ConfigScope::User, ConfigKind::Extensions, "ext-one", "/u"),
        config_row(ConfigScope::User, ConfigKind::Skills, "skill-one", "/u"),
        config_row(ConfigScope::User, ConfigKind::Skills, "skill-two", "/u"),
        config_row(ConfigScope::Project, ConfigKind::Skills, "skill-proj", "/p"),
    ];
    app.open_boxed_selector(SelectorKind::Settings, Box::new(ConfigSelector::new(rows)));
    app
}

/// Group and subgroup headers are rows of the window, between the resources, but only the
/// resources answer.
#[test]
fn config_headers_are_not_resources_and_a_click_toggles_one() {
    let mut app = config_app();
    let rows = screen(&mut app);
    for needle in ["User (/u)", "Extensions", "Project (/p)"] {
        let at = (4, row_of(&rows, needle) as u16);
        assert_eq!(click(&mut app, at), AppAction::None, "{needle}");
        assert!(marked(&mut app, "ext-one", ">"), "{needle} moved nothing");
    }

    let at = cell(&mut app, "skill-two");
    press(&mut app, at);
    assert!(marked(&mut app, "skill-two", ">"));
    let action = release(&mut app, at);
    assert!(
        matches!(action, AppAction::Command(AppCommand::ApplySetting { .. })),
        "a toggle persists: {action:?}"
    );
    let rows = screen(&mut app);
    assert!(
        rows[row_of(&rows, "skill-two")].contains("[x]"),
        "{rows:#?}"
    );
    assert!(
        rows[row_of(&rows, "skill-one")].contains("[ ]"),
        "{rows:#?}"
    );
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Settings));
}

/// A wheel notch skips the headers between resources, as the arrow keys do.
#[test]
fn the_config_wheel_skips_headers_and_stops_at_the_ends() {
    let mut app = config_app();
    let over = cell(&mut app, "skill-one");
    wheel(&mut app, false, over);
    assert!(marked(&mut app, "ext-one", ">"), "no wrap upward");
    wheel(&mut app, true, over);
    assert!(
        marked(&mut app, "skill-one", ">"),
        "the subgroup header was skipped"
    );
    for _ in 0..6 {
        wheel(&mut app, true, over);
    }
    assert!(marked(&mut app, "skill-proj", ">"), "clamped at the last");
}
