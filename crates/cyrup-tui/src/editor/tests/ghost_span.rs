#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::editor::*;
use crate::editor::render::ghost_span;

// ---- ghost_span ---------------------------------------------------------------------------

#[test]
fn ghost_span_renders_whole_when_it_fits() {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let span = ghost_span("<provider/model>", 40, dim).unwrap();
    assert_eq!(span.content.as_ref(), "<provider/model>");
    assert_eq!(span.style, dim);
}

#[test]
fn ghost_span_none_when_no_columns_are_available() {
    let dim = Style::default();
    assert!(ghost_span("<hint>", 0, dim).is_none());
}

#[test]
fn ghost_span_single_column_is_just_an_ellipsis() {
    let dim = Style::default();
    let span = ghost_span("<hint>", 1, dim).unwrap();
    assert_eq!(span.content.as_ref(), "…");
}

#[test]
fn ghost_span_clips_with_a_trailing_ellipsis_when_it_overflows() {
    let dim = Style::default();
    let span = ghost_span("todo_file | number_of_agents | additional_instructions", 10, dim)
        .unwrap();
    assert_eq!(span.content.as_ref(), "todo_file…");
    assert_eq!(display_width(&span.content), 10, "never overruns its budget");
}

#[test]
fn ghost_span_exact_fit_needs_no_ellipsis() {
    let dim = Style::default();
    let span = ghost_span("abc", 3, dim).unwrap();
    assert_eq!(span.content.as_ref(), "abc");
}
