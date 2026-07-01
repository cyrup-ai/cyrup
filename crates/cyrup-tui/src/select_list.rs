//! The `SelectList` popup widget (spec/tui/04 §4; spec/tui/05 §3.2; port of
//! `pi-tui/src/components/select-list.ts`).
//!
//! A windowed, two-column list rendered as plain styled rows (NOT a bordered box) — for the
//! autocomplete popup it is appended directly below the editor's bottom rule, and it is the shared
//! engine behind every selector. Selection is internal state mutated by [`SelectList::select_up`] /
//! [`SelectList::select_down`] (both **wrap**, `select-list.ts:115-123`) and read at render
//! (pure `state -> lines`). Width math is char-based here (CJK/emoji visible-width via `unicode-width`
//! is tracked as a residual — gaps 41/42).

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::component::Component;
use crate::theme::UiTheme;

/// Default max visible rows (`editor.ts:333-334`), clamped to `[3, 20]`.
pub const DEFAULT_MAX_VISIBLE: u16 = 5;
const PRIMARY_COLUMN_GAP: usize = 2;
const MIN_DESCRIPTION_WIDTH: usize = 10;
const DEFAULT_PRIMARY_COLUMN_MAX: usize = 32;

/// One row: a primary `label` and an optional secondary `description`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectItem {
    pub label: String,
    pub description: Option<String>,
}

impl SelectItem {
    pub fn new(label: impl Into<String>, description: Option<String>) -> Self {
        SelectItem { label: label.into(), description }
    }
    /// A label-only row.
    pub fn label(label: impl Into<String>) -> Self {
        SelectItem { label: label.into(), description: None }
    }
}

/// The fixed primary-column width policy. Slash commands pin `[12, 32]`
/// (`SLASH_COMMAND_SELECT_LIST_LAYOUT`, `editor.ts:231-234`); the default auto-sizes to the widest
/// label, clamped to 32 (`getPrimaryColumnWidth`, `:178-197`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnLayout {
    pub primary_min: usize,
    pub primary_max: usize,
}

impl ColumnLayout {
    /// Auto-sizing default (min 0, max 32).
    pub const DEFAULT: ColumnLayout = ColumnLayout { primary_min: 0, primary_max: DEFAULT_PRIMARY_COLUMN_MAX };
    /// Slash-command layout: primary column pinned to `[12, 32]`.
    pub const SLASH: ColumnLayout = ColumnLayout { primary_min: 12, primary_max: DEFAULT_PRIMARY_COLUMN_MAX };
}

/// A windowed select list with a centered selection and a `(i/N)` scroll indicator.
#[derive(Clone, Debug)]
pub struct SelectList {
    items: Vec<SelectItem>,
    selected: usize,
    max_visible: u16,
    layout: ColumnLayout,
    /// Text for the empty state (e.g. `No matching commands`).
    no_match: String,
}

impl SelectList {
    /// A new list over `items` with the given column layout.
    pub fn new(items: Vec<SelectItem>, layout: ColumnLayout) -> Self {
        SelectList { items, selected: 0, max_visible: DEFAULT_MAX_VISIBLE, layout, no_match: "No matches".to_string() }
    }

    /// Set the empty-state text.
    pub fn with_no_match(mut self, text: impl Into<String>) -> Self {
        self.no_match = text.into();
        self
    }

    /// Set the max visible rows, clamped to `[3, 20]` (`editor.ts:333-334`).
    pub fn set_max_visible(&mut self, n: u16) {
        self.max_visible = n.clamp(3, 20);
    }

    /// The current max visible rows (Pi `autocompleteMaxVisible`; test/inspection).
    pub fn max_visible(&self) -> u16 {
        self.max_visible
    }

    /// Replace the items, clamping the selection into range.
    pub fn set_items(&mut self, items: Vec<SelectItem>) {
        self.items = items;
        self.clamp_selection();
    }

