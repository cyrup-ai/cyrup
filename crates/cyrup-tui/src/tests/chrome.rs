//! Chrome-tail tests (spec/tui/01; Pi `keybinding-hints.ts` / `visual-truncate.ts` /
//! `bordered-loader.ts`) — TestBackend buffer assertions where rendered.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{
    BorderedLoader, Keymap, UiTheme, compact_hints, format_key_text, truncate_to_visual_lines,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

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
    assert_eq!(
        descs,
        vec!["interrupt", "clear/exit", "commands", "bash", "more"]
    );
    // Defaults: Escape interrupt, Ctrl+C clear, Ctrl+D exit, Ctrl+O expand. The interrupt key spells
    // out as `escape`: upstream's id is `"app.interrupt": { defaultKeys: "escape" }` (v0.84.1
    // `coding-agent/src/core/keybindings.ts:66`) and `formatKeyText` (`keybinding-hints.ts:17-27`)
    // only splits on `/`+`+` and rewrites `alt`→`option` — it never abbreviates.
    assert_eq!(hints[0].0, "escape");
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
    let loader = BorderedLoader::cancellable("Working on it", "escape/ctrl+c");
    // 7 rows: `DynamicBorder` + `Loader` (2 — `["", ...super.render(width)]`, v0.84.1
    // `tui/src/components/loader.ts:43-45`) + `Spacer(1)` + `Text(keyHint, 1, 0)` + `Spacer(1)` +
    // `DynamicBorder` (`coding-agent/src/modes/interactive/components/bordered-loader.ts:16-39`).
    assert_eq!(loader.height(), 7);
    let mut terminal = Terminal::new(TestBackend::new(40, 7)).unwrap();
    terminal
        .draw(|f| loader.render(f, Rect::new(0, 0, 40, 7), &theme, 0))
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
    // 5 rows: the cancellable pair (`Spacer(1)` + hint `Text`) is skipped, everything else stands
    // (`bordered-loader.ts:34-39`).
    assert_eq!(loader.height(), 5);
    let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
    terminal
        .draw(|f| loader.render(f, Rect::new(0, 0, 40, 5), &theme, 1))
        .unwrap();
    let text = buf_string(&terminal);
    assert!(text.contains("Loading"));
    assert!(!text.contains("cancel"));
}

/// `truncateToVisualLines` owns no wrapping of its own upstream — it is literally
/// `new Text(text, paddingX, 0).render(width)` (`visual-truncate.ts:37-38`), i.e.
/// `wrapTextWithAnsi(text, width - paddingX * 2)` (`text.ts:64`, `:67`). So the rows it returns are
/// WORD-wrapped and measured in terminal COLUMNS.
///
/// cyrup sliced each logical line into fixed `width`-*char* chunks instead: it broke mid-word
/// (`… output tha` / `t certainly …`, visible on every long `!command` output row), counted a CJK
/// ideograph as one column when it occupies two, and could split a ZWJ sequence or detach a
/// combining mark. Because the `... N more lines` count is the row count, it was wrong too.
#[test]
fn truncate_word_wraps_and_measures_in_columns_not_chars() {
    let text = "a very long line of program output that certainly does not fit";
    let r = truncate_to_visual_lines(text, 20, 38);
    assert_eq!(r.skipped, 0);
    // Word boundaries only — no row may start or end mid-word.
    for row in &r.lines {
        assert!(row.len() <= 38, "row overflows: {row:?}");
        assert!(
            !row.starts_with(' ') && !row.ends_with(' '),
            "untrimmed row: {row:?}"
        );
    }
    let rejoined = r.lines.join(" ");
    assert_eq!(rejoined, text, "words were split: {:?}", r.lines);

    // A double-width script is measured in COLUMNS: four ideographs are eight columns, so a width of
    // 4 fits exactly two per row — a `chars()` chunker would put four on a row twice as wide as the
    // pane.
    let cjk = truncate_to_visual_lines("日本語だ", 20, 4);
    assert_eq!(cjk.lines, vec!["日本".to_string(), "語だ".to_string()]);

    // A ZWJ family is one cluster and is never split across rows.
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    let zwj = truncate_to_visual_lines(&format!("{family}{family}"), 20, 4);
    for row in &zwj.lines {
        assert!(
            !row.starts_with('\u{200d}') && !row.ends_with('\u{200d}'),
            "ZWJ sequence split: {:?}",
            zwj.lines
        );
    }

    // MIRROR: the tail-truncate and the hidden count still work off the WRAPPED row count.
    let many = truncate_to_visual_lines(text, 2, 20);
    assert_eq!(many.lines.len(), 2);
    assert!(many.skipped > 0, "nothing reported hidden: {many:?}");
}
