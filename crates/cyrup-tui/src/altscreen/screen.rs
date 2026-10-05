//! The text of the cells the last frame painted — the second row source of the text selection.
//!
//! Pi's selection has two kinds of anchor. A press inside a scroll view anchors on a row of that
//! view's retained content (`scrollContentLines`); a press anywhere else anchors on a row of the
//! **screen** — `previousScreen`, the composed frame as it was last written to the terminal
//! (`tui-alt-screen.ts:1160-1166`, `:1149-1158`, `:1431-1453`). That second kind is what lets a
//! drag over the editor, a selector, the footer, the working band, extension widgets or the queued
//! messages select and copy what is painted there, and it is the only kind that can: those
//! components are not rows of the scrolled document, so there is no retained text to select from.
//!
//! ratatui keeps no composed screen of its own, only a cell [`Buffer`] that the backend diffs and
//! forgets, so the renderer takes the equivalent of `previousScreen` itself: [`ScreenRows::capture`]
//! reads the buffer at the end of [`super::AltScreen::draw_with`] — after the dock, the overlays
//! and the flash have been painted, which is upstream's order (`:1688-1690`, `previousScreen =
//! screen` at `:1750`) — and keeps one string per row. The selection then reads that snapshot
//! exactly as it reads document rows, and the highlight paints back over the same cells.
//!
//! # What a row is
//! The symbols of the row's cells, left to right, with the cell a wide glyph spans after its first
//! skipped, so a column in the string is a column on the screen: the text of a row is addressed by
//! the same display columns a pointer report is. Trailing blanks are dropped. Pi's rows are
//! strings whose width is the width of their content, a copy trims trailing space off every row
//! (`:1448`), and a highlight stops where the text does (`getSelectionColumns` clamps to the line
//! width, `:1425`); a dense cell buffer would otherwise make every row full width.
//!
//! # Not invalidated by a repaint
//! The snapshot is replaced on every frame and the selection holds screen coordinates, not text.
//! If the dock repaints different text under a live selection, the highlight stays on the same
//! cells and a copy reads whatever is painted in them now — pi's behaviour, where the selection is
//! `{row, col}` and `getActiveSelectionText` reads the newest `previousScreen`.

use ratatui::buffer::Buffer;

use crate::text_width::str_width;

/// One string per screen row, as painted by the last frame. `Default` is no frame yet: no rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ScreenRows {
    rows: Vec<String>,
    /// The frame's width in cells. A pointer column is clamped to it (`:1153`), not to a row's own
    /// text width, so a press right of a short row is still on the screen.
    width: usize,
}

impl ScreenRows {
    /// Read the rows of `buffer` — the frame just painted.
    pub(super) fn capture(buffer: &Buffer) -> Self {
        let area = buffer.area;
        let rows = (area.top()..area.bottom())
            .map(|y| {
                let mut text = String::new();
                let mut x = area.left();
                while x < area.right() {
                    let symbol = buffer.cell((x, y)).map_or(" ", |cell| cell.symbol());
                    // An empty symbol is a cell a wide glyph to its left covers; it has no text of
                    // its own, and the glyph's width is stepped over below.
                    let step = str_width(symbol).max(1);
                    text.push_str(if symbol.is_empty() { " " } else { symbol });
                    x = x.saturating_add(u16::try_from(step).unwrap_or(1));
                }
                text.truncate(text.trim_end().len());
                text
            })
            .collect();
        Self {
            rows,
            width: usize::from(area.width),
        }
    }

    /// The text of screen row `row`, empty past the last row (pi's `previousScreen[row] ?? ""`).
    pub(super) fn text(&self, row: usize) -> &str {
        self.rows.get(row).map_or("", String::as_str)
    }

    /// How many rows the frame had.
    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    /// The frame's width in cells.
    pub(super) fn width(&self) -> usize {
        self.width
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    use super::*;

    fn buffer(width: u16, rows: &[&str]) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, rows.len() as u16));
        for (y, row) in rows.iter().enumerate() {
            buf.set_string(0, y as u16, row, Style::default());
        }
        buf
    }

    /// A row is its cells' text with trailing blanks dropped, and the frame's width is kept apart
    /// from any row's own.
    #[test]
    fn a_row_is_its_text_without_trailing_blanks() {
        let rows = ScreenRows::capture(&buffer(12, &["  indented  ", "", "x"]));
        assert_eq!(rows.text(0), "  indented");
        assert_eq!(rows.text(1), "");
        assert_eq!(rows.text(2), "x");
        assert_eq!(rows.text(9), "", "past the last row is empty");
        assert_eq!((rows.len(), rows.width()), (3, 12));
    }

    /// The cell a wide glyph covers holds no text, so the glyph is not followed by a phantom blank
    /// and every later character sits at its screen column.
    #[test]
    fn a_wide_glyph_occupies_its_columns_without_a_phantom_blank() {
        let rows = ScreenRows::capture(&buffer(8, &["a世界b"]));
        assert_eq!(rows.text(0), "a世界b");
        assert_eq!(str_width(rows.text(0)), 6, "a, two wide glyphs, b");
    }

    /// Before the first frame there is nothing to anchor on.
    #[test]
    fn no_frame_has_no_rows() {
        let rows = ScreenRows::default();
        assert_eq!((rows.len(), rows.width()), (0, 0));
    }
}
