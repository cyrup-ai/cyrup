//! The editor-swap selector engine (spec/tui/05 §1.1, §3; port of Pi's `showSelector`
//! `interactive-mode.ts:3922-3933` + the `*-selector.ts` components).
//!
//! Pi's first-party selectors are **not** floating overlays: they *replace the input editor in place*
//! in the bottom inline region, full-width, delimited top and bottom by a `DynamicBorder`
//! (`dynamic-border.ts` — a full-width `─` rule, no box corners), and they push the message history up
//! (spec/tui/05 §1.1, §11). This module realizes that as the [`Selector`] trait (the input-slot
//! occupant) plus a shared [`ListSelector`] engine over [`SelectList`](crate::select_list::SelectList),
//! and the three dependency-free selectors Pi opens this way: thinking (`thinking-selector.ts`),
//! show-images (`show-images-selector.ts`), and theme with live preview (`theme-selector.ts`).
//!
//! The floating `OverlayManager` z-stack (spec/tui/05 §2) backs only extension-custom UI + the
//! hotkeys/help popup and is gated to the outer (L7) layer — the 13 first-party selectors are all
//! editor-swap, exactly as Pi (§1.2 "Decision for parity").

use cyrup_resources::theme::builtin_themes;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{SelectAction, SelectKeymap};
use crate::select_list::{ColumnLayout, SelectItem, SelectList};
use crate::theme::UiTheme;

/// Which first-party selector occupies the input slot (spec/tui/05 §7 `SelectorKind`). The chrome
/// interprets a [`SelectorOutcome::Confirm`] / [`SelectorOutcome::Preview`] against this kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectorKind {
    /// Reasoning-level picker (`thinking-selector.ts`).
    Thinking,
    /// Inline-images yes/no (`show-images-selector.ts`).
    ShowImages,
    /// Theme picker with live preview (`theme-selector.ts`).
    Theme,
}

/// The routing outcome of feeding one key to the active selector (spec/tui/05 §3.1
/// `SelectorOutcome`). The chrome closes the slot and restores the editor on `Confirm`/`Cancel`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectorOutcome {
    /// The key was not a selector binding — nothing changed.
    Ignored,
    /// Internal state changed (e.g. selection moved); redraw, stay open.
    Redraw,
    /// Selection moved on a live-preview selector; carries the now-highlighted value
    /// (`theme-selector.ts:54-56` `onSelectionChange → onPreview`).
    Preview(String),
    /// The highlighted row was confirmed (`tui.select.confirm`); carries its value.
    Confirm(String),
    /// The selector was dismissed (`tui.select.cancel` — `Esc`/`Ctrl+C`).
    Cancel,
}

/// The input-slot occupant contract (spec/tui/05 §3.1). Object-safe so the chrome can hold a
/// `Box<dyn Selector>` in place of the editor.
pub trait Selector: Send {
    /// Lines this selector wants this frame, driving the live-region height (top rule + body + bottom
    /// rule). The chrome clamps this to the available rows.
    fn desired_height(&self, width: u16) -> u16;
    /// Render into `area` (the editor slot's `Rect`).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme);
    /// Route one key through the [`SelectKeymap`], returning the outcome.
    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome;
    /// Cursor position for an embedded search `Input` (none for these pure-list selectors).
    fn cursor(&self) -> Option<(u16, u16)> {
        None
    }
}

/// The shared list-selector engine (spec/tui/05 §3.2 `ListView<T>`): a [`SelectList`] body wrapped in
/// the top/bottom `DynamicBorder` chrome, with a parallel `values` vector returned on confirm and an
/// optional live-preview hook.
pub struct ListSelector {
    list: SelectList,
    /// Confirm value per row, parallel to the list items (`SelectItem.value`, e.g.
    /// `thinking-selector.ts:35` `value: level`).
    values: Vec<String>,
    /// Whether a selection move emits [`SelectorOutcome::Preview`] (theme live preview only).
    preview: bool,
}

impl ListSelector {
    /// Build from `(value, label, description)` rows, the max visible window, and whether the selector
    /// previews on navigation. The selection preselects `selected`. Column layout is Pi's selector
    /// default `{min:12,max:32}` (`THINKING_SELECT_LIST_LAYOUT` etc.).
    fn new(
        rows: Vec<(String, String, Option<String>)>,
        max_visible: u16,
        selected: usize,
        preview: bool,
    ) -> Self {
        let mut values = Vec::with_capacity(rows.len());
        let mut items = Vec::with_capacity(rows.len());
        for (value, label, desc) in rows {
            values.push(value);
            items.push(SelectItem::new(label, desc));
        }
        let mut list = SelectList::new(items, ColumnLayout::SLASH);
        list.set_max_visible(max_visible);
        list.set_selected(selected);
        ListSelector { list, values, preview }
    }

