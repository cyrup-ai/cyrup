//! `/settings`, `/trust`, and the `/share` loader chrome (spec/tui/05 §6; gaps 1 + 2).
//!
//! Drive the new editor-swap selectors through the real `App::open_boxed_selector` + `handle_input`
//! routing and assert the rendered `TestBackend` buffer (full-width rules, title, label↔value
//! columns, the `→` cursor, the trust header) plus the routing outcomes (a settings cycle emits an
//! `ApplySetting` command and updates the displayed value in place; a trust confirm carries the chosen
//! option index; the bordered loader occupies the editor slot while a long op runs).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use super::harness::*;
use crate::crossterm::event::KeyCode;
use crate::{
    App, AppAction, AppCommand, SelectorKind, SettingRow, SettingsSelector, TrustSelector, UiTheme,
};
use ratatui::backend::TestBackend;

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

/// S16 — `SettingsSelectorComponent` is `DynamicBorder` / `SettingsList` / `DynamicBorder`
/// (`settings-selector.ts:765,873,874`) and nothing else. The previous revision asserted a
/// `"Settings"` **title row** that upstream does not draw; what the dialog actually leads with is
/// the `SettingsList`'s search `Input` (`settings-list.ts:94`), and it closes with `addHintLine`'s
/// search-enabled hint (`:242`).
#[test]
fn settings_selector_renders_search_rows_values_and_hint() {
    let mut app = settings_app();
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Settings));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        !text.lines().any(|l| l.trim() == "Settings"),
        "upstream draws no title row for /settings: {text}"
    );
    assert!(
        text.lines().any(|l| l.trim_end() == ">"),
        "the search Input: {text}"
    );
    assert!(text.contains("Show images"), "row label shown: {text}");
    assert!(text.contains("true"), "current value shown: {text}");
    assert!(
        text.contains("Type to search · Enter/Space to change · Esc to cancel"),
        "addHintLine (settings-list.ts:242): {text}"
    );
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
    assert!(
        buf_text(&app).contains("false"),
        "displayed value updated in place"
    );
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
                vec![
                    "auto".to_string(),
                    "websocket".to_string(),
                    "sse".to_string(),
                ],
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
    let labels = vec!["Trust".to_string(), "Do not trust".to_string()];
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
    assert!(
        text.contains("Saved decision: none"),
        "saved decision line: {text}"
    );
    assert!(
        text.contains("Current session: untrusted"),
        "session trust line: {text}"
    );
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
        Box::new(TrustSelector::new(
            "/p",
            "none",
            true,
            vec!["Trust".to_string()],
            0,
        )),
    );
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), None);
}

#[test]
fn bordered_loader_occupies_the_editor_slot_when_set() {
    let mut app = App::new(TestBackend::new(60, 12), UiTheme::dark()).unwrap();
    app.state_mut().loader = Some(crate::BorderedLoader::plain("Creating gist…"));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("Creating gist"),
        "loader message rendered: {text}"
    );
}

fn trust_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_boxed_selector(
        SelectorKind::Trust,
        Box::new(TrustSelector::new(
            "/home/me/project",
            "none",
            false,
            vec!["Trust".to_string(), "Do not trust".to_string()],
            0,
        )),
    );
    app.draw().unwrap();
    app
}

/// **S34.** `/trust`'s hint row joins its three pairs with the literal two-space string `"  "`
/// (`trust-selector.ts:76,79`) and names the arrows via `rawKeyHint("↑↓", "navigate")` (`:75`) —
/// `formatKeyText` splits on `/` and `+`, finds neither in `"↑↓"`, and joins back to `"↑↓"`
/// (`keybinding-hints.ts:17-27`), so there is no slash between them.
///
/// cyrup drew `" ↑/↓ navigate · enter save · esc cancel"`: `·` separators, a stray `/`, and `esc`
/// where `keyText("tui.select.cancel")` resolves to every bound key joined with `/`.
#[test]
fn trust_hint_row_uses_two_space_separators_and_bare_arrows() {
    let app = trust_app();
    let (_, row) = row_with(&app, "navigate");
    assert_eq!(
        row.trim_end(),
        " ↑↓ navigate  enter save  escape/ctrl+c cancel",
        "trust-selector.ts:74-83 verbatim"
    );
    assert!(!row.contains('·'), "no `·` separators upstream: {row:?}");
    assert!(
        !row.contains("↑/↓"),
        "`rawKeyHint(\"↑↓\", …)` has no slash: {row:?}"
    );
}

