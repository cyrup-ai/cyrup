//! Scoped-models checkbox+reorder selector tests (spec/tui/05 §6; `scoped-models-selector.ts`).
//!
//! Drives the bespoke `/scoped-models` selector through `App::handle_input`: the `✓`/`✗` checkbox
//! render over the full catalog, `Enter` **toggling** membership (not confirming), Alt+↑/↓ reorder of
//! the enabled cycle order, Ctrl+A/Ctrl+X enable/clear-all, and Ctrl+S confirming with the ordered
//! enabled set (or the `SCOPED_MODELS_ALL` sentinel).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{
    App, AppAction, AppCommand, CheckboxSelector, InputEvent, Selector, SelectorKind, UiTheme,
    SCOPED_MODELS_ALL,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn ctrl(c: char) -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}
fn alt(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::ALT))
}

/// A 3-model catalog: ids `m0`,`m1`,`m2` across two providers.
fn catalog() -> Vec<(String, String, String, Option<String>)> {
    vec![
        ("m0".into(), "Model Zero".into(), "openai".into(), Some("openai".into())),
        ("m1".into(), "Model One".into(), "openai".into(), Some("openai".into())),
        ("m2".into(), "Model Two".into(), "anthropic".into(), Some("anthropic".into())),
    ]
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

/// Pull the confirm value out of a `ConfirmSelection { ScopedModels, .. }` action.
fn confirm_value(action: AppAction) -> String {
    match action {
        AppAction::Command(AppCommand::ConfirmSelection { kind: SelectorKind::ScopedModels, value }) => {
            value
        }
        other => panic!("expected ScopedModels confirm, got {other:?}"),
    }
}

/// Rows, trailing-trimmed, from a direct render of the selector at its own natural height —
/// the presentation assertions below need the whole envelope, not the slice `App` happens to give
/// the dialog inside a 20-row terminal.
fn rows_of(sel: &mut CheckboxSelector, w: u16) -> (Vec<String>, ratatui::buffer::Buffer) {
    let theme = UiTheme::dark();
    let h = sel.desired_height(w);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
    let buf = term.backend().buffer().clone();
    let rows = (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            for x in 0..buf.area.width {
                if let Some(c) = buf.cell((x, y)) {
                    line.push_str(c.symbol());
                }
            }
            line.trim_end().to_string()
        })
        .collect();
    (rows, buf)
}

fn fg_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Option<ratatui::style::Color> {
    buf.cell((x, y)).map(|c| c.style().fg).unwrap_or(None)
}

/// The terminal COLUMN `needle` starts at within `row`. Counted in chars, never bytes — every row
/// here carries `→`/`·`/`✓`, so a `str::find` byte offset would point at the wrong cell (the
/// char-vs-column defect this crate has already had four times).
fn col_of(row: &str, needle: &str) -> u16 {
    let byte = row.find(needle).unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    row[..byte].chars().count() as u16
}

fn selector(enabled: Option<Vec<String>>) -> CheckboxSelector {
    CheckboxSelector::scoped_models(catalog(), enabled)
}

/// The APP-MOUNTED render: `/scoped-models` in the real editor slot, as opposed to the
/// component-level `rows_of` fixtures below. Every claim in the name is asserted here — the title
/// (`scoped-models-selector.ts:132`), the marker rows (`:245-259`) and the footer (`:154`,
/// `getFooterText` `:190-209`) — because the slot is the one place a row can be lost to the
/// envelope rather than to the component.
#[test]
fn app_mounted_render_shows_the_title_the_marker_rows_and_the_footer() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    // Explicit scope: only m0 enabled → markers must show (not the all-enabled blank form).
    app.open_checkbox_selector(catalog(), Some(vec!["m0".into()]));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ScopedModels));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Model Configuration"), "title missing:\n{text}");
    assert!(text.contains("m0 [openai] ✓"), "enabled row missing:\n{text}");
    assert!(text.contains("m1 [openai] ✗"), "disabled row missing:\n{text}");
    assert!(text.contains("enter toggle"), "footer hint missing (:154):\n{text}");
    assert!(text.contains("ctrl+s save"), "footer hint missing (:154):\n{text}");
}

