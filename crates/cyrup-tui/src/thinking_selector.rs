//! The `/thinking` level picker — a port of pi's `ThinkingSelectorComponent`
//! (`packages/coding-agent/src/modes/interactive/components/thinking-selector.ts` @0.84.3).
//!
//! Upstream stopped being a bare `DynamicBorder` + `SelectList` + `DynamicBorder` list: at 0.84.3
//! the constructor (`:77-97`) is
//!
//! ```text
//! DynamicBorder(:77) · Spacer(:78) · Text("Thinking Level")(:79) · Spacer(:80)
//! Text(`${keyDisplayText("app.thinking.cycle")} cycles thinking levels in-session`)(:81)
//! Spacer(:82) · Input(:84-86) · Spacer(:87) · SelectList(:92) · Spacer(:93)
//! Text(dim, "  Enter to select · Ctrl+S to set as default · Esc to cancel")(:94)
//! DynamicBorder(:97)
//! ```
//!
//! i.e. a titled, self-describing dialog with an embedded fuzzy search `Input` — the same shape
//! [`crate::model_selector::ModelSelector`] already has. It therefore lives here as its own
//! component rather than as a [`crate::selector::ListSelector`] constructor: the three kinds that
//! ARE still border/list/border upstream (`show-images-selector.ts:25,41,44`,
//! `theme-selector.ts:35,58,61`) keep the shared engine flush, and the `Input`/title/hint chrome
//! never leaks onto them — the failure mode `SelectorKind::draws_hint_row`'s doc exists to prevent.
//!
//! The rows are built from a caller-supplied ladder (pi's `availableLevels` constructor argument,
//! `:58`, fed from `session.getAvailableThinkingLevels()` at `interactive-mode.ts:4792`), not from a
//! hardcoded table, so a non-reasoning model offers `off` alone.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::fuzzy;
use crate::keymap::{EditorKeymap, SelectAction, SelectKeymap};
use crate::select_list::{ColumnLayout, SelectItem, SelectList};
use crate::selector::{border_rule_line, input_line_spans, Selector, SelectorOutcome};
use crate::text_input::{Input, InputOutcome};
use crate::theme::UiTheme;

/// `LEVEL_DESCRIPTIONS` (`thinking-selector.ts:24-32`), as a lookup rather than a row source so it
/// labels whatever ladder the session hands us instead of enumerating one of its own. pi's `max`
/// commit renamed the `xhigh` copy from "Maximum" to "Extra-high" and gave "Maximum reasoning" to
/// the new top rung.
///
/// A level with no upstream copy (there is none today) yields an empty description, which
/// [`SelectItem`] renders as a bare label — never a panic and never an invented string.
pub(crate) fn level_description(level: &str) -> &'static str {
    match level {
        "off" => "No reasoning",
        "minimal" => "Very brief reasoning (~1k tokens)",
        "low" => "Light reasoning (~2k tokens)",
        "medium" => "Moderate reasoning (~8k tokens)",
        "high" => "Deep reasoning (~16k tokens)",
        "xhigh" => "Extra-high reasoning (~32k tokens)",
        "max" => "Maximum reasoning",
        _ => "",
    }
}

/// The `/thinking` picker (`ThinkingSelectorComponent`).
pub struct ThinkingSelector {
    /// `allItems` (`thinking-selector.ts:41,69-74`): every available level as
    /// `(value/label, description)`, the persisted default already badged ` · default` (`:73`).
    all: Vec<(String, String)>,
    /// The values parallel to [`Self::list`]'s items — the FILTERED view, rebuilt by
    /// [`Self::apply_filter`] exactly as pi swaps in a fresh `SelectList` (`:111-119`).
    values: Vec<String>,
    list: SelectList,
    /// The embedded search box (`:84-86`). Its `onSubmit` forwards `\r` to the list (`:85`), which
    /// here is simply the `tui.select.confirm` arm running before the input ever sees the key.
    input: Input,
    /// `keyDisplayText("app.thinking.cycle")` (`:81`) — resolved from the app's LIVE table at
    /// construction, so a rebind changes the sentence.
    cycle_key: String,
    /// **[CYRUP-DELTA]** `hideThinkingBlock` at construction. Upstream's picker never states it,
    /// because upstream's `setHideThinkingBlock` re-renders the prior assistant messages
    /// (`assistant-message.ts:57-62`) and the suppression is visible on screen. Under ADR-0001
    /// cyrup's committed rows have already left the render tree (`TUI-N06`), so this is the one
    /// place a user picks a reasoning level while the output of that choice is being swallowed.
    hidden: bool,
    /// The live `app.thinking.toggle` label, so the warning names the actual key rather than a
    /// hardcoded `Ctrl+T` — the same reason [`Self::cycle_key`] is resolved and not literal.
    toggle_key: String,
}