/// **S4.** Each hint pair is two-tone: `theme.fg("dim", keyText(kb)) + theme.fg("muted",
/// ` ${description}`)` (`keybinding-hints.ts:42-44`). cyrup painted the whole row one flat `dim`.
#[test]
fn trust_hint_pairs_are_dim_key_plus_muted_description() {
    let app = trust_app();
    let theme = UiTheme::dark();
    let (y, row) = row_with(&app, "navigate");
    let buf = app.terminal().backend().buffer();
    let col = |needle: &str| row.find(needle).map(|b| row[..b].chars().count()).unwrap() as u16;
    // The KEY runs are `dim`.
    for key_text in ["↑↓", "enter", "escape/ctrl+c"] {
        let x = col(key_text);
        assert_eq!(
            buf.cell((x, y)).unwrap().fg,
            theme.dim_style().fg.unwrap(),
            "key {key_text:?} must be dim"
        );
    }
    // The DESCRIPTION runs are `muted` — a different colour, or the two-tone split is invisible.
    for desc in ["navigate", "save", "cancel"] {
        let x = col(desc);
        assert_eq!(
            buf.cell((x, y)).unwrap().fg,
            theme.muted_style().fg.unwrap(),
            "description {desc:?} must be muted"
        );
    }
    assert_ne!(
        theme.dim_style().fg,
        theme.muted_style().fg,
        "the two tones must differ or this test proves nothing"
    );
}

/// `trust-selector.ts:113` wraps every option row in `new Text(…, 1, 0)`, so it carries the same
/// one-column left margin (`text.ts:70-76`) the header and hint rows do. cyrup started the option
/// rows at column 0, leaving the `→` cursor hanging one column left of the rest of the dialog.
#[test]
fn trust_option_rows_are_inset_one_column_like_the_header() {
    let app = trust_app();
    let (_, row) = row_with(&app, "→ Trust");
    assert!(
        row.starts_with(" → Trust"),
        "one-column inset (`Text(…, 1, 0)`): {row:?}"
    );
    let (_, header) = row_with(&app, "Project trust");
    assert!(
        header.starts_with(" Project trust"),
        "header is inset too: {header:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// `SettingsList` behaviours the rewrite added and nothing asserted — each was verified by
// disabling it and watching the whole suite stay green.
// ---------------------------------------------------------------------------------------------

/// More rows than `SettingsList`'s `maxVisible = 10` (`settings-selector.ts:767`).
fn many_settings(n: usize) -> Vec<SettingRow> {
    (0..n)
        .map(|i| SettingRow::toggle(format!("s{i:02}"), format!("Setting {i:02}"), true))
        .collect()
}

fn settings_app_with(rows: Vec<SettingRow>) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(70, 30), UiTheme::dark()).unwrap();
    app.open_boxed_selector(
        SelectorKind::Settings,
        Box::new(SettingsSelector::new("Settings", rows)),
    );
    app
}

/// `settings-list.ts:146-150`: when the `maxVisible` window does not cover the list,
/// `truncateToWidth(\`  (${selectedIndex + 1}/${displayItems.length})\`, width - 2, "")` in
/// `theme.hint`. The counters walk the FILTERED list, so narrowing the search changes the
/// denominator. NOTHING asserted this row existed.
#[test]
fn settings_reports_its_scroll_position_and_counts_the_filtered_rows() {
    let mut app = settings_app_with(many_settings(14));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("  (1/14)"),
        "no scroll readout (settings-list.ts:146-150):\n{text}"
    );
    assert_eq!(
        text.lines().filter(|l| l.contains("Setting ")).count(),
        10,
        "only maxVisible = 10 rows are drawn:\n{text}"
    );

    // It tracks the highlight...
    app.handle_input(&key(KeyCode::Down));
    app.draw().unwrap();
    assert!(buf_text(&app).contains("  (2/14)"), "{}", buf_text(&app));

    // ...and the denominator follows the SEARCH, not the full row set (`:148` reads
    // `displayItems`, which `applyFilter` has already narrowed at `:231-234`).
    app.handle_input(&key(KeyCode::Char('1')));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        !text.contains("/14)"),
        "the denominator must follow the filter:\n{text}"
    );

    // A list that fits the window gets no readout at all.
    let mut small = settings_app_with(many_settings(4));
    small.draw().unwrap();
    assert!(!buf_text(&small).contains("(1/4)"), "{}", buf_text(&small));
}

/// `settings-list.ts:179-184` — both arrows WRAP
/// (`selectedIndex === 0 ? items.length - 1 : selectedIndex - 1`). Nothing asserted it, so a clamp
/// would have passed.
#[test]
fn settings_navigation_wraps_at_both_ends() {
    let mut app = settings_app();
    // Two rows: Up from row 0 lands on "Steering mode", whose cycle set proves which row moved.
    app.handle_input(&key(KeyCode::Up));
    match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(AppCommand::ApplySetting { id, .. }) => {
            assert_eq!(
                id, "steeringMode",
                "Up at index 0 wrapped to the last row (:179-181)"
            );
        }
        other => panic!("expected ApplySetting, got {other:?}"),
    }

    let mut app = settings_app();
    app.handle_input(&key(KeyCode::Down));
    app.handle_input(&key(KeyCode::Down));
    match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(AppCommand::ApplySetting { id, .. }) => {
            assert_eq!(
                id, "terminal.showImages",
                "Down past the last row wrapped back to 0 (:182-184)"
            );
        }
        other => panic!("expected ApplySetting, got {other:?}"),
    }
}

