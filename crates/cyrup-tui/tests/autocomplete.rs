//! Autocomplete + SelectList + fuzzy tests (spec/tui/04 §3-5; gaps 3/4).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{
    fuzzy_filter, fuzzy_match, fuzzy_score, App, ColumnLayout, EditorOutcome, InputEditor,
    SelectItem, SelectList, UiTheme,
};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn type_str(ed: &mut InputEditor, s: &str) {
    for c in s.chars() {
        ed.handle_key(&key(KeyCode::Char(c)));
    }
}

// ---- fuzzy --------------------------------------------------------------------------------

#[test]
fn fuzzy_subsequence_and_ordering() {
    // Non-subsequence → None; subsequence → Some. (Pi: lower score = better.)
    assert!(fuzzy_score("settings", "xyz").is_none());
    assert!(fuzzy_score("settings", "set").is_some());
    // Prefix/boundary match outranks a scattered one → LOWER score (fuzzy.ts:35-49).
    let prefix = fuzzy_score("settings", "set").unwrap();
    let scattered = fuzzy_score("scoped-models", "set").unwrap_or(f64::MAX);
    assert!(prefix < scattered, "prefix {prefix} should beat (be lower than) scattered {scattered}");
    // Empty query matches everything at score 0.
    assert_eq!(fuzzy_score("anything", ""), Some(0.0));
    // Query longer than text never matches (fuzzy.ts:21-23).
    assert!(fuzzy_score("se", "settings").is_none());
}

#[test]
fn fuzzy_exact_and_boundary_bonuses() {
    // Whole-string-exact gets the -100 bonus (fuzzy.ts:63-65), beating a mere prefix.
    let exact = fuzzy_match("set", "set").unwrap();
    let prefix = fuzzy_match("set", "settings").unwrap();
    assert!(exact < prefix - 90.0, "exact {exact} should be ~100 below prefix {prefix}");
    // Word-boundary match (after '-') earns the -10 bonus vs a non-boundary interior match.
    let boundary = fuzzy_match("m", "scoped-models").unwrap();
    let interior = fuzzy_match("e", "scoped-models").unwrap();
    assert!(boundary < interior, "boundary {boundary} should beat interior {interior}");
}

#[test]
fn fuzzy_alphanumeric_swap_fallback() {
    // "gpt4" should match "gpt-4o" directly; the swap fallback rescues "4gpt" → +5 penalty.
    let direct = fuzzy_match("gpt4", "gpt-4o");
    assert!(direct.is_some());
    let swapped = fuzzy_match("4gpt", "gpt-4o");
    assert!(swapped.is_some(), "alphanumeric-swap fallback should match (fuzzy.ts:75-92)");
    // The swapped retry is penalized by +5 over the equivalent direct query.
    assert!(swapped.unwrap() > direct.unwrap(), "swap retry carries +5 penalty");
    // No swap is possible for a pure-letter query that fails → None.
    assert!(fuzzy_match("zzz", "gpt-4o").is_none());
}

#[test]
fn fuzzy_filter_ranks_best_first() {
    let items = ["settings", "session", "scoped-models"];
    let ranked = fuzzy_filter(&items, "se", |s| *s);
    // "settings" and "session" both start with "se"; "scoped-models" matches s..e scattered.
    assert_eq!(ranked.first().map(|m| items[m.index]), Some("settings").or(Some("session")));
    assert!(ranked.iter().any(|m| items[m.index] == "session"));
    // Scattered match ranks last (highest score).
    assert_eq!(ranked.last().map(|m| items[m.index]), Some("scoped-models"));
}

#[test]
fn fuzzy_filter_requires_all_tokens() {
    // Multi-token query (whitespace/'/'-separated): every token must match (fuzzy.ts:120-128).
    let items = ["scoped models", "settings", "session"];
    let ranked = fuzzy_filter(&items, "sc mo", |s| *s);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked.first().map(|m| items[m.index]), Some("scoped models"));
    // Empty/whitespace query keeps every item in original order.
    let all = fuzzy_filter(&items, "   ", |s| *s);
    assert_eq!(all.iter().map(|m| m.index).collect::<Vec<_>>(), vec![0, 1, 2]);
}

// ---- SelectList ---------------------------------------------------------------------------

#[test]
fn select_list_wraps_navigation() {
    let items = vec![SelectItem::label("a"), SelectItem::label("b"), SelectItem::label("c")];
    let mut list = SelectList::new(items, ColumnLayout::DEFAULT);
    assert_eq!(list.selected(), 0);
    list.select_up(); // wraps to bottom
    assert_eq!(list.selected(), 2);
    list.select_down(); // wraps to top
    assert_eq!(list.selected(), 0);
}

#[test]
fn select_list_windows_and_indicates_scroll() {
    let items: Vec<SelectItem> = (0..22).map(|i| SelectItem::label(format!("cmd{i}"))).collect();
    let mut list = SelectList::new(items, ColumnLayout::SLASH);
    list.set_max_visible(5);
    let theme = UiTheme::dark();
    let lines = list.lines(60, &theme);
    // 5 rows + 1 scroll indicator.
    assert_eq!(lines.len(), 6);
    let last: String = lines[5].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(last.contains("(1/22)"), "scroll indicator missing: {last}");
    // Selected row carries the → glyph.
    let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(first.starts_with("→ "), "selection glyph missing: {first}");
}

// ---- slash autocomplete in the editor -----------------------------------------------------

#[test]
fn typing_slash_opens_command_popup() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/se");
    assert!(ed.autocomplete_open(), "slash popup did not open");
    let ac = ed.autocomplete().unwrap();
    // Top candidate matches "se" — settings or session.
    let top = ac.list.selected_item().unwrap();
    assert!(top.label == "settings" || top.label == "session", "unexpected top: {}", top.label);
}

#[test]
fn tab_accepts_slash_completion_with_trailing_space() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/sett");
    // Tab accepts the selected item, keeps editing.
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "/settings ");
    // The popup closed after acceptance left no slash context (there is a trailing space now).
    assert!(!ed.autocomplete_open());
}

#[test]
fn enter_on_slash_popup_submits_immediately() {
    // spec/tui/04 §5 edge 15: accepting a slash item with Enter submits.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/tre");
    let out = ed.handle_key(&key(KeyCode::Enter));
    assert_eq!(out, EditorOutcome::Submit("/tree".to_string()));
    assert!(ed.is_empty());
}

#[test]
fn esc_cancels_popup_keeps_text() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/mod");
    assert!(ed.autocomplete_open());
    ed.handle_key(&key(KeyCode::Esc));
    assert!(!ed.autocomplete_open());
    assert_eq!(ed.text(), "/mod");
}

#[test]
fn popup_renders_below_editor_in_viewport() {
    // The popup is appended below the editor in the live region (spec/tui/04 §7).
    let mut app = App::new(TestBackend::new(70, 16), UiTheme::dark()).unwrap();
    for c in "/se".chars() {
        app.editor_mut().handle_key(&key(KeyCode::Char(c)));
    }
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer();
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                text.push_str(cell.symbol());
            }
        }
        text.push('\n');
    }
    assert!(text.contains("settings"), "popup row 'settings' missing from viewport:\n{text}");
    assert!(text.contains("Open settings menu"), "description column missing:\n{text}");
}