impl ThinkingSelector {
    /// Build the picker (`thinking-selector.ts:56-98`).
    ///
    /// * `levels` — pi's `availableLevels`, the session's `getAvailableThinkingLevels()`.
    /// * `current` — `session.thinkingLevel ?? DEFAULT_THINKING_LEVEL` (`interactive-mode.ts:4791`),
    ///   the preselected row.
    /// * `default_level` — `settingsManager.getDefaultThinkingLevel() ?? DEFAULT_THINKING_LEVEL`
    ///   (`:4797`), the row that gets the ` · default` badge.
    /// * `cycle_key` — the already-formatted `app.thinking.cycle` label for the hint sentence.
    pub fn new(
        levels: &[String],
        current: &str,
        default_level: &str,
        cycle_key: String,
        hidden: bool,
        toggle_key: String,
    ) -> Self {
        let all = levels
            .iter()
            .map(|level| {
                let desc = level_description(level);
                // `level === defaultThinkingLevel ? `${…} · default` : …` (`:72-73`).
                let desc = if level == default_level {
                    format!("{desc} \u{b7} default")
                } else {
                    desc.to_string()
                };
                (level.clone(), desc)
            })
            .collect();
        let mut sel = ThinkingSelector {
            all,
            values: Vec::new(),
            list: SelectList::new(Vec::new(), ColumnLayout::SLASH),
            input: Input::new(),
            cycle_key,
            hidden,
            toggle_key,
        };
        sel.rebuild(current);
        sel
    }

    /// `buildSelectList(items, preselect)` (`thinking-selector.ts:100-109`) over the rows passing
    /// the live query: `maxVisible = max(1, items.length)`, and `setSelectedIndex` only when the
    /// preselected value is actually present (`:102-105`).
    fn rebuild(&mut self, preselect: &str) {
        // `applyFilter` (`:111-119`): an empty query means the whole ladder, otherwise
        // `fuzzyFilter(allItems, query, (item) => `${item.label} ${item.description ?? ""}`)`.
        let query = self.input.value().to_string();
        let indices: Vec<usize> = if query.is_empty() {
            (0..self.all.len()).collect()
        } else {
            let haystacks: Vec<String> =
                self.all.iter().map(|(label, desc)| format!("{label} {desc}")).collect();
            fuzzy::filter(&haystacks, &query, String::as_str).into_iter().map(|m| m.index).collect()
        };
        let mut values = Vec::with_capacity(indices.len());
        let mut items = Vec::with_capacity(indices.len());
        for index in indices {
            if let Some((level, desc)) = self.all.get(index) {
                values.push(level.clone());
                items.push(SelectItem::new(level.clone(), Some(desc.clone())));
            }
        }
        let mut list = SelectList::new(items, ColumnLayout::SLASH);
        list.set_max_visible(values.len().clamp(1, usize::from(u16::MAX)) as u16);
        if let Some(index) = values.iter().position(|v| v == preselect) {
            list.set_selected(index);
        }
        self.values = values;
        self.list = list;
    }

    /// Re-filter around the currently-highlighted value, pi's
    /// `const selectedValue = this.selectList.getSelectedItem()?.value` → `buildSelectList(filtered,
    /// selectedValue)` (`thinking-selector.ts:115-116`).
    fn apply_filter(&mut self) {
        let preselect = self.current_value().unwrap_or_default();
        self.rebuild(&preselect);
    }

    /// The highlighted level, or `None` when the query matched nothing.
    fn current_value(&self) -> Option<String> {
        self.values.get(self.list.selected()).cloned()
    }

    /// Read-only access to the inner list (tests / chrome inspection).
    pub fn list(&self) -> &SelectList {
        &self.list
    }

