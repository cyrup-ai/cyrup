#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::editor::*;
use crate::editor::render::spans_for_segment;

// ---- spans_for_segment -------------------------------------------------------------------

fn span_texts(spans: &[Span<'static>]) -> Vec<String> {
    spans.iter().map(|s| s.content.to_string()).collect()
}

#[test]
fn spans_for_segment_with_no_cursor_emits_one_span_per_zone() {
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    let cursor_style = base.add_modifier(Modifier::REVERSED);
    let seg: Vec<char> = "/model ".chars().collect();
    let zones = [None, Some((0usize, 6usize, accent)), Some((6usize, 1usize, base))];
    let spans = spans_for_segment(&seg, &zones, None, cursor_style, base);
    assert_eq!(span_texts(&spans), vec!["/model".to_string(), " ".to_string()]);
    assert_eq!(spans[0].style, accent);
    assert_eq!(spans[1].style, base);
}

#[test]
fn spans_for_segment_end_of_line_caret_is_a_reversed_trailing_space() {
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    let cursor_style = base.add_modifier(Modifier::REVERSED);
    let seg: Vec<char> = "/model ".chars().collect();
    let zones = [None, Some((0usize, 6usize, accent)), Some((6usize, 1usize, base))];
    // Cursor at seg.len() (end of line): the caret is the trailing reversed space, appended
    // AFTER the ordinary zone spans.
    let spans = spans_for_segment(&seg, &zones, Some(seg.len()), cursor_style, base);
    let last = spans.last().unwrap();
    assert_eq!(last.content.as_ref(), " ");
    assert_eq!(last.style, cursor_style);
}

#[test]
fn spans_for_segment_cursor_inside_the_accent_zone_splits_it() {
    let base = Style::default();
    let accent = Style::default().fg(ratatui::style::Color::Cyan);
    let cursor_style = base.add_modifier(Modifier::REVERSED);
    let seg: Vec<char> = "/model ".chars().collect();
    let zones = [None, Some((0usize, 6usize, accent)), Some((6usize, 1usize, base))];
    // Cursor at column 2 ("/m|odel "), inside the accent zone.
    let spans = spans_for_segment(&seg, &zones, Some(2), cursor_style, base);
    assert_eq!(span_texts(&spans), vec!["/m", "o", "del", " "]);
    assert_eq!(spans[0].style, accent, "before-cursor part of the accent zone stays accent");
    assert_eq!(spans[1].style, cursor_style, "the cursor cell is reversed");
    assert_eq!(spans[2].style, accent, "after-cursor part of the accent zone stays accent");
    assert_eq!(spans[3].style, base, "the tail zone is unaffected");
}

#[test]
fn spans_for_segment_empty_visual_line_emits_one_empty_base_span() {
    let base = Style::default();
    let cursor_style = base.add_modifier(Modifier::REVERSED);
    let seg: Vec<char> = Vec::new();
    let zones = [Some((0usize, 0usize, base)), None, None];
    let spans = spans_for_segment(&seg, &zones, None, cursor_style, base);
    assert_eq!(span_texts(&spans), vec![String::new()]);
    assert_eq!(spans[0].style, base, "a blank NON-cursor soft-newline row must not carry a caret");
}

#[test]
fn spans_for_segment_empty_visual_line_with_cursor_is_the_reversed_caret() {
    let base = Style::default();
    let cursor_style = base.add_modifier(Modifier::REVERSED);
    let seg: Vec<char> = Vec::new();
    let zones = [Some((0usize, 0usize, base)), None, None];
    let spans = spans_for_segment(&seg, &zones, Some(0), cursor_style, base);
    assert_eq!(span_texts(&spans), vec![" ".to_string()]);
    assert_eq!(spans[0].style, cursor_style);
}
