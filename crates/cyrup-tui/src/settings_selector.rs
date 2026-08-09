//! The settings/config + trust editor-swap selectors (spec/tui/05 §6; port of Pi's
//! `settings-selector.ts` (810), `config-selector.ts` (628), and `trust-selector.ts` (134)).
//!
//! Pi's settings (`/settings`) and config (`/config`) pickers are a [`SettingsList`](settings-list.ts):
//! a windowed list of `SettingItem`s, each rendering a **label** on the left and the **current value**
//! on the right, where `Enter`/`Space` **cycles** the value through a fixed set and applies it *live*
//! (`onChange`) while the slot stays open. The `/trust` picker (`trust-selector.ts`) is a small list
//! with a multi-line header (cwd + saved decision + current-session trust) whose `Enter` **confirms**
//! the chosen option (writing the trust store) and closes. Both occupy the input slot exactly like the
//! [`ListSelector`](crate::selector::ListSelector), delimited by the full-width `DynamicBorder`.

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{SelectAction, SelectKeymap};
use crate::select_list::{ColumnLayout, SelectItem, SelectList};
use crate::selector::{stack_rows, Selector, SelectorOutcome};
use crate::theme::UiTheme;

/// The field separator inside a [`SelectorOutcome::Apply`] / [`SelectorOutcome::Confirm`] payload
/// (`"id\u{1f}value"`), an ASCII Unit Separator so it never collides with a setting id or value.
pub const FIELD_SEP: char = '\u{1f}';

/// One editable row in a [`SettingsSelector`] (Pi `SettingItem`, settings-list.ts:7-20). `id` is the
/// dotted settings key persisted on change; `cycle` are the values `Enter`/`Space` rotates through
/// (empty ⇒ read-only display, e.g. an informational `/config` row).
#[derive(Clone, Debug)]
pub struct SettingRow {
    /// The dotted settings key (`terminal.showImages`) persisted on change.
    pub id: String,
    /// The left-column display label.
    pub label: String,
    /// The current value (right column).
    pub value: String,
    /// The cycle set; `Enter`/`Space` advances to the next. Empty ⇒ read-only.
    pub cycle: Vec<String>,
    /// Optional secondary description (unused in the compact grid; kept for parity).
    pub description: Option<String>,
    /// When `Some(id)`, activating the row opens a nested picker instead of cycling in place (Pi's
    /// `SettingItem.submenu`, `settings-selector.ts:603-610`). The chrome maps `id` to the picker
    /// (e.g. `"theme"` → the theme selector). A submenu row never cycles and never persists a value.
    pub submenu: Option<String>,
}

impl SettingRow {
    /// A boolean toggle row (`true`/`false` cycle).
    pub fn toggle(id: impl Into<String>, label: impl Into<String>, value: bool) -> Self {
        SettingRow {
            id: id.into(),
            label: label.into(),
            value: value.to_string(),
            cycle: vec!["true".to_string(), "false".to_string()],
            description: None,
            submenu: None,
        }
    }

    /// A fixed-choice row cycling through `choices`, preselecting `value`.
    pub fn choice(
        id: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        choices: Vec<String>,
    ) -> Self {
        SettingRow {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            cycle: choices,
            description: None,
            submenu: None,
        }
    }

    /// A read-only informational row (no cycle).
    pub fn info(id: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> Self {
        SettingRow {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            cycle: Vec::new(),
            description: None,
            submenu: None,
        }
    }

    /// A row that opens a nested picker on activation instead of cycling (Pi `SettingItem.submenu`,
    /// `settings-selector.ts:603-610`; the "Theme" row). `value` is the current display value (e.g. the
    /// active theme name); `submenu_id` is the picker key the chrome maps (`"theme"`).
    pub fn submenu(
        id: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        submenu_id: impl Into<String>,
    ) -> Self {
        SettingRow {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            cycle: Vec::new(),
            description: None,
            submenu: Some(submenu_id.into()),
        }
    }
}

/// The settings/config grid selector (`settings-selector.ts` / `config-selector.ts`). Reuses the
/// shared [`SelectList`] body for windowing + label/value column alignment (the value rides the
/// right column), with a bold title between the top/bottom `DynamicBorder`. `Enter`/`Confirm` cycles
/// the highlighted row's value in place and emits [`SelectorOutcome::Apply`] (the slot stays open);
/// `Cancel` closes.
pub struct SettingsSelector {
    title: String,
    rows: Vec<SettingRow>,
    list: SelectList,
}

impl SettingsSelector {
    /// Build from `title` + `rows`, preselecting the first row.
    pub fn new(title: impl Into<String>, rows: Vec<SettingRow>) -> Self {
        let mut sel = SettingsSelector { title: title.into(), rows, list: empty_list() };
        sel.refresh();
        sel.list.set_max_visible(10);
        sel
    }

