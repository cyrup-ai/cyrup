//! Chrome-tail tests (spec/tui/01; Pi `keybinding-hints.ts` / `visual-truncate.ts` /
//! `bordered-loader.ts`) — TestBackend buffer assertions where rendered.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{
    compact_hints, format_key_text, truncate_to_visual_lines, BorderedLoader, Keymap, UiTheme,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

fn buf_string(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn format_key_text_splits_chords_and_alternatives() {
    // `/` separates alternatives, `+` separates chord parts (formatKeyText, keybinding-hints.ts).
    assert_eq!(format_key_text("ctrl+c/ctrl+d", false), {
        // On macOS `alt`→`option`; neither part here is alt, so identity.
        "ctrl+c/ctrl+d".to_string()
    });
    // Capitalize title-cases each part.
    assert_eq!(format_key_text("ctrl+o", true), "Ctrl+O");
}

#[test]
fn format_key_text_rewrites_alt_to_option_on_macos() {
    let got = format_key_text("alt+d", false);
    if cfg!(target_os = "macos") {
        assert_eq!(got, "option+d");
    } else {
        assert_eq!(got, "alt+d");
    }
}

#[test]
fn compact_hints_source_keys_from_the_live_keymap() {
    let km = Keymap::default();
    let hints = compact_hints(&km);
    // Pi order: interrupt, clear/exit, /, !, more.
    let descs: Vec<&str> = hints.iter().map(|(_, d)| d.as_str()).collect();
    assert_eq!(descs, vec!["interrupt", "clear/exit", "commands", "bash", "more"]);
    // Defaults: Esc interrupt, Ctrl+C clear, Ctrl+D exit, Ctrl+O expand.
    assert_eq!(hints[0].0, "esc");
    assert_eq!(hints[1].0, "ctrl+c/ctrl+d");
    assert_eq!(hints[2].0, "/");
    assert_eq!(hints[4].0, "ctrl+o");
}

#[test]
fn truncate_keeps_last_n_visual_lines_and_reports_skipped() {
    let text = "a\nb\nc\nd\ne";
    let r = truncate_to_visual_lines(text, 2, 80);
    assert_eq!(r.lines, vec!["d".to_string(), "e".to_string()]);
    assert_eq!(r.skipped, 3);
}

#[test]
fn truncate_accounts_for_wrapping() {
    // A 30-char line wraps to three visual lines at width 10; with max 2 the first is skipped.
    let text = "x".repeat(30);
    let r = truncate_to_visual_lines(&text, 2, 10);
    assert_eq!(r.lines.len(), 2);
    assert_eq!(r.skipped, 1);
    assert!(r.lines.iter().all(|l| l.chars().count() == 10));
}

#[test]
fn truncate_no_op_when_under_limit() {
    let r = truncate_to_visual_lines("one\ntwo", 5, 80);
    assert_eq!(r.skipped, 0);
    assert_eq!(r.lines, vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn bordered_loader_renders_message_cancel_hint_and_rules() {
    let theme = UiTheme::dark();
    let loader = BorderedLoader::cancellable("Working on it", "esc");
    assert_eq!(loader.height(), 4);
    let mut terminal = Terminal::new(TestBackend::new(40, 4)).unwrap();
    terminal
        .draw(|f| loader.render(f, Rect::new(0, 0, 40, 4), &theme, 0))
        .unwrap();
    let text = buf_string(&terminal);
    assert!(text.contains("Working on it"), "message: {text}");
    assert!(text.contains("cancel"), "cancel hint: {text}");
    assert!(text.contains('─'), "border rule: {text}");
    assert!(text.contains('⠋'), "spinner frame 0: {text}");
}

#[test]
fn plain_loader_has_no_cancel_row() {
    let theme = UiTheme::dark();
    let loader = BorderedLoader::plain("Loading");
    assert_eq!(loader.height(), 3);
    let mut terminal = Terminal::new(TestBackend::new(40, 3)).unwrap();
    terminal
        .draw(|f| loader.render(f, Rect::new(0, 0, 40, 3), &theme, 1))
        .unwrap();
    let text = buf_string(&terminal);
    assert!(text.contains("Loading"));
    assert!(!text.contains("cancel"));
}