// ---------------------------------------------------------------------------------------------
// S6 / S7 / S29 — what `ScopedModelsSelectorComponent` draws that the shared list engine does not
// ---------------------------------------------------------------------------------------------

/// **S6.** `scoped-models-selector.ts:245-259` composes each row as
/// `prefix + modelText + providerBadge + status`, so the enable marker is APPENDED — after the id
/// *and* after the ` [provider]` badge — and it is coloured: `theme.fg("success", " ✓")` when
/// enabled, `theme.fg("dim", " ✗")` when not (`:252-258`).
///
/// cyrup prepended `"✓ "` into the label and handed the result to `SelectList`, which drew it in the
/// row's base style: the id moved two columns right, the marker lost its colour, and the row read
/// `✓ Model Zero` — the model *name*, where upstream shows `item.model?.id` (`:249`).
#[test]
fn enable_marker_follows_the_provider_badge_and_carries_its_colour() {
    let mut sel = selector(Some(vec!["m0".into()]));
    let theme = UiTheme::dark();
    let (rows, buf) = rows_of(&mut sel, 76);
    assert_eq!(rows[7], "→ m0 [openai] ✓", "selected row (:248-259): {rows:?}");
    assert_eq!(rows[8], "  m1 [openai] ✗", "unselected row: {rows:?}");
    assert!(
        rows.iter().all(|r| !r.starts_with("✓ ") && !r.starts_with("✗ ")),
        "no prepended marker survives: {rows:?}"
    );
    // The two markers are DIFFERENT colours, and neither is the row's base style.
    assert_ne!(theme.success_style().fg, theme.dim_style().fg);
    let tick = col_of(&rows[7], "✓");
    assert_eq!(fg_at(&buf, tick, 7), theme.success_style().fg, "`✓` is success");
    let cross = col_of(&rows[8], "✗");
    assert_eq!(fg_at(&buf, cross, 8), theme.dim_style().fg, "`✗` is dim");
}

/// **S6.** `allEnabled ? "" : …` (`:252-255`): while every model is enabled there is no marker at
/// all, and the footer says so in words instead.
#[test]
fn all_enabled_draws_no_marker_and_says_so_in_the_footer() {
    let mut sel = selector(None);
    let (rows, _) = rows_of(&mut sel, 76);
    assert_eq!(rows[7], "→ m0 [openai]", "{rows:?}");
    assert!(rows.iter().all(|r| !r.contains('✓') && !r.contains('✗')), "{rows:?}");
    assert!(rows.iter().any(|r| r.ends_with("all enabled")), "countText (:194-196): {rows:?}");
}

/// **S7.** Title `theme.fg("accent", theme.bold("Model Configuration"))` at `paddingX 0` (`:132`),
/// muted subtitle (`:133-135`), the provider as a muted bracketed badge immediately after the id
/// (`:251`) and the highlighted model's *name* on its own `Model Name:` row (`:269-279`).
///
/// cyrup showed `" Scoped Models"` (a leading space upstream does not have, and the wrong words),
/// no subtitle, the provider in `SelectList`'s right-aligned description column, and never showed
/// the model name at all.
#[test]
fn title_subtitle_badge_and_model_name_row() {
    let mut sel = selector(Some(vec!["m0".into()]));
    let theme = UiTheme::dark();
    let (rows, buf) = rows_of(&mut sel, 76);
    assert_eq!(rows[2], "Model Configuration", "title (:132): {rows:?}");
    assert!(rows.iter().all(|r| !r.contains("Scoped Models")), "old title is gone: {rows:?}");
    assert_eq!(
        rows[3], "Session-only. ctrl+s to save to settings.",
        "subtitle (:133-135): {rows:?}"
    );
    assert_eq!(fg_at(&buf, 0, 2), theme.accent_style().fg, "title is accent");
    assert_eq!(fg_at(&buf, 0, 3), theme.muted_style().fg, "subtitle is muted");
    // The badge is adjacent to the id and muted — not a padded right-hand column.
    assert_eq!(rows[7], "→ m0 [openai] ✓", "badge sits right after the id: {rows:?}");
    let badge_x = col_of(&rows[7], "[openai]");
    assert_eq!(fg_at(&buf, badge_x, 7), theme.muted_style().fg, "badge is muted");
    assert_eq!(fg_at(&buf, badge_x - 1, 7), theme.muted_style().fg, "so is its leading space");
    // `Model Name:` shows the NAME; the rows above show ids.
    assert_eq!(rows[11], "  Model Name: Model Zero", "(:272-278): {rows:?}");
}