    /// Rebuild the rendered list from `rows`, preserving the highlight. The value rides the
    /// description (right) column so the shared `ColumnLayout::SLASH` aligns label↔value.
    fn refresh(&mut self) {
        let selected = self.list.selected();
        let items: Vec<SelectItem> = self
            .rows
            .iter()
            .map(|r| SelectItem::new(r.label.clone(), Some(r.value.clone())))
            .collect();
        let mut list = SelectList::new(items, ColumnLayout::SLASH).with_no_match("No settings available");
        list.set_max_visible(10);
        list.set_selected(selected.min(self.rows.len().saturating_sub(1)));
        self.list = list;
    }

    /// Read-only access to the inner list (tests / chrome inspection).
    pub fn list(&self) -> &SelectList {
        &self.list
    }

    /// The highlighted row (tests / inspection).
    pub fn current(&self) -> Option<&SettingRow> {
        self.rows.get(self.list.selected())
    }

    /// Cycle the highlighted row's value to the next in its `cycle` set, returning the
    /// `"id\u{1f}value"` payload to persist (or `None` for a read-only row).
    fn cycle_current(&mut self) -> Option<String> {
        let idx = self.list.selected();
        let row = self.rows.get_mut(idx)?;
        if row.cycle.is_empty() {
            return None;
        }
        let pos = row.cycle.iter().position(|v| *v == row.value).unwrap_or(0);
        let next = (pos + 1) % row.cycle.len();
        row.value = row.cycle.get(next).cloned().unwrap_or_default();
        let payload = format!("{}{}{}", row.id, FIELD_SEP, row.value);
        self.refresh();
        Some(payload)
    }
}

/// The shared empty list builder (avoids repeating the `ColumnLayout` + max-visible boilerplate).
fn empty_list() -> SelectList {
    let mut list = SelectList::new(Vec::new(), ColumnLayout::SLASH);
    list.set_max_visible(10);
    list
}

impl Selector for SettingsSelector {
    fn desired_height(&self, _width: u16) -> u16 {
        // Top rule + title + body + footer hint + bottom rule.
        self.list.rendered_height().saturating_add(4)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // L4/SYS-3 — **no envelope spacers here, and that is upstream-correct.** The audit lists
        // this site among the six that need `Spacer(1)` rows; the source disagrees.
        // `SettingsSelectorComponent`'s constructor adds exactly three children —
        // `new DynamicBorder()` (`settings-selector.ts:765`), the `SettingsList` (`:873`) and
        // `new DynamicBorder()` (`:874`) — with no `Spacer` anywhere, and `SettingsList` emits none
        // of its own around the list either: `renderMainList`
        // (`packages/tui/src/components/settings-list.ts:90-166`) pushes only the search box + one
        // `""`, the rows, an optional scroll indicator, an optional description block and the hint.
        // Adding blank rows here would be a NEW divergence, not a fix.
        //
        // The row STACK is still wrong until it is carved explicitly, though, and this was the last
        // `Layout::vertical` envelope in the crate. Ratatui's solver treats these five constraints
        // as an error-minimisation problem, not a priority order, so on a one-row slot it satisfied
        // `Length(1)` #4 — the HINT — and gave the dialog's own content, its rule and its rows
        // zero: a `/settings` dialog whose single visible row read
        // "↑/↓ navigate · enter cycle · esc close" and nothing else. `stack_rows` is strictly
        // top-first, which is what pi's layout engine does to an over-tall `Container`
        // (`packages/tui/src/layout.ts:113,307-310` — see `stack_rows`' doc), so the surviving row
        // is now the top `DynamicBorder` (`settings-selector.ts:765`), upstream's own first child.
        // The body takes its NATURAL height from the list rather than the leftover rows, so the
        // rendered rows at any height are a prefix of the rows at the natural height.
        let body_h = self.list.rendered_height();
        let [top, title_area, body, hint, bottom] = stack_rows(area, [1, 1, body_h, 1, 1]);
        frame.render_widget(border_rule(top.width, theme), top);
        let title = Span::styled(
            format!(" {}", self.title),
            theme.accent_style().add_modifier(Modifier::BOLD),
        );
        frame.render_widget(Paragraph::new(Line::from(title)), title_area);
        let lines = self.list.lines(body.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), body);
        let hint_text = " ↑/↓ navigate · enter cycle · esc close";
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(hint_text, theme.dim_style()))),
            hint,
        );
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.list.select_up();
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                self.list.select_down();
                SelectorOutcome::Redraw
            }
            // Enter on a submenu row opens the nested picker (Pi `SettingItem.submenu`); otherwise it
            // cycles the value in place and applies it live (Pi `cycleValue` → `onChange`), the slot
            // staying open.
            Some(SelectAction::Confirm) => {
                if let Some(id) = self.current().and_then(|r| r.submenu.clone()) {
                    return SelectorOutcome::OpenSubmenu(id);
                }
                match self.cycle_current() {
                    Some(payload) => SelectorOutcome::Apply(payload),
                    None => SelectorOutcome::Redraw,
                }
            }
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => SelectorOutcome::Ignored,
        }
    }
}

