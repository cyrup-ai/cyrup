#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::editor::*;
use crate::editor::render::style_zones;

// ---- style_zones ------------------------------------------------------------------------

fn vl(logical: usize, start: usize, len: usize) -> VisualLine {
    VisualLine { logical, start, len }
}

#[test]
fn style_zones_plain_when_there_is_no_token() {
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    let zones = style_zones(&vl(0, 0, 7), 7, None, base, accent);
    assert_eq!(zones, [Some((0, 7, base)), None, None]);
}

#[test]
fn style_zones_plain_off_logical_line_zero() {
    // Only logical line 0 ever carries a command token, so a token on another logical line's
    // visual-line window is ignored even if the range would otherwise overlap.
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    let zones = style_zones(&vl(1, 0, 6), 6, Some(&(0..6)), base, accent);
    assert_eq!(zones, [Some((0, 6, base)), None, None]);
}

#[test]
fn style_zones_splits_a_token_fully_inside_one_window() {
    // "/model " — token 0..6 inside a 7-char window.
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    let zones = style_zones(&vl(0, 0, 7), 7, Some(&(0..6)), base, accent);
    // Head zone is always absent (`a == 0` under the invariant `token.start == 0`).
    assert_eq!(zones, [None, Some((0, 6, accent)), Some((6, 1, base))]);
}

#[test]
fn style_zones_covers_the_whole_window_when_the_token_extends_past_it() {
    // A window that is entirely inside a longer token (the wrapped-token case): the whole
    // segment is accent, with no tail zone.
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    // Token 0..20 (a long wrapped command name), window is visual line 1: chars 8..14 of the
    // logical line.
    let zones = style_zones(&vl(0, 8, 6), 6, Some(&(0..20)), base, accent);
    assert_eq!(zones, [None, Some((0, 6, accent)), None]);
}

#[test]
fn style_zones_a_token_spanning_two_visual_lines_covers_both_windows_fully() {
    // A 14-char token wrapped into two 7-char visual lines: both windows must come back fully
    // accent, proving the highlight survives the wrap.
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    let token = 0..14;
    let first = style_zones(&vl(0, 0, 7), 7, Some(&token), base, accent);
    let second = style_zones(&vl(0, 7, 7), 7, Some(&token), base, accent);
    assert_eq!(first, [None, Some((0, 7, accent)), None]);
    assert_eq!(second, [None, Some((0, 7, accent)), None]);
}

#[test]
fn style_zones_plain_when_the_window_is_entirely_past_the_token() {
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    // Token 0..6, window starts at 6 (right after the token) — no overlap.
    let zones = style_zones(&vl(0, 6, 4), 4, Some(&(0..6)), base, accent);
    assert_eq!(zones, [Some((0, 4, base)), None, None]);
}