/// **S29.** `getFooterText` (`:190-209`): seven `·`-joined parts behind a TWO-space indent, every
/// key read from the live keymap, and `countText` last. cyrup's literal had a one-space indent, no
/// `provider` key, no count and no `(unsaved)`.
#[test]
fn footer_hint_names_every_key_plus_the_enabled_count() {
    let mut sel = selector(Some(vec!["m0".into()]));
    let (rows, _) = rows_of(&mut sel, 200);
    let footer = rows.iter().find(|r| r.contains("toggle")).expect("footer missing");
    assert_eq!(
        footer,
        "  enter toggle · ctrl+a all · ctrl+x clear · ctrl+p provider · \
alt+up/alt+down reorder · ctrl+s save · 1/3 enabled",
        "{rows:?}"
    );
}

/// **S29.** `unavailableCount` (`:192`, `:196`): an enabled id that is no longer in the catalog is
/// excluded from `enabledCount` and reported separately — and it still gets a row, ` [unavailable]`
/// with a dim `✗` (`:251`, `:258`), because `getSortedIds` (`:62-66`) keeps it.
#[test]
fn unavailable_enabled_ids_get_a_row_and_their_own_count() {
    let mut sel = selector(Some(vec!["gone".into(), "m0".into()]));
    let (rows, _) = rows_of(&mut sel, 200);
    assert_eq!(rows[7], "→ gone [unavailable] ✗", "{rows:?}");
    assert_eq!(rows[12], "  Model unavailable", "(:274): {rows:?}");
    let footer = rows.iter().find(|r| r.contains("toggle")).expect("footer missing");
    assert!(footer.ends_with("1/3 enabled · 1 unavailable"), "{footer:?}");
}

/// **S29.** `isDirty` (`:206-208`): a trailing space plus `theme.fg("warning", "(unsaved)")`.
#[test]
fn a_mutation_appends_an_unsaved_warning_in_warning_colour() {
    let mut sel = selector(Some(vec!["m0".into()]));
    let theme = UiTheme::dark();
    let (clean, _) = rows_of(&mut sel, 200);
    assert!(clean.iter().all(|r| !r.contains("(unsaved)")), "clean on open: {clean:?}");

    let km = cyrup_tui::SelectKeymap::default();
    sel.handle(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &km);
    let (rows, buf) = rows_of(&mut sel, 200);
    let y = rows.iter().position(|r| r.contains("toggle")).expect("footer missing") as u16;
    let footer = &rows[y as usize];
    assert!(footer.ends_with("0/3 enabled (unsaved)"), "{footer:?}");
    let x = col_of(footer, "(unsaved)");
    assert_eq!(fg_at(&buf, x, y), theme.warning_style().fg, "`(unsaved)` is warning");
    assert_eq!(fg_at(&buf, x - 2, y), theme.dim_style().fg, "the rest of the run is dim");
}

/// MIRROR — the per-component discipline this batch exists to keep. Every row above belongs to
/// `ScopedModelsSelectorComponent` alone. `ExtensionSelectorComponent` (`extension-selector.ts:
/// 44-75`) hosts the same shared `SelectList` and gets NONE of it: no `Model Configuration` title,
/// no subtitle, no `[provider]` badge, no enable marker, no `N/M enabled` count. If any of this
/// migrates into `SelectList`/`ListSelector`, this fails.
#[test]
fn none_of_this_leaks_into_the_shared_list_selector() {
    let mut sel = cyrup_tui::ListSelector::prompt(
        "Pick one".to_string(),
        vec![
            ("a".to_string(), "Alpha".to_string(), Some("anthropic".to_string())),
            ("b".to_string(), "Beta".to_string(), None),
        ],
        0,
    )
    .with_upstream_chrome(SelectorKind::ExtensionSelect, &cyrup_tui::SelectKeymap::default());
    let theme = UiTheme::dark();
    let h = sel.desired_height(60);
    let mut term = Terminal::new(TestBackend::new(60, h)).unwrap();
    term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
    let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    for needle in
        ["Model Configuration", "Session-only.", "[anthropic]", "enabled", "(unsaved)", "✗"]
    {
        assert!(!text.contains(needle), "{needle:?} leaked into the shared engine:\n{text}");
    }
}

