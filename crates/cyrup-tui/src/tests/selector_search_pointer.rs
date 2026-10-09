//! The pointer over a selector's search box, driven the way a terminal drives it: a real fullscreen
//! `App` on a `TestBackend`, a query typed with real keys, and `Down`/`Up` reports sent through
//! `App::handle_input` at cells found in the drawn frame.
//!
//! Pi's `Input.handleMouse` (`packages/tui/src/components/input.ts:229-243` @v1.0.0) acts on a left
//! **press** only: the caret goes to the start of the grapheme whose cells hold the pointer
//! (`max(0, x - 2)` plus the scroll start the field was last drawn at), or to the end of the value
//! past it. A `Container` forwards a mouse event to the child under it, so the dialogs that add
//! their `Input` as a container child get this for free — `/model`, `/scoped-models`, `/login`,
//! `/logout`, the thinking picker, searchable settings submenus, the settings list, the extension
//! input, the login dialog's prompt and the rename panel — while `SessionList`, `ResourceList` and
//! `TreeSelector` draw their own search line and implement no `handleMouse`
//! (`session-selector.ts:283`, `config-selector.ts:228`, `tree-selector.ts:1165`), so there the
//! press is nobody's. The caret is read back from the drawn frame, as a user sees it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::Modifier;

use super::harness::{ctrl, key};
use crate::{
    App, AppAction, ConfigKind, ConfigRow, ConfigScope, ConfigSelector, InputEvent, LoginDialog,
    ModelEntry, OAuthMode, OAuthSelector, SelectKeymap, SelectorKind, SessionRow, SessionSelector,
    SettingRow, SettingsSelector, TextInputSelector, TreeNode, TreeSelector, UiTheme,
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

/// The screen row of the search box: the first one that starts with `prefix`.
fn row_starting(app: &mut App<TestBackend>, prefix: &str) -> u16 {
    let rows = screen(app);
    rows.iter()
        .position(|r| r.starts_with(prefix))
        .unwrap_or_else(|| panic!("no row starts with `{prefix}`: {rows:#?}")) as u16
}

/// Where the caret is drawn on screen row `y` of the frame just drawn, and the glyph under it: the
/// reversed cell, the one `Input.render` draws its fake cursor with.
fn caret(app: &mut App<TestBackend>, y: u16) -> (u16, String) {
    app.draw().unwrap();
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    let buf = alt.backend_for_test().buffer().clone();
    (0..COLS)
        .find(|&x| buf[(x, y)].modifier.contains(Modifier::REVERSED))
        .map(|x| (x, buf[(x, y)].symbol().to_string()))
        .unwrap_or_else(|| panic!("no caret on row {y}"))
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
    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), at))
}

fn release(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), at))
}

/// A whole gesture as a terminal delivers it: the press, the frame the run loop paints after it,
/// then the release. Answers the release's action.
fn click(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    press(app, at);
    app.draw().unwrap();
    release(app, at)
}

fn type_text(app: &mut App<TestBackend>, text: &str) {
    for c in text.chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
}

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

fn model_app() -> App<TestBackend> {
    let mut app = fullscreen_app();
    app.open_model_selector(catalog(5), None);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Model));
    app
}

/// Press at column `x` of the search row `y` and answer the caret it leaves.
fn press_caret(app: &mut App<TestBackend>, y: u16, x: u16) -> (u16, String) {
    click(app, (x, y));
    caret(app, y)
}

// ------------------------------------------------------------------------------------- /model --

#[test]
fn a_press_in_the_model_search_box_puts_the_caret_on_the_grapheme_under_it() {
    let mut app = model_app();
    type_text(&mut app, "model");
    let y = row_starting(&mut app, "> model");
    assert_eq!(
        caret(&mut app, y),
        (7, " ".into()),
        "typing leaves it at the end"
    );

    // The value starts two columns in, behind the prompt: column 4 holds the `d`.
    assert_eq!(press_caret(&mut app, y, 4), (4, "d".into()));
    assert_eq!(press_caret(&mut app, y, 6), (6, "l".into()));
}

