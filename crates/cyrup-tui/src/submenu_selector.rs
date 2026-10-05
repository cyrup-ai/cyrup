//! A `/settings` submenu step — a port of pi's `SelectSubmenu`
//! (`packages/coding-agent/src/modes/interactive/components/settings-submenu.ts:31-133` @0.84.3)
//! together with the `Step i/N · ` label `SteppedSubmenu` prepends to each step's description
//! (`:204-214`).
//!
//! Upstream a submenu is a child COMPONENT swapped into the open settings list
//! (`settings-list.ts:214-236`); cyrup mounts it as its own frame in the input slot with the parent
//! list hanging off [`crate::app::ActiveSelector::parent`], so the two `DynamicBorder` rules below
//! are cyrup framing rather than upstream children — the same adaptation the theme submenu already
//! makes by riding [`crate::selector::ListSelector`], which draws them too.
//!
//! What is NOT framing and IS upstream: the bold accent title (`:59`), the muted description row
//! (`:62-65`), the optional search `Input` (`:68-74`), the bare full-width `SelectList` (`:80-83`)
//! and the dim footer whose wording depends on whether the step is searchable (`:86-91`). The
//! per-model thinking flow (`settings-selector.ts:578-644`) declares `searchable: true` on its
//! model step (`:611`) and nothing on its level step, which is the only reason the search box is
//! optional here.

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::fuzzy;
use crate::keymap::{EditorKeymap, SelectAction, SelectKeymap};
use crate::select_list::{ColumnLayout, SelectItem, SelectList};
use crate::selector::{
    Selector, SelectorOutcome, border_rule_line, input_line_spans, list_outcome, list_pointer,
};
use crate::text_input::{Input, InputOutcome};
use crate::theme::UiTheme;

/// `MODEL_PICKER_LAYOUT` (`settings-selector.ts:27`) — the wider primary column the model step
/// overrides `SUBMENU_SELECT_LIST_LAYOUT` (`settings-submenu.ts:15-18`) with, so
/// `provider/long-model-id` labels are not clipped at 32 columns.
pub(crate) const MODEL_PICKER_LAYOUT: ColumnLayout = ColumnLayout {
    primary_min: 12,
    primary_max: 46,
};

/// One step of a `/settings` submenu.
pub struct SubmenuSelector {
    /// `allOptions` (`settings-submenu.ts:34`): every row as `(value, label, description)`.
    all: Vec<(String, String, Option<String>)>,
    /// The values parallel to [`Self::list`]'s items — the filtered view.
    values: Vec<String>,
    list: SelectList,
    layout: ColumnLayout,
    /// `theme.bold(theme.fg("accent", title))` (`:59`).
    title: String,
    /// `theme.fg("muted", description)` (`:64`), already carrying `SteppedSubmenu`'s
    /// `Step i/N · ` prefix (`:206`). Empty means no row at all (`if (description)`, `:62`).
    description: String,
    /// `submenuOptions?.searchable` (`:68`): `Some` builds the search `Input`, `None` routes every
    /// key straight to the list (`:117-131`).
    input: Option<Input>,
    /// The `maxVisible` cap, `Math.min(options.length, 10)` (`:93`).
    max_visible: usize,
    /// `onSelectionChange` (`settings-submenu.ts:44`, `:100-102`): a move emits
    /// [`SelectorOutcome::Preview`] with the value now highlighted.
    preview: bool,
}

impl SubmenuSelector {
    /// Build one step (`settings-submenu.ts:40-91`). `selected` preselects a row, standing in for
    /// upstream's `currentValue` lookup (`:95-96`).
    pub fn new(
        title: String,
        description: String,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
        searchable: bool,
        layout: ColumnLayout,
    ) -> Self {
        let mut sel = SubmenuSelector {
            all: rows,
            values: Vec::new(),
            list: SelectList::new(Vec::new(), layout),
            layout,
            title,
            description,
            input: if searchable { Some(Input::new()) } else { None },
            max_visible: 10,
            preview: false,
        };
        sel.rebuild();
        sel.list.set_selected(selected);
        sel
    }

    /// `buildSelectList` (`settings-submenu.ts:92-105`) over the rows passing the live query.
    ///
    /// `applyFilter` rebuilds with `preselect: ""` (`:114`), i.e. the highlight returns to the top
    /// of the filtered set on every keystroke — unlike `thinking-selector.ts:115`, which preserves
    /// it. Reproduced rather than unified: the two upstream components genuinely differ here.
    fn rebuild(&mut self) {
        let query = self
            .input
            .as_ref()
            .map(|i| i.value().to_string())
            .unwrap_or_default();
        let indices: Vec<usize> = if query.is_empty() {
            (0..self.all.len()).collect()
        } else {
            // `fuzzyFilter(allOptions, query, (item) => `${item.label} ${item.description ?? ""}`)`
            // (`:109-111`).
            let haystacks: Vec<String> = self
                .all
                .iter()
                .map(|(_, label, desc)| format!("{label} {}", desc.as_deref().unwrap_or("")))
                .collect();
            fuzzy::filter(&haystacks, &query, String::as_str)
                .into_iter()
                .map(|m| m.index)
                .collect()
        };
        let mut values = Vec::with_capacity(indices.len());
        let mut items = Vec::with_capacity(indices.len());
        for index in indices {
            if let Some((value, label, desc)) = self.all.get(index) {
                values.push(value.clone());
                items.push(SelectItem::new(label.clone(), desc.clone()));
            }
        }
        let mut list = SelectList::new(items, self.layout).with_no_match("No matches".to_string());
        let window = values
            .len()
            .min(self.max_visible)
            .clamp(1, usize::from(u16::MAX)) as u16;
        list.set_max_visible(window);
        self.values = values;
        self.list = list;
    }