#[test]
fn enter_toggles_membership_and_ctrl_s_confirms() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec![])); // start with nothing enabled
    // Highlight is on row0 (m0). Enter toggles it ON (does NOT confirm/close).
    app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ScopedModels), "Enter must not close");
    // Move down to m1, enable it too.
    app.handle_input(&key(KeyCode::Down));
    app.handle_input(&key(KeyCode::Enter));
    // Ctrl+S confirms with the ordered enabled set "m0\nm1".
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), "m0\nm1");
    assert_eq!(app.active_selector_kind(), None, "Ctrl+S closes the selector");
}

#[test]
fn alt_down_reorders_enabled_cycle_order() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec!["m0".into(), "m1".into()]));
    // Highlight m0 (row0); Alt+Down moves it down in cycle order → [m1, m0].
    app.handle_input(&alt(KeyCode::Down));
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), "m1\nm0");
}

#[test]
fn ctrl_a_enables_all_sentinel_and_ctrl_x_clears() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec!["m0".into()]));
    app.handle_input(&ctrl('a'));
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), SCOPED_MODELS_ALL, "Ctrl+A → all-enabled sentinel");

    // Reopen and clear all → empty confirm value.
    app.open_checkbox_selector(catalog(), None);
    app.handle_input(&ctrl('x'));
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), "", "Ctrl+X → empty scoped set");
}

#[test]
fn ctrl_p_toggles_whole_provider() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    // Start empty; highlight m0 (openai). Ctrl+P enables the whole openai provider (m0, m1).
    app.open_checkbox_selector(catalog(), Some(vec![]));
    app.handle_input(&ctrl('p'));
    let action = app.handle_input(&ctrl('s'));
    let value = confirm_value(action);
    assert!(value.contains("m0") && value.contains("m1"), "provider enable failed: {value:?}");
    assert!(!value.contains("m2"), "anthropic must stay disabled: {value:?}");
}

// ---------------------------------------------------------------------------------------------
// Behaviours the batch-9 rewrite added and nothing asserted — each was verified by disabling it
// and watching the whole suite stay green.
// ---------------------------------------------------------------------------------------------

/// A catalog large enough to overflow `maxVisible = 8` (`scoped-models-selector.ts:112`).
fn big_catalog(n: usize) -> Vec<(String, String, String, Option<String>)> {
    (0..n)
        .map(|i| {
            (format!("m{i:02}"), format!("Model {i}"), "openai".into(), Some("openai".into()))
        })
        .collect()
}

/// `updateList`'s scroll indicator (`scoped-models-selector.ts:263-267`):
/// `theme.fg("muted", \`  (${selectedIndex + 1}/${filteredItems.length})\`)`, emitted whenever the
/// `maxVisible = 8` window does not cover the filtered list. NOTHING asserted this row existed.
#[test]
fn scoped_models_reports_its_scroll_position_past_the_eight_row_window() {
    let mut sel = CheckboxSelector::scoped_models(big_catalog(12), None);
    let theme = UiTheme::dark();
    let (rows, buf) = rows_of(&mut sel, 60);
    let y = rows
        .iter()
        .position(|r| r.starts_with("  (1/12)"))
        .unwrap_or_else(|| panic!("no scroll row (:263-267): {rows:?}"));
    assert_eq!(rows[y], "  (1/12)", "the counters are 1-based over the FILTERED list: {rows:?}");
    assert_eq!(fg_at(&buf, 2, y as u16), theme.muted_style().fg, "muted (:264)");
    // Only eight model rows are drawn, and the readout tracks the highlight.
    assert_eq!(rows.iter().filter(|r| r.contains("[openai]")).count(), 8, "{rows:?}");

    // A list that fits gets no readout at all (`startIndex > 0 || endIndex < len`).
    let mut small = CheckboxSelector::scoped_models(big_catalog(4), None);
    let (rows, _) = rows_of(&mut small, 60);
    assert!(rows.iter().all(|r| !r.starts_with("  (")), "no readout when it fits: {rows:?}");
}