    /// The current items.
    pub fn items(&self) -> &[SelectItem] {
        &self.items
    }
    /// The selected index (0 when empty).
    pub fn selected(&self) -> usize {
        self.selected
    }
    /// The selected item, if any.
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.items.get(self.selected)
    }
    /// Number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Force the selection to a specific index (clamped).
    pub fn set_selected(&mut self, index: usize) {
        self.selected = index;
        self.clamp_selection();
    }

    /// Move selection up one row, wrapping bottom↔top (`select-list.ts:115-118`).
    pub fn select_up(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 { self.items.len() - 1 } else { self.selected - 1 };
    }

    /// Move selection down one row, wrapping top↔bottom (`select-list.ts:120-123`).
    pub fn select_down(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = if self.selected + 1 >= self.items.len() { 0 } else { self.selected + 1 };
    }

    fn clamp_selection(&mut self) {
        if self.items.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.items.len() {
            self.selected = self.items.len() - 1;
        }
    }

    /// The visible window `[start, end)` centered on the selection
    /// (`select-list.ts:86-90`): `start = clamp(selected - maxVisible/2, 0, len - maxVisible)`.
    fn window(&self) -> (usize, usize) {
        let len = self.items.len();
        let max = self.max_visible as usize;
        if len <= max {
            return (0, len);
        }
        let half = max / 2;
        let start = self.selected.saturating_sub(half).min(len - max);
        (start, start + max)
    }

    /// The number of rendered rows (visible rows + optional indicator), for live-region height math.
    pub fn rendered_height(&self) -> u16 {
        if self.items.is_empty() {
            return 1; // the no-match row
        }
        let (start, end) = self.window();
        let mut h = end - start;
        if end - start < self.items.len() {
            h += 1; // scroll indicator
        }
        h.min(u16::MAX as usize) as u16
    }

    /// Build the styled rows for the popup (pure `state -> lines`). `width` is the available cell
    /// width; the primary column auto-sizes within the layout bounds and the description fills the
    /// remainder (dropped if under [`MIN_DESCRIPTION_WIDTH`]).
    pub fn lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        if self.items.is_empty() {
            return vec![Line::from(Span::styled(format!("  {}", self.no_match), theme.muted_style()))];
        }
        let width = width as usize;
        let (start, end) = self.window();
        let visible = self.items.get(start..end).unwrap_or(&[]);
        let primary_w = self.primary_column_width(visible);
        let two_col = width > 40 && visible.iter().any(|i| i.description.is_some());

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible.len() + 1);
        for (offset, item) in visible.iter().enumerate() {
            let idx = start + offset;
            let is_sel = idx == self.selected;
            lines.push(self.row_line(item, is_sel, two_col, primary_w, width, theme));
        }
        if end - start < self.items.len() {
            // Scroll indicator `  (sel+1/total)` in muted (`select-list.ts:103-107`).
            let indicator = format!("  ({}/{})", self.selected + 1, self.items.len());
            lines.push(Line::from(Span::styled(indicator, theme.muted_style())));
        }
        lines
    }

    /// The auto-sized primary-column width: widest visible label clamped to the layout bounds.
    fn primary_column_width(&self, visible: &[SelectItem]) -> usize {
        let widest = visible.iter().map(|i| i.label.chars().count()).max().unwrap_or(0);
        widest.clamp(self.layout.primary_min, self.layout.primary_max)
    }

    /// Render one row. Selected rows are `→ ` + accent across the whole row (`select-list.ts:160-162`);
    /// unselected rows are two leading spaces + a normal label + a muted description.
    fn row_line(
        &self,
        item: &SelectItem,
        is_sel: bool,
        two_col: bool,
        primary_w: usize,
        width: usize,
        theme: &UiTheme,
    ) -> Line<'static> {
        let prefix = if is_sel { "→ " } else { "  " };
        let prefix_w = 2usize;
        if !two_col || item.description.is_none() {
            let avail = width.saturating_sub(prefix_w);
            let label = truncate(&item.label, avail);
            let style = if is_sel { theme.selected_bg_style() } else { theme.base_style() };
            return Line::from(vec![
                Span::styled(prefix.to_string(), if is_sel { theme.selected_bg_style() } else { theme.base_style() }),
                Span::styled(label, style),
            ]);
        }
        // Two-column: prefix + padded primary + gap + truncated description.
        let label = truncate(&item.label, primary_w);
        let pad = primary_w.saturating_sub(label.chars().count());
        let desc_avail = width
            .saturating_sub(prefix_w)
            .saturating_sub(primary_w)
            .saturating_sub(PRIMARY_COLUMN_GAP);
        let desc = item.description.as_deref().unwrap_or("");
        if desc_avail < MIN_DESCRIPTION_WIDTH {
            // Drop the description column when it cannot fit (`select-list.ts:169-175`).
            let style = if is_sel { theme.selected_bg_style() } else { theme.base_style() };
            return Line::from(vec![
                Span::styled(prefix.to_string(), style),
                Span::styled(label, style),
            ]);
        }
        let desc = truncate(desc, desc_avail);
        if is_sel {
            // The whole selected row is accent (description not separately muted, `:160-162`).
            Line::from(vec![
                Span::styled(prefix.to_string(), theme.selected_bg_style()),
                Span::styled(format!("{label}{:pad$}", "", pad = pad), theme.selected_bg_style()),
                Span::styled(" ".repeat(PRIMARY_COLUMN_GAP), theme.selected_bg_style()),
                Span::styled(desc, theme.selected_bg_style()),
            ])
        } else {
            Line::from(vec![
                Span::styled(prefix.to_string(), theme.base_style()),
                Span::styled(format!("{label}{:pad$}", "", pad = pad), theme.base_style()),
                Span::styled(" ".repeat(PRIMARY_COLUMN_GAP), theme.base_style()),
                Span::styled(desc, theme.muted_style()),
            ])
        }
    }
}

/// Hard-cut a string to `max` chars, no ellipsis (`truncateToWidth(text, max, "")`,
/// `select-list.ts:159`). Char-based; CJK/emoji visible-width is a tracked residual.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

impl Component for SelectList {
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let lines = self.lines(area.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }
}
