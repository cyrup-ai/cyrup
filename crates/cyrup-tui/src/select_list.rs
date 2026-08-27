//! The `SelectList` popup widget (spec/tui/04 §4; spec/tui/05 §3.2; port of
//! `pi-tui/src/components/select-list.ts`).
//!
//! A windowed, two-column list rendered as plain styled rows (NOT a bordered box) — for the
//! autocomplete popup it is appended directly below the editor's bottom rule, and it is the shared
//! engine behind every selector. Selection is internal state mutated by [`SelectList::select_up`] /
//! [`SelectList::select_down`] (both **wrap**, `select-list.ts:115-123`) and read at render
//! (pure `state -> lines`).
//!
//! **Width math is visible-width, never char counts.** Upstream measures with `visibleWidth`
//! (`select-list.ts:147`, `:153`, `:181` → `tui/src/utils.ts:240-295`, which segments into grapheme
//! clusters and sums `graphemeWidth`) and cuts with `truncateToWidth` (`:159`, `:209`, `:211` →
//! `utils.ts:1053`, which walks grapheme clusters and never splits one). cyrup used
//! `chars().count()` and `chars().take(n)` for both — the char-vs-grapheme defect this crate has
//! now carried in eight separate measurements. A CJK label measured half its true column count and
//! a ZWJ family emoji or a combining mark could be cut in half, corrupting the row.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::component::Component;
use crate::selector::centered_window;
use crate::text_width::{str_width, truncate_to_width};
use crate::theme::UiTheme;

/// Default max visible rows (`editor.ts:333-334`), clamped to `[3, 20]`.
pub const DEFAULT_MAX_VISIBLE: u16 = 5;
const PRIMARY_COLUMN_GAP: usize = 2;
const MIN_DESCRIPTION_WIDTH: usize = 10;
const DEFAULT_PRIMARY_COLUMN_MAX: usize = 32;