    /// The highlighted value, or `None` when the query matched nothing.
    fn current_value(&self) -> Option<String> {
        self.values.get(self.list.selected()).cloned()
    }

    /// Emit [`SelectorOutcome::Preview`] as the highlight moves — pi's `onSelectionChange` argument.
    #[must_use]
    pub fn with_preview(mut self) -> Self {
        self.preview = true;
        self
    }

    /// The answer to a move: the preview of the value now under the highlight, or a plain redraw.
    fn moved(&self) -> SelectorOutcome {
        match (self.preview, self.current_value()) {
            (true, Some(value)) => SelectorOutcome::Preview(value),
            _ => SelectorOutcome::Redraw,
        }
    }

    /// Read-only access to the inner list (tests / chrome inspection).
    pub fn list(&self) -> &SelectList {
        &self.list
    }

    /// The rendered envelope, in pi's child order (`settings-submenu.ts:56-91`).
    fn lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut lines = self.head_lines(width, theme);
        lines.extend(self.list.lines(width, theme)); // :80-83
        lines.push(Line::from("")); // :86
        lines.push(Line::from(Span::styled(
            if self.input.is_some() {
                "  Type to filter \u{b7} Enter to select \u{b7} Esc to go back"
            } else {
                "  Enter to select \u{b7} Esc to go back"
            },
            theme.dim_style(),
        ))); // :87-91
        lines.push(border_rule_line(width, theme));
        lines
    }

    /// Everything above the list (`settings-submenu.ts:56-77`), up to and including the blank
    /// before it. Its length is the slot row the list's first item is painted on, which is what
    /// [`Selector::pointer`] hit-tests against.
    fn head_lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(8);
        lines.push(border_rule_line(width, theme));
        lines.push(Line::from(Span::styled(
            self.title.clone(),
            theme
                .accent_style()
                .add_modifier(ratatui::style::Modifier::BOLD),
        ))); // :59
        if !self.description.is_empty() {
            lines.push(Line::from("")); // :63
            lines.push(Line::from(Span::styled(
                self.description.clone(),
                theme.muted_style(),
            )));
            // :64
        }
        if let Some(input) = &self.input {
            lines.push(Line::from("")); // :69
            lines.push(Line::from(input_line_spans(
                input.value(),
                input.cursor(),
                width,
                theme,
            ))); // :70-74
        }
        lines.push(Line::from("")); // :77
        lines
    }

    /// The slot row the list's first item is painted on: the number of rows above it.
    fn first_list_row(&self, width: u16) -> u16 {
        self.head_lines(width, UiTheme::default_ref())
            .len()
            .min(usize::from(u16::MAX)) as u16
    }
}

impl Selector for SubmenuSelector {
    fn desired_height(&self, width: u16) -> u16 {
        self.lines(width, UiTheme::default_ref())
            .len()
            .clamp(0, usize::from(u16::MAX)) as u16
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let lines = self.lines(area.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn pointer(&mut self, area: Rect, event: crate::app::Pointer) -> SelectorOutcome {
        let top = self.first_list_row(area.width);
        // The search `Input` (`settings-submenu.ts:70-74`, a bare container child) is the row above
        // the blank that closes the head: a press there places its caret.
        if let Some(input) = self.input.as_mut()
            && input.pointer_in_row(
                event,
                top.saturating_sub(2),
                crate::selector::INPUT_PROMPT_COLS,
                area.width,
            )
        {
            return SelectorOutcome::Redraw;
        }
        let available = area.height.saturating_sub(top);
        let answer = list_pointer(&mut self.list, top, available, event);
        list_outcome(
            answer,
            || self.moved(),
            || match self.current_value() {
                Some(value) => SelectorOutcome::Confirm(value),
                None => SelectorOutcome::Redraw,
            },
        )
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // `isNav` is exactly these four ids (`settings-submenu.ts:119-124`); with no search input
        // every key goes to the list instead (`:130`).
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.list.select_up();
                self.moved()
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                self.list.select_down();
                self.moved()
            }
            Some(SelectAction::Confirm) => match self.current_value() {
                Some(value) => SelectorOutcome::Confirm(value),
                None => SelectorOutcome::Redraw,
            },
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            _ => match &mut self.input {
                // `this.searchInput.handleInput(data); this.applyFilter(…)` (`:126-127`).
                Some(input) => match input.handle_key(key) {
                    InputOutcome::Edited => {
                        self.rebuild();
                        SelectorOutcome::Redraw
                    }
                    InputOutcome::Moved => SelectorOutcome::Redraw,
                    InputOutcome::Ignored => SelectorOutcome::Ignored,
                },
                None => SelectorOutcome::Ignored,
            },
        }
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn set_editor_keymap(&mut self, keymap: &EditorKeymap) {
        if let Some(input) = &mut self.input {
            input.set_editor_keymap(keymap);
        }
    }

    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        match &mut self.input {
            Some(input) => {
                input.paste(text);
                self.rebuild();
                SelectorOutcome::Redraw
            }
            None => SelectorOutcome::Ignored,
        }
    }
}
