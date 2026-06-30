//! Footer third line: extension statuses (footer.ts:232-241; gap 9).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{Component, StatusLine, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

fn render_footer(status: &mut StatusLine, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let theme = UiTheme::dark();
    term.draw(|f| status.render(f, Rect::new(0, 0, w, h), &theme)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn statuses_render_sorted_sanitized_on_the_third_line() {
    let mut status = StatusLine::new("anthropic/opus");
    status.set_cwd("~/proj".to_string());
    // Out-of-order keys + a control char that must collapse to a single space.
    status.set_extension_status("zeta", "z-status");
    status.set_extension_status("alpha", "a\n\tline");
    assert!(status.has_extension_statuses());
    // BTreeMap key order → alpha before zeta; the newline/tab sanitize to a single space.
    assert_eq!(status.extension_status_text(), "a line z-status");

    let text = render_footer(&mut status, 40, 3);
    let last = text.lines().nth(2).unwrap_or("");
    assert!(last.contains("a line"), "third line shows alpha status: {text:?}");
    assert!(last.contains("z-status"), "third line shows zeta status: {text:?}");
}

#[test]
fn empty_value_clears_a_status_and_drops_the_third_line() {
    let mut status = StatusLine::new("m");
    status.set_extension_status("ext", "busy");
    assert!(status.has_extension_statuses());
    status.set_extension_status("ext", "   ");
    assert!(!status.has_extension_statuses(), "blank value removes the entry");
    // With no statuses, a 2-row footer renders only location + usage.
    let text = render_footer(&mut status, 40, 2);
    assert_eq!(text.lines().count(), 2);
}