#[test]
fn a_press_on_the_prompt_or_past_the_value_goes_to_the_start_or_the_end() {
    let mut app = model_app();
    type_text(&mut app, "model");
    let y = row_starting(&mut app, "> model");
    assert_eq!(
        press_caret(&mut app, y, 0),
        (2, "m".into()),
        "prompt column 0"
    );
    assert_eq!(
        press_caret(&mut app, y, 1),
        (2, "m".into()),
        "prompt column 1"
    );
    assert_eq!(
        press_caret(&mut app, y, 40),
        (7, " ".into()),
        "past the end"
    );
}

/// The caret decides where the next key lands, and the list refilters on it: a press at the start
/// then `m` makes `m01`, which still narrows the catalog to the one model it spells.
#[test]
fn typing_after_a_press_edits_at_the_caret_and_the_list_still_filters() {
    let mut app = model_app();
    type_text(&mut app, "01");
    let y = row_starting(&mut app, "> 01");
    click(&mut app, (2, y));
    type_text(&mut app, "m");
    let rows = screen(&mut app);
    assert!(rows[usize::from(y)].starts_with("> m01"), "{rows:#?}");
    assert!(rows.iter().any(|r| r.contains("model-01")), "{rows:#?}");
    assert!(
        rows.iter().all(|r| !r.contains("model-02")),
        "the query narrowed the list: {rows:#?}"
    );
}

/// The right half of a double-width glyph is still the glyph: the caret lands on its start.
#[test]
fn a_press_on_either_half_of_a_wide_glyph_lands_on_its_start() {
    let mut app = model_app();
    type_text(&mut app, "a\u{6f22}b");
    let y = row_starting(&mut app, "> a");
    // `a` is column 2, the glyph takes 3 and 4, `b` is column 5.
    assert_eq!(
        press_caret(&mut app, y, 3),
        (3, "\u{6f22}".into()),
        "left half"
    );
    assert_eq!(
        press_caret(&mut app, y, 4),
        (3, "\u{6f22}".into()),
        "right half"
    );
    assert_eq!(press_caret(&mut app, y, 5), (5, "b".into()));
}

/// A value wider than the field is drawn from a scroll start, and the press resolves against the
/// window that was drawn: with the caret at the end of a hundred columns the field shows columns
/// 23.. (pi's `startCol = totalWidth - scrollWidth`), so its first cell holds the `d` at index 23.
/// The press moves the caret there, which re-centres the window at 0 — and the click that ends the
/// gesture, on the same cell, must not resolve against that new window and move the caret again.
#[test]
fn a_press_in_a_scrolled_search_box_resolves_against_the_window_it_was_drawn_at() {
    let mut app = model_app();
    type_text(&mut app, &"abcdefghij".repeat(10));
    let y = row_starting(&mut app, "> ");
    let rows = screen(&mut app);
    assert!(
        rows[usize::from(y)].starts_with("> defghij"),
        "scrolled to column 23: {:?}",
        rows[usize::from(y)]
    );

    press(&mut app, (2, y));
    // Caret on index 23 (`d`); the window now starts at 0, so it is drawn at column 2 + 23.
    assert_eq!(caret(&mut app, y), (25, "d".into()), "after the press");
    release(&mut app, (2, y));
    assert_eq!(
        caret(&mut app, y),
        (25, "d".into()),
        "the click did not move it again"
    );
}

#[test]
fn the_model_search_box_claims_a_press_but_not_the_click_after_it() {
    let mut app = model_app();
    type_text(&mut app, "abc");
    let y = row_starting(&mut app, "> abc");
    assert_eq!(press(&mut app, (3, y)), AppAction::Redraw);
    assert_eq!(release(&mut app, (3, y)), AppAction::None);
}

