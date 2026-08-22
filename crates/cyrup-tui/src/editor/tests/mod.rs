//! `editor`'s in-tree unit tests. They live inside `crate::editor` so they can reach the
//! module's `pub(super)` helpers (`word_wrap_line`, `style_zones`, `spans_for_segment`,
//! `ghost_span`, `scroll_border`, `display_width`).

mod command_highlight;
mod ghost_span;
mod render;
mod scroll_rule;
mod spans;
mod style_zones;
mod wrap;