/// The project-trust selector (`trust-selector.ts`): a small option list (`Trust` / `Trust parent` /
/// `Do not trust`) under a multi-line header showing the cwd, the saved decision, and the
/// current-session trust. `Enter` **confirms** the highlighted option (the chrome writes the trust
/// store and closes); `Cancel` closes without writing.
pub struct TrustSelector {
    cwd: String,
    saved_decision: String,
    session_trusted: bool,
    labels: Vec<String>,
    selected: usize,
    /// The index of the option matching the persisted decision (Pi `isSavedOption`,
    /// `trust-selector.ts:92-98`), or `None` when nothing is saved — drives the ` ✓` marker
    /// (S20). Distinct from `selected`, which upstream *derives* from it but falls back to 0 when
    /// there is no match (`Math.max(0, findIndex(...))`, `:45-48`), so a preselected row is NOT
    /// evidence of a saved decision.
    saved_index: Option<usize>,
}

impl TrustSelector {
    /// Build from the option `labels` (Pi `getProjectTrustOptions` labels), the `cwd`, the formatted
    /// `saved_decision` line, the current-session trust flag, and the preselected option index.
    pub fn new(
        cwd: impl Into<String>,
        saved_decision: impl Into<String>,
        session_trusted: bool,
        labels: Vec<String>,
        selected: usize,
    ) -> Self {
        let selected = selected.min(labels.len().saturating_sub(1));
        TrustSelector {
            cwd: cwd.into(),
            saved_decision: saved_decision.into(),
            session_trusted,
            labels,
            selected,
            saved_index: None,
        }
    }

    /// Record which option the persisted decision corresponds to, so it carries the ` ✓`
    /// saved-decision marker (S20 — Pi `isSavedOption` → `checkmark`, `trust-selector.ts:109-110`).
    /// `None` (the default) means no saved decision and draws no marker anywhere.
    ///
    /// Additive rather than a `new` parameter: both callers already compute this predicate to
    /// derive the preselected index (`app.rs` `/trust`, `cyrup/src/startup_ui.rs`
    /// `trust_selected_index`), but they collapse "no match" onto index 0, which cannot be
    /// distinguished afterwards.
    pub fn with_saved_index(mut self, saved: Option<usize>) -> Self {
        self.saved_index = saved.filter(|i| *i < self.labels.len());
        self
    }