// ------------------------------------------------------------------------- the other dialogs --

#[test]
fn a_press_in_the_thinking_search_box_places_the_caret() {
    let mut app = fullscreen_app();
    app.state_mut().available_thinking_levels = ["off", "low", "high"].map(str::to_string).to_vec();
    app.state_mut().thinking_level = "low".into();
    app.open_selector(SelectorKind::Thinking);
    type_text(&mut app, "hig");
    let y = row_starting(&mut app, "> hig");
    assert_eq!(press_caret(&mut app, y, 3), (3, "i".into()));
}

#[test]
fn a_press_in_the_settings_search_box_places_the_caret() {
    let mut app = fullscreen_app();
    let rows = vec![
        SettingRow::toggle("terminal.showImages", "Show images", true),
        SettingRow::submenu("theme", "Theme", "dark", "theme"),
    ];
    app.open_boxed_selector(
        SelectorKind::Settings,
        Box::new(SettingsSelector::new("Settings", rows)),
    );
    type_text(&mut app, "the");
    let y = row_starting(&mut app, "> the");
    assert_eq!(press_caret(&mut app, y, 3), (3, "h".into()));
    // Typing there is an edit at the caret, and the list refilters on it.
    type_text(&mut app, "m");
    let rows = screen(&mut app);
    assert!(rows[usize::from(y)].starts_with("> tmhe"), "{rows:#?}");
}

#[test]
fn a_press_in_the_provider_search_box_places_the_caret() {
    let mut app = fullscreen_app();
    let options =
        [("openai", "OpenAI"), ("anthropic", "Anthropic")].map(|(id, name)| LoginProviderOption {
            id: ProviderId::from(id),
            name: name.to_string(),
            auth_type: AuthType::ApiKey,
            method_name: None,
            login_label: None,
            supports_login: true,
            status: None,
            subscription: None,
        });
    app.open_boxed_selector(
        SelectorKind::Login,
        Box::new(OAuthSelector::new(OAuthMode::Login, &options, None)),
    );
    type_text(&mut app, "ope");
    let y = row_starting(&mut app, "> ope");
    assert_eq!(press_caret(&mut app, y, 3), (3, "p".into()));
}

#[test]
fn a_press_in_the_scoped_models_search_box_places_the_caret() {
    let mut app = fullscreen_app();
    let catalog = ["alpha", "beta"]
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
    type_text(&mut app, "alp");
    let y = row_starting(&mut app, "> alp");
    assert_eq!(press_caret(&mut app, y, 3), (3, "l".into()));
}

#[test]
fn a_press_in_a_searchable_submenu_places_the_caret() {
    let mut app = fullscreen_app();
    app.state_mut().pending_model_thinking = Some("acme/model-01".into());
    app.open_submenu_child_selector(
        SelectorKind::ModelThinkingLevel,
        "Per-Model Thinking Level".into(),
        "Pick a level",
        1,
        1,
        true,
        crate::ColumnLayout::SLASH,
        vec![
            ("low".into(), "low".into(), Some("Light".into())),
            ("high".into(), "high".into(), Some("Deep".into())),
        ],
        0,
        None,
    );
    type_text(&mut app, "hig");
    let y = row_starting(&mut app, "> hig");
    assert_eq!(press_caret(&mut app, y, 3), (3, "i".into()));
}

#[test]
fn a_press_in_the_extension_input_places_the_caret() {
    let mut app = fullscreen_app();
    app.open_boxed_selector(
        SelectorKind::ExtensionInput,
        Box::new(TextInputSelector::new("Name?".to_string(), None)),
    );
    type_text(&mut app, "hello");
    let y = row_starting(&mut app, "> hello");
    assert_eq!(press_caret(&mut app, y, 4), (4, "l".into()));
}