/// `normalizeToSingleLine` (`select-list.ts:9`):
/// `const normalizeToSingleLine = (text: string): string => text.replace(/[\r\n]+/g, " ").trim();`
///
/// A **run** of `\r`/`\n` collapses to exactly one space (the regex is `+`), then the result is
/// trimmed of surrounding whitespace. Interior tabs and multiple spaces are left alone — upstream
/// does not touch them here.
fn normalize_to_single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_break = false;
    for ch in text.chars() {
        if ch == '\r' || ch == '\n' {
            if !in_break {
                out.push(' ');
                in_break = true;
            }
        } else {
            in_break = false;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

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
    #[must_use]
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
        centered_window(self.selected, self.items.len(), usize::from(self.max_visible))
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
        let primary_w = self.primary_column_width();

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible.len() + 1);
        for (offset, item) in visible.iter().enumerate() {
            let idx = start + offset;
            let is_sel = idx == self.selected;
            // S38: the description is flattened to ONE line before it reaches `renderItem` —
            // `select-list.ts:98`
            //   `const descriptionSingleLine = item.description ? normalizeToSingleLine(item.description) : undefined;`
            // and it is that value, never `item.description`, that `:99` passes on. cyrup handed
            // the raw string through, so a description carrying a `\n` (an extension's `ui.select`
            // option, a wrapped skill blurb) injected a real line break into a row that is measured
            // and truncated as a single line — the row broke and the geometry below went with it.
            // `:149`'s gate is on the NORMALIZED value, and an all-whitespace description
            // normalizes to `""` — falsy in JS, so upstream takes the single-column arm. `filter`
            // reproduces that; `Some("")` would not.
            let desc = item
                .description
                .as_deref()
                .map(normalize_to_single_line)
                .filter(|d| !d.is_empty());
            lines.push(self.row_line(item, desc.as_deref(), is_sel, primary_w, width, theme));
        }
        if end - start < self.items.len() {
            // Scroll indicator `  (sel+1/total)` in muted, truncated to `width - 2` with no
            // ellipsis (`select-list.ts:103-107`). The `- 2` is the same right safety gutter the
            // rows reserve (S25); without it the indicator was the one row that could run past the
            // frame on a narrow terminal.
            let indicator = format!("  ({}/{})", self.selected + 1, self.items.len());
            let indicator = truncate(&indicator, width.saturating_sub(2));
            lines.push(Line::from(Span::styled(indicator, theme.muted_style())));
        }
        lines
    }

    /// The auto-sized primary-column width — `getPrimaryColumnWidth` (`select-list.ts:178-197`).
    ///
    /// S26 (TUI-FIDELITY §3D): two corrections over the previous implementation.
    ///
    /// 1. Upstream reduces over **`this.filteredItems`** (`:180-184`) — the whole list, not the
    ///    currently-visible window. cyrup measured only the `maxVisible` rows it was about to draw,
    ///    so the description column shifted horizontally every time the window scrolled past a
    ///    longer or shorter label. That jitter is on by default for any list longer than 5 rows.
    /// 2. The `PRIMARY_COLUMN_GAP` is **folded into the column width** (`:181`
    ///    `visibleWidth(...) + PRIMARY_COLUMN_GAP`) rather than appended as a separate span after
    ///    padding. It only shows when the clamp binds: at the 32-column cap upstream draws a
    ///    30-char label + a 2-column gap and starts the description at `prefix + 32`, where cyrup
    ///    drew a 32-char label + 2 and started it at `prefix + 34`.
    ///
    /// The bounds mirror `getPrimaryColumnBounds` (`:187-197`): ordered and floored at 1, so the
    /// `clamp` below can never be handed an inverted range.
    ///
    /// 3. The measurement is `visibleWidth` (`:181`), not a char count: a two-column CJK label
    ///    measured one column per char, so the description column landed *inside* the label.
    fn primary_column_width(&self) -> usize {
        let widest = self
            .items
            .iter()
            .map(|i| str_width(&i.label).saturating_add(PRIMARY_COLUMN_GAP))
            .max()
            .unwrap_or(0);
        let lo = self.layout.primary_min.min(self.layout.primary_max).max(1);
        let hi = self.layout.primary_min.max(self.layout.primary_max).max(1);
        widest.clamp(lo, hi)
    }

    /// Render one row — a line-for-line port of `renderItem` (`select-list.ts:139-176`).
    ///
    /// Selected rows are `→ ` + the `selectedText` colour across the whole row (`:160-162`);
    /// unselected rows are two leading spaces + an unstyled label + a `description`-coloured
    /// spacing+description run (`:164-165` — upstream puts the spacing *inside* the description
    /// style).
    ///
    /// S1/SYS-4: the selected row used to be filled with `selectedBg`. Upstream's `SelectList`
    /// never calls `theme.bg(...)` at all (`git grep selectedBg v0.84.1 -- packages/tui` is empty);
    /// `getSelectListTheme` (`theme/theme.ts:1291-1298`) makes `selectedText` a bare
    /// `theme.fg("accent", …)`. The bar is now drawn only where upstream draws it — `/tree` and
    /// `/resume` (see `tree_selector.rs` / `session_selector.rs`).
    ///
    /// S25: both width budgets reserve upstream's 2-column right safety gutter (`:156`, `:169`).
    /// S27: the label is truncated to `effectivePrimaryColumnWidth - PRIMARY_COLUMN_GAP` (`:151`),
    /// which is what guarantees at least the gap's worth of blank columns before the description.
    fn row_line(
        &self,
        item: &SelectItem,
        description: Option<&str>,
        is_sel: bool,
        primary_w: usize,
        width: usize,
        theme: &UiTheme,
    ) -> Line<'static> {
        let prefix = if is_sel { "→ " } else { "  " };
        let prefix_w = 2usize;
        let sel_style = theme.accent_style();

        // Two-column arm — upstream's gate is per-item: `descriptionSingleLine && width > 40`
        // (`:149`).
        if let Some(desc) = description
            && width > 40
        {
            // `effectivePrimaryColumnWidth = max(1, min(primaryColumnWidth, width - prefix - 4))`.
            let effective = primary_w.min(width.saturating_sub(prefix_w).saturating_sub(4)).max(1);
            let max_primary = effective.saturating_sub(PRIMARY_COLUMN_GAP).max(1);
            let label = truncate(&item.label, max_primary);
            let label_w = str_width(&label);
            // `spacing = " ".repeat(max(1, effectivePrimaryColumnWidth - truncatedValueWidth))`.
            let spacing = effective.saturating_sub(label_w).max(1);
            let desc_start = prefix_w.saturating_add(label_w).saturating_add(spacing);
            // `remainingWidth = width - descriptionStart - 2; // -2 for safety`, and the guard is
            // strictly-greater (`:158`).
            let remaining = width.saturating_sub(desc_start).saturating_sub(2);
            if remaining > MIN_DESCRIPTION_WIDTH {
                let desc = truncate(desc, remaining);
                let pad = " ".repeat(spacing);
                return if is_sel {
                    Line::from(vec![
                        Span::styled(prefix.to_string(), sel_style),
                        Span::styled(label, sel_style),
                        Span::styled(format!("{pad}{desc}"), sel_style),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(prefix.to_string(), theme.base_style()),
                        Span::styled(label, theme.base_style()),
                        Span::styled(format!("{pad}{desc}"), theme.muted_style()),
                    ])
                };
            }
            // Otherwise fall through to the single-column arm, exactly as upstream does.
        }

        // Single-column arm (`:169-175`): `maxWidth = width - prefixWidth - 2`.
        let avail = width.saturating_sub(prefix_w).saturating_sub(2);
        let label = truncate(&item.label, avail);
        let style = if is_sel { sel_style } else { theme.base_style() };
        Line::from(vec![Span::styled(prefix.to_string(), style), Span::styled(label, style)])
    }
}

/// Hard-cut a string to `max` **visible columns**, no ellipsis — `truncateToWidth(text, max, "")`
/// (`select-list.ts:159`, `:209`, `:211`; `tui/src/utils.ts:1053-1092`).
///
/// S27: this delegates to the crate's one grapheme-atomic truncator rather than slicing `chars()`.
/// Upstream's non-ASCII branch iterates `graphemeSegmenter.segment(text)` and only keeps a cluster
/// whole (`utils.ts:1100-1110`), so `👨‍👩‍👧` is either present or absent, never reduced to its
/// leading `👨`, and `e` + U+0301 never loses its accent. A char cut did both, and additionally
/// over-filled the column for any wide character.
fn truncate(s: &str, max: usize) -> String {
    truncate_to_width(s, max, "")
}

impl Component for SelectList {
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let lines = self.lines(area.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }
}
