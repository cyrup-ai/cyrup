//! The pointer over the editor: a click places the caret, a click on a completion row accepts it.
//!
//! Pi's `Editor.handleMouse` (`components/editor.ts:632-686` @v1.0.0) answers only a left *click*:
//! the press, drag and release that make one are left to the renderer's text selection, which is
//! why this module never claims them. A click maps the pointer's row to a visual line of the
//! wrapped buffer and its column to a grapheme cluster on that line.

use ratatui::layout::Rect;

use super::{InputEditor, LastAction, VisualLine, display_width};
use crate::app::Pointer;
use crate::select_list::ListPointer;

/// Where on a visual line a click at display column `target` puts the caret, as a char offset into
/// the LOGICAL line.
///
/// `clusters` are the char offsets at which the visual line's grapheme clusters start, in order,
/// each paired with its display width. The first cluster whose far edge lies past `target` owns
/// the click — so the right half of a wide glyph lands on its start, pi's `targetColumn <
/// nextColumn`. A click past the last cluster lands at `end`, the end of the visual line — unless
/// another visual line of the same logical line follows (`mid_line`): that offset is the wrap
/// boundary and belongs to the next row, so the caret takes the last cluster's start instead (pi's
/// `lastGraphemeIndex` rule).
fn caret_for_column(
    clusters: &[(usize, usize)],
    end: usize,
    mid_line: bool,
    target: usize,
) -> usize {
    let mut edge = 0usize;
    for &(start, width) in clusters {
        edge = edge.saturating_add(width);
        if target < edge {
            return start;
        }
    }
    match clusters.last() {
        Some(&(last, _)) if mid_line => last,
        _ => end,
    }
}

impl InputEditor {
    /// Act on a pointer event over the editor, `event` local to `area` (the rectangle the editor
    /// was rendered into). `true` when the editor changed and the frame must be repainted.
    ///
    /// Only a click places the caret. A click on a rule row, or below the last visible line, is the
    /// editor's (the caller claims it) but moves nothing.
    pub(crate) fn pointer(&mut self, area: Rect, event: Pointer) -> bool {
        let Pointer::Click { at, .. } = event else {
            return false;
        };
        let map = self.visual_line_map();
        // Row 0 is the top rule; the visible lines follow, at most as many as the render window
        // shows — the same budget `render` takes.
        let window = usize::from(area.height.saturating_sub(2))
            .min(usize::from(self.max_visible_lines()))
            .max(1);
        let shown = map.len().saturating_sub(self.scroll_offset).min(window);
        let row = usize::from(at.y);
        if row == 0 || row > shown {
            return false;
        }
        let index = self.scroll_offset.saturating_add(row - 1);
        let (Some(&vl), next) = (map.get(index), map.get(index + 1)) else {
            return false;
        };
        let mid_line = next.is_some_and(|n| n.logical == vl.logical);
        let target = usize::from(at.x.saturating_sub(self.effective_padding(area.width)));
        let col = self.column_at(vl, mid_line, target);

        let before = (self.row, self.col);
        self.row = vl.logical;
        self.col = col;
        // `setCursorCol` forgets the sticky column; a click is not a vertical run.
        self.reset_preferred_col();
        self.last_action = LastAction::None;
        self.exit_history();
        let popup = self.autocomplete.is_some();
        if popup {
            self.update_autocomplete();
        }
        popup || before != (self.row, self.col)
    }

    /// The caret column a click at display column `target` of visual line `vl` gives.
    fn column_at(&self, vl: VisualLine, mid_line: bool, target: usize) -> usize {
        let Some(line) = self.lines.get(vl.logical) else {
            return 0;
        };
        let end = vl.start.saturating_add(vl.len).min(line.len());
        let bounds = self.marker_grapheme_boundaries(line);
        let mut clusters = Vec::new();
        for pair in bounds.windows(2) {
            let &[start, stop] = pair else {
                continue;
            };
            if start < vl.start || start >= end {
                continue;
            }
            let text: String = line
                .iter()
                .skip(start)
                .take(stop.min(end).saturating_sub(start))
                .collect();
            clusters.push((start, display_width(&text)));
        }
        caret_for_column(&clusters, end, mid_line, target)
    }

    /// Act on a pointer event over the completion popup, `event` local to `area` (the popup's
    /// rectangle). `true` when the popup or the buffer changed.
    ///
    /// The list answers like a key would: a wheel notch or a press moves the highlight, a click
    /// activates an item. Activation is pi's `SelectList.onSelect` for the editor's popup
    /// (`components/editor.ts:2241-2255`): snapshot for undo, apply the completion, close the
    /// popup. It does NOT submit — the Enter key's slash-command fall-through to submit lives in
    /// the key handler, not in `onSelect` — and it does not reopen the popup, which is why a
    /// clicked directory is not drilled into the way Tab drills.
    ///
    /// The list is painted inset by the editor's horizontal padding, but a row is chosen by its
    /// vertical position alone: pi shifts `x` by the padding and the list never reads it, so a
    /// click in the padding columns still hits the row it is level with.
    pub(crate) fn pointer_popup(&mut self, _area: Rect, event: Pointer) -> bool {
        let Some(ac) = self.autocomplete.as_mut() else {
            return false;
        };
        match ac.list.pointer(event) {
            ListPointer::Ignored | ListPointer::Handled => false,
            ListPointer::Moved => true,
            ListPointer::Activated(_) => {
                self.push_undo_for(LastAction::None);
                self.last_action = LastAction::None;
                self.accept_completion();
                self.autocomplete = None;
                true
            }
        }
    }
}