/// The prompt sits under a message that wraps over several rows; the field's row is found from the
/// wrapped height, not from the number of lines pushed.
#[test]
fn a_press_in_the_login_prompt_places_the_caret_below_wrapped_text() {
    let mut app = fullscreen_app();
    let mut dialog = LoginDialog::new("Login to acme", &SelectKeymap::default());
    dialog.show_prompt(&"paste the key ".repeat(14), None);
    app.open_boxed_selector(SelectorKind::LoginDialog, Box::new(dialog));
    type_text(&mut app, "abcdef");
    let y = row_starting(&mut app, "> abcdef");
    assert_eq!(press_caret(&mut app, y, 4), (4, "c".into()));
}

fn session_rows() -> Vec<SessionRow> {
    ["alpha", "bravo"]
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

/// The rename field is the rename panel's own `Input` (a container child upstream), so a press on it
/// places the caret; the value starts after the eight-column ` rename ` label.
#[test]
fn a_press_in_the_session_rename_field_places_the_caret() {
    let mut app = fullscreen_app();
    app.open_boxed_selector(
        SelectorKind::Session,
        Box::new(SessionSelector::new(session_rows())),
    );
    app.handle_input(&ctrl(KeyCode::Char('r')));
    type_text(&mut app, "abcdef");
    let y = row_starting(&mut app, " rename abcdef");
    assert_eq!(press_caret(&mut app, y, 10), (10, "c".into()));
}

// -------------------------------------------------------------- search boxes that take no press --

/// `SessionList` draws its search `Input` inside its own `render` and has no `handleMouse`
/// (`session-selector.ts:283`), so the press is nobody's: the caret stays where typing left it (a drag from there selects text).
#[test]
fn the_resume_search_box_takes_no_press() {
    let mut app = fullscreen_app();
    app.open_boxed_selector(
        SelectorKind::Session,
        Box::new(SessionSelector::new(session_rows())),
    );
    type_text(&mut app, "abc");
    let y = row_starting(&mut app, "> abc");
    // Nobody's press: no component moves the caret. The press is the dock selection's now, so
    // the frame repaints for the highlight it starts.
    let _ = press(&mut app, (3, y));
    assert_eq!(caret(&mut app, y), (5, " ".into()));
}

/// `ResourceList` (`config-selector.ts:228`) — same shape, same answer.
#[test]
fn the_config_search_box_takes_no_press() {
    let mut app = fullscreen_app();
    let row = ConfigRow {
        scope: ConfigScope::User,
        kind: ConfigKind::Skills,
        display_name: "skill-one".to_string(),
        pattern: "skills/skill-one".to_string(),
        base_dir: "/u".to_string(),
        enabled: false,
    };
    app.open_boxed_selector(
        SelectorKind::Settings,
        Box::new(ConfigSelector::new(vec![row])),
    );
    type_text(&mut app, "abc");
    let y = row_starting(&mut app, "> abc");
    // Nobody's press: no component moves the caret. The press is the dock selection's now, so
    // the frame repaints for the highlight it starts.
    let _ = press(&mut app, (3, y));
    assert_eq!(caret(&mut app, y), (5, " ".into()));
}

/// `TreeSelector`'s `SearchLine` is a bare `Component` that holds no `Input` at all
/// (`tree-selector.ts:1165`); there is no caret to place.
#[test]
fn the_tree_search_line_takes_no_press() {
    let mut app = fullscreen_app();
    let nodes = (0..3)
        .map(|i| TreeNode::message(format!("e{i}"), 0, format!("entry number {i:02}")))
        .collect();
    app.open_boxed_selector(SelectorKind::Tree, Box::new(TreeSelector::new(nodes)));
    type_text(&mut app, "abc");
    let y = row_starting(&mut app, "  Type to search: abc");
    // The press is the dock selection's, not the tree's; a click that selects nothing copies nothing.
    let _ = press(&mut app, (20, y));
    assert_eq!(release(&mut app, (20, y)), AppAction::None);
    let rows = screen(&mut app);
    assert_eq!(rows[usize::from(y)], "  Type to search: abc");
}