/// `config.refreshStatus` (`scoped-models-selector.ts:149-152`): a `muted` `  {status}` row between
/// the list's trailing `Spacer` (`:148`) and the footer (`:154`). `setRefreshStatus` (`:178-180`)
/// clears it with an empty string.
#[test]
fn scoped_models_draws_the_refresh_status_row_between_the_list_and_the_footer() {
    let mut sel = selector(Some(vec!["m0".into()]));
    let theme = UiTheme::dark();
    let (before, _) = rows_of(&mut sel, 76);
    assert!(before.iter().all(|r| !r.contains("Refreshing")), "{before:?}");

    sel.set_refresh_status("Refreshing model catalogs…");
    let (rows, buf) = rows_of(&mut sel, 76);
    let y = rows
        .iter()
        .position(|r| r == "  Refreshing model catalogs…")
        .unwrap_or_else(|| panic!("no refresh row (:150-151): {rows:?}"));
    assert_eq!(fg_at(&buf, 2, y as u16), theme.muted_style().fg, "muted (:151)");
    assert_eq!(rows[y - 1], "", "the list's own Spacer(1) above it (:148): {rows:?}");
    assert!(rows[y + 1].contains("toggle"), "the footer directly below it (:154): {rows:?}");

    sel.set_refresh_status("");
    let (rows, _) = rows_of(&mut sel, 76);
    assert!(rows.iter().all(|r| !r.contains("Refreshing")), "empty clears it: {rows:?}");
}

/// `tui.select.up`/`down` WRAP here (`scoped-models-selector.ts:286-297`:
/// `selectedIndex === 0 ? length - 1 : selectedIndex - 1`), unlike `/login`, which clamps. Nothing
/// asserted the wrap, so a clamp would have passed.
#[test]
fn scoped_models_navigation_wraps_at_both_ends() {
    let mut app = App::new(TestBackend::new(70, 24), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec![]));
    // Up from row 0 wraps to the LAST row; Enter toggles exactly that one.
    app.handle_input(&key(KeyCode::Up));
    app.handle_input(&key(KeyCode::Enter));
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), "m2", "Up at index 0 wrapped to the last row (:288)");

    // Down from the last row wraps back to the first.
    app.open_checkbox_selector(catalog(), Some(vec![]));
    for _ in 0..3 {
        app.handle_input(&key(KeyCode::Down));
    }
    app.handle_input(&key(KeyCode::Enter));
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), "m0", "Down at the last row wrapped to 0 (:294)");
}

/// **S5 regression.** `matchesKey(data, Key.ctrl("c"))` (`scoped-models-selector.ts:378-387`)
/// clears a NON-EMPTY search box and only cancels when it is already empty. The S6/S7 rewrite
/// dropped the arm, so the first Ctrl+C closed the dialog — and because cyrup's stock
/// `tui.select.cancel` binds `ctrl+c` alongside `esc`, it did so through the generic cancel path.
#[test]
fn ctrl_c_clears_a_non_empty_search_before_it_cancels() {
    let mut app = App::new(TestBackend::new(70, 24), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec![]));
    app.handle_input(&key(KeyCode::Char('m')));
    app.handle_input(&key(KeyCode::Char('2')));

    let first = app.handle_input(&ctrl('c'));
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::ScopedModels),
        "the FIRST Ctrl+C only clears the query (:380-382), it does not cancel: {first:?}"
    );
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("m0 [openai]"), "the full catalog is back:\n{text}");
    assert!(text.contains("m1 [openai]"), "the full catalog is back:\n{text}");

    let second = app.handle_input(&ctrl('c'));
    assert_eq!(
        app.active_selector_kind(),
        None,
        "the SECOND Ctrl+C cancels, the box now being empty (:383-385): {second:?}"
    );
}

/// Escape is unconditional (`scoped-models-selector.ts:390-392`) — it cancels even with a query in
/// the box, so the Ctrl+C arm above cannot be implemented by making `cancel` itself two-stage.
#[test]
fn escape_cancels_even_with_a_non_empty_search() {
    let mut app = App::new(TestBackend::new(70, 24), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec![]));
    app.handle_input(&key(KeyCode::Char('m')));
    app.handle_input(&key(KeyCode::Esc));
    assert_eq!(app.active_selector_kind(), None, "Esc always cancels (:390-392)");
}