// ---- TUI-N03 / TUI-032 / TUI-036 -------------------------------------------------------------

/// TUI-N03 — a theme chosen in `/settings` must PERSIST, not just repaint.
///
/// RED at HEAD: `confirm_selector`'s `SelectorKind::Theme` arm returned `None`, so no
/// `AppCommand::ApplySetting` ever reached the persist arm and the choice died with the process.
/// Pi distinguishes preview from confirm — `onThemePreview: (name) => themeController.preview(name)`
/// versus `onThemeChange: (t) => { this.settingsManager.setTheme(t); void
/// this.themeController.applyFromSettings(); }` (`interactive-mode.ts:4226-4231` @v0.83.0).
///
/// Worse in combination with TUI-004: `ThemeController::sync_with_terminal` persists an OSC-11
/// detection only when `settings.theme` is UNSET — exactly the state a never-persisted user choice
/// leaves behind — so the next launch overwrote it.
#[test]
fn confirming_a_theme_emits_an_apply_setting_command() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_selector(SelectorKind::Theme);
    // Drive the list to a row and confirm.
    let out = app.handle_input(&key(KeyCode::Enter));
    match out {
        AppAction::Command(AppCommand::ApplySetting { id, value }) => {
            assert_eq!(id, "theme", "the persist arm keys on `theme`");
            assert!(!value.is_empty(), "a theme name must ride along");
        }
        other => panic!("theme confirm must persist, got {other:?}"),
    }
}

/// TUI-032 — confirming the `Thinking level` submenu applies the level to the SESSION.
///
/// Pi's `onThinkingLevelChange` is `this.session.setThinkingLevel(level); this.footer.invalidate();
/// this.updateEditorBorderColor();` (`interactive-mode.ts:4222-4226`) — a session op, not a settings
/// write. RED at HEAD: the arm returned `None`, and `SelectorKind::Thinking` was unreachable anyway
/// because `open_selector` had exactly one call site and it only ever built `SelectorKind::Theme`.
#[test]
fn confirming_a_thinking_level_emits_a_set_thinking_command() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_selector(SelectorKind::Thinking);
    match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(AppCommand::SetThinking(level)) => {
            assert!(!level.is_empty(), "a level must ride along");
        }
        other => panic!("thinking confirm must reach the session, got {other:?}"),
    }
}

/// TUI-032 — the two submenu rows pi ships. `warnings` (`settings-selector.ts:578-590` @v0.83.0)
/// and `thinking` (`:591-611`) had no cyrup counterpart at all, so `warnings.anthropicExtraUsage`
/// — fully parsed and honoured by `cyrup-config` — could only be changed by hand-editing
/// `settings.json`.
#[test]
fn the_settings_grid_offers_the_warnings_and_thinking_submenus() {
    let rows = crate::app::settings_rows_for_test();
    assert!(
        rows.iter().any(|r| r.id == "warnings"),
        "pi's `warnings` submenu row is missing"
    );
    assert!(
        rows.iter().any(|r| r.id == "thinking"),
        "pi's `Thinking level` submenu row is missing"
    );
}

/// TUI-036 — `Show images` / `Image width` are offered ONLY on a terminal with an image protocol.
///
/// Pi: `// Only show image toggle if terminal supports it` / `if (supportsImages) { items.splice(1,
/// 0, {id:"show-images", …}); items.splice(2, 0, {id:"image-width-cells", …}); }`
/// (`settings-selector.ts:654-671` @v0.83.0). The neighbouring `auto-resize-images` row is
/// deliberately NOT gated — it is spliced at `supportsImages ? 3 : 1` — which is exactly the
/// distinction cyrup lost by pushing all three unconditionally.
///
/// RED at HEAD: `settings_rows` took no capability argument at all, so on a plain xterm both rows
/// were offered and could not change anything, and every row below them sat at a different index
/// from pi's.
#[test]
fn the_image_rows_are_gated_on_an_image_protocol() {
    let with = crate::app::settings_rows_for_test_with_images(true);
    assert!(with.iter().any(|r| r.id == "terminal.showImages"));
    assert!(with.iter().any(|r| r.id == "terminal.imageWidthCells"));

    let without = crate::app::settings_rows_for_test_with_images(false);
    assert!(
        !without.iter().any(|r| r.id == "terminal.showImages"),
        "no protocol ⇒ no `Show images` row"
    );
    assert!(
        !without.iter().any(|r| r.id == "terminal.imageWidthCells"),
        "no protocol ⇒ no `Image width` row"
    );
    assert!(
        without.iter().any(|r| r.id == "images.autoResize"),
        "`Auto-resize images` is NOT gated upstream"
    );
}