    /// The highlighted option index (tests / chrome maps it back to the trust option).
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The header lines (title + cwd + saved decision + current session), test-visible.
    ///
    /// Includes the `Spacer(1)` upstream puts between the cwd row and the `Saved decision:` row
    /// (`trust-selector.ts:56`) — the header's own blank, distinct from the envelope's. S20:
    /// "cwd runs straight into `Saved decision:`".
    fn header_lines(&self, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(Span::styled(
                " Project trust",
                theme.accent_style().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(format!(" {}", self.cwd), theme.muted_style())),
        ];
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" Saved decision: {}", self.saved_decision),
            theme.muted_style(),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                " Current session: {}",
                if self.session_trusted { "trusted" } else { "untrusted" }
            ),
            theme.muted_style(),
        )));
        lines
    }

    /// The option list body lines (cursor + label + saved-decision checkmark), test-visible.
    ///
    /// S20: upstream appends `theme.fg("success", " ✓")` to the option matching the persisted
    /// decision — `const checkmark = isCurrent ? theme.fg("success", " ✓") : ""`
    /// (`trust-selector.ts:110`), joined as `` `${prefix}${label}${checkmark}` `` at `:113`. The
    /// marker is OUTSIDE the accent/text colouring of the label, so it stays `success` green on the
    /// highlighted row too, and it is independent of the cursor: the saved option keeps its `✓`
    /// while the highlight moves elsewhere.
    fn option_lines(&self, theme: &UiTheme) -> Vec<Line<'static>> {
        self.labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let selected = i == self.selected;
                let prefix = if selected { "→ " } else { "  " };
                let style = if selected { theme.accent_style() } else { theme.base_style() };
                let mut spans = vec![
                    Span::styled(prefix.to_string(), style),
                    Span::styled(label.clone(), style),
                ];
                if self.saved_index == Some(i) {
                    spans.push(Span::styled(" ✓", theme.success_style()));
                }
                Line::from(spans)
            })
            .collect()
    }
}

impl Selector for TrustSelector {
    fn desired_height(&self, _width: u16) -> u16 {
        // rule + blank + title + cwd + blank + saved + session + blank + options + blank + hint +
        // blank + rule — `TrustSelectorComponent`'s full child list, see `render`.
        (self.labels.len() as u16).saturating_add(12)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // L4/SYS-3 + S20. `TrustSelectorComponent` (`trust-selector.ts:52-87`) is the densest
        // envelope in the batch — **five** `Spacer(1)` children, in this order:
        //   `DynamicBorder`(:52) · `Spacer`(:53) · "Project trust"(:54) · cwd(:55) · `Spacer`(:56) ·
        //   "Saved decision: …"(:57-66) · "Current session: …"(:67-69) · `Spacer`(:70) ·
        //   listContainer(:72-73) · `Spacer`(:74) · hint(:75-85) · `Spacer`(:86) ·
        //   `DynamicBorder`(:87).
        // cyrup already had two of the five (before the options and before the hint); the three
        // added here are `:53`, `:56` and `:86`. Note there is deliberately NO blank between the
        // title and the cwd, nor between the two header value rows — those pairs are adjacent
        // upstream.
        //
        // Unconditional, because upstream's `Spacer` children are — `Paragraph` clips the vector
        // top-first, matching pi's layout engine (see `crate::selector::stack_rows`' doc).
        let blank = || Line::from("");
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(border_rule_line(area.width, theme));
        lines.push(blank());
        lines.extend(self.header_lines(theme));
        lines.push(blank());
        lines.extend(self.option_lines(theme));
        lines.push(blank());
        lines.push(Line::from(Span::styled(
            " ↑/↓ navigate · enter save · esc cancel",
            theme.dim_style(),
        )));
        lines.push(blank());
        lines.push(border_rule_line(area.width, theme));
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.selected = self.selected.saturating_sub(1);
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                if self.selected + 1 < self.labels.len() {
                    self.selected += 1;
                }
                SelectorOutcome::Redraw
            }
            // Confirm carries the chosen option INDEX (the chrome maps it back to the trust option's
            // store updates); the slot closes.
            Some(SelectAction::Confirm) => SelectorOutcome::Confirm(self.selected.to_string()),
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => SelectorOutcome::Ignored,
        }
    }
}

/// A full-width `─` rule [`Paragraph`] (the `DynamicBorder`, shared with [`crate::selector`]).
fn border_rule(width: u16, theme: &UiTheme) -> Paragraph<'static> {
    Paragraph::new(border_rule_line(width, theme))
}

/// A full-width `─` rule as a single [`Line`] (for multi-line paragraph composition).
fn border_rule_line(width: u16, theme: &UiTheme) -> Line<'static> {
    let rule = "─".repeat(width.max(1) as usize);
    Line::from(Span::styled(rule, theme.border_style()))
}