    /// The rendered envelope, in pi's child order (`thinking-selector.ts:77-97`).
    ///
    /// The two header `Text` children are **unstyled** upstream — `new Text("Thinking Level", 0, 0)`
    /// (`:79`) and `new Text(`${…} cycles …`, 0, 0)` (`:81`) receive raw strings with no
    /// `theme.bold`/`theme.fg` wrapper, unlike `SelectSubmenu`'s title
    /// (`settings-submenu.ts:59`) — so they render in the base style here too. Only the footer is
    /// coloured (`theme.fg("dim", …)`, `:94`).
    fn lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.values.len().saturating_add(10));
        lines.push(border_rule_line(width, theme)); // :77
        lines.push(Line::from("")); // :78
        lines.push(Line::from(Span::styled("Thinking Level", theme.base_style()))); // :79
        lines.push(Line::from("")); // :80
        lines.push(Line::from(Span::styled(
            format!("{} cycles thinking levels in-session", self.cycle_key),
            theme.base_style(),
        ))); // :81
        // **[CYRUP-DELTA]** — additive, and only when the flag is on: with it off this dialog is
        // byte-identical to upstream's. Picking a level here while `hideThinkingBlock` is set
        // spends reasoning tokens whose output is replaced by the static `Thinking...` label, and
        // nothing else on screen says so.
        if self.hidden {
            lines.push(Line::from(""));
            let how = if self.toggle_key.is_empty() {
                "set hideThinkingBlock to false to show it".to_string()
            } else {
                format!("{} shows it", self.toggle_key)
            };
            lines.push(Line::from(Span::styled(
                format!("Thinking output is HIDDEN - {how}"),
                theme.warning_style(),
            )));
        }
        lines.push(Line::from("")); // :82
        // The shared `Input.render` prompt — an unstyled `"> "` at column 0 (`input.ts:380`),
        // because `:86` adds `this.searchInput` to the container as a bare child.
        lines.push(Line::from(input_line_spans(
            self.input.value(),
            self.input.cursor(),
            width,
            theme,
        ))); // :84-86
        lines.push(Line::from("")); // :87
        // `:92` adds the `SelectList` straight to the container, so it is laid out at the FULL
        // container width and its rows start at column 0 — no `paddingX` wrapper, no inset.
        lines.extend(self.list.lines(width, theme)); // :92
        lines.push(Line::from("")); // :93
        lines.push(Line::from(Span::styled(
            "  Enter to select \u{b7} Ctrl+S to set as default \u{b7} Esc to cancel",
            theme.dim_style(),
        ))); // :94
        lines.push(border_rule_line(width, theme)); // :97
        lines
    }
}

impl Selector for ThinkingSelector {
    fn desired_height(&self, width: u16) -> u16 {
        self.lines(width, UiTheme::default_ref()).len().clamp(0, usize::from(u16::MAX)) as u16
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let lines = self.lines(area.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // `if (matchesKey(keyData, "ctrl+s") && this.onSelectAsDefault)` (`thinking-selector.ts:
        // 122-126`) — a LITERAL chord, not a binding id, checked before the keymap so a rebound
        // `tui.select.*` cannot shadow it. Unconditional here because this picker is only ever
        // opened on the path that can persist (`interactive-mode.ts:4796` always wires the
        // callback).
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return match self.current_value() {
                Some(level) => SelectorOutcome::ConfirmDefault(level),
                None => SelectorOutcome::Redraw,
            };
        }
        // `isNav` is exactly these four ids (`:129-133`); everything else — PageUp/PageDown
        // included — falls through to the search input, where pi's `Input` drops what it cannot
        // insert.
        match keymap.action_for(key) {
            Some(SelectAction::Up) => {
                self.list.select_up();
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) => {
                self.list.select_down();
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Confirm) => match self.current_value() {
                Some(level) => SelectorOutcome::Confirm(level),
                None => SelectorOutcome::Redraw,
            },
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            _ => {
                // `this.searchInput.handleInput(keyData); this.applyFilter(…)` (`:139-140`).
                match self.input.handle_key(key) {
                    InputOutcome::Edited => {
                        self.apply_filter();
                        SelectorOutcome::Redraw
                    }
                    InputOutcome::Moved => SelectorOutcome::Redraw,
                    InputOutcome::Ignored => SelectorOutcome::Ignored,
                }
            }
        }
    }

    /// Adopt the live `tui.editor.*` table so the embedded [`Input`] resolves word motion / kill
    /// ring / undo through the user's own bindings (pi's `Input` calls `getKeybindings()` on every
    /// key, `input.ts:86`).
    fn set_editor_keymap(&mut self, keymap: &EditorKeymap) {
        self.input.set_editor_keymap(keymap);
    }

    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        self.input.paste(text);
        self.apply_filter();
        SelectorOutcome::Redraw
    }
}