    /// The value of the currently-highlighted row (empty string if the list is empty — never panics).
    fn current_value(&self) -> String {
        self.values.get(self.list.selected()).cloned().unwrap_or_default()
    }

    /// Read-only access to the inner list (tests / chrome inspection).
    pub fn list(&self) -> &SelectList {
        &self.list
    }

    // ---- Pi selector constructors -----------------------------------------------------------

    /// Thinking-level picker (`thinking-selector.ts:11-55`): one row per available level with its
    /// token-estimate description, `maxVisible = levels.len()`, preselecting `current`.
    pub fn thinking(current: &str) -> Self {
        // `LEVEL_DESCRIPTIONS` (`thinking-selector.ts:11-18`), in Pi's order.
        const LEVELS: [(&str, &str); 6] = [
            ("off", "No reasoning"),
            ("minimal", "Very brief reasoning (~1k tokens)"),
            ("low", "Light reasoning (~2k tokens)"),
            ("medium", "Moderate reasoning (~8k tokens)"),
            ("high", "Deep reasoning (~16k tokens)"),
            ("xhigh", "Maximum reasoning (~32k tokens)"),
        ];
        let rows: Vec<_> = LEVELS
            .iter()
            .map(|(level, desc)| ((*level).to_string(), (*level).to_string(), Some((*desc).to_string())))
            .collect();
        let selected = LEVELS.iter().position(|(l, _)| *l == current).unwrap_or(0);
        ListSelector::new(rows, LEVELS.len().min(u16::MAX as usize) as u16, selected, false)
    }

    /// Inline-images yes/no (`show-images-selector.ts:19-31`): `maxVisible = 5`, preselecting
    /// `Yes` when currently on, else `No`.
    pub fn show_images(current: bool) -> Self {
        let rows = vec![
            ("yes".to_string(), "Yes".to_string(), Some("Show images inline in terminal".to_string())),
            ("no".to_string(), "No".to_string(), Some("Show text placeholder instead".to_string())),
        ];
        let selected = if current { 0 } else { 1 };
        ListSelector::new(rows, 5, selected, false)
    }

    /// Theme picker with live preview (`theme-selector.ts:27-56`): one row per available theme,
    /// `maxVisible = 10`, the current theme marked `(current)`, preselecting it. Navigation emits
    /// [`SelectorOutcome::Preview`] so the whole UI re-themes as the highlight moves.
    pub fn theme(current: &str) -> Self {
        let mut rows = Vec::new();
        let mut selected = 0usize;
        for (i, theme) in builtin_themes().iter().enumerate() {
            let key = theme.key.as_str().to_string();
            let is_current = key == current;
            if is_current {
                selected = i;
            }
            let desc = is_current.then(|| "(current)".to_string());
            rows.push((key.clone(), key, desc));
        }
        ListSelector::new(rows, 10, selected, true)
    }
}

impl Selector for ListSelector {
    fn desired_height(&self, _width: u16) -> u16 {
        // Top `DynamicBorder` + list body + bottom `DynamicBorder` (spec/tui/05 §3, §11).
        self.list.rendered_height().saturating_add(2)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let [top, body, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(border_rule(top.width, theme), top);
        let lines = self.list.lines(body.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), body);
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.list.select_up();
                self.moved()
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                self.list.select_down();
                self.moved()
            }
            Some(SelectAction::Confirm) => SelectorOutcome::Confirm(self.current_value()),
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => SelectorOutcome::Ignored,
        }
    }
}

impl ListSelector {
    /// The outcome of a navigation move: a live-preview emit for previewing selectors, else a redraw
    /// (`select-list.ts:103-108` `notifySelectionChange` → `onSelectionChange`).
    fn moved(&self) -> SelectorOutcome {
        if self.preview {
            SelectorOutcome::Preview(self.current_value())
        } else {
            SelectorOutcome::Redraw
        }
    }
}

/// A full-width `─` rule styled `border`, matching Pi's `DynamicBorder`
/// (`dynamic-border.ts:23` `color("─".repeat(max(1,width)))`) — **not** a ratatui `Block` border, so
/// it spans the whole inline width with no corners (spec/tui/05 §11).
fn border_rule(width: u16, theme: &UiTheme) -> Paragraph<'static> {
    let rule = "─".repeat(width.max(1) as usize);
    Paragraph::new(Line::from(Span::styled(rule, theme.border_style())))
}
