//! The settings/config + trust editor-swap selectors (spec/tui/05 §6; port of Pi's
//! `settings-selector.ts` (810), `config-selector.ts` (628), and `trust-selector.ts` (134)).
//!
//! Pi's settings picker (`/settings`) is a `SettingsList` (`packages/tui/src/components/settings-list.ts`):
//! a windowed list of `SettingItem`s, each rendering a **label** on the left and the **current value**
//! on the right, where `Enter`/`Space` **cycles** the value through a fixed set and applies it *live*
//! (`onChange`) while the slot stays open — plus, all of it `SettingsList`'s own and none of it the
//! shared `SelectList`'s, a search `Input`, a `min(30, widest)` label column, a `(i/n)` scroll
//! readout, the highlighted row's wrapped description and a `Type to search …` hint (S16/S33).
//! `/config` is a **different** upstream component ([`crate::config_selector`]) that only looks
//! similar. The `/trust` picker (`trust-selector.ts`) is a small list
//! with a multi-line header (cwd + saved decision + current-session trust) whose `Enter` **confirms**
//! the chosen option (writing the trust store) and closes. Both occupy the input slot exactly like the
//! [`ListSelector`](crate::selector::ListSelector), delimited by the full-width `DynamicBorder`.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{
    border_rule, border_rule_line, centered_window, input_line_spans, stack_rows, Selector,
    SelectorOutcome,
};
use crate::text_width::{str_width, truncate_line_to_width, truncate_to_width};
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
    /// Optional description (Pi `SettingItem.description`, `settings-list.ts:12-13`). Rendered as
    /// the wrapped, dim block under the list while this row is highlighted (`:152-160`) — see
    /// [`SettingRow::with_description`]. Was documented "unused" and never set, which is the whole
    /// of S16's "never explains what the highlighted setting does".
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

    /// Attach the row's `description` (Pi `SettingItem.description`, `settings-list.ts:12-13`) —
    /// the text `SettingsList` renders as its own wrapped, dim block under the list whenever this
    /// row is the highlighted one (`:152-160`). Every one of upstream's `/settings` items carries
    /// one; the field was previously documented "unused in the compact grid" and nothing ever set
    /// it, which is why the block never appeared (S16).
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// `new SettingsList(items, 10, …)` — the `maxVisible` `SettingsSelectorComponent` hands its list
/// (`settings-selector.ts:767`). A literal, not a terminal-derived number: `SettingsList` takes no
/// `terminalHeight` and never recomputes it.
const SETTINGS_MAX_VISIBLE: usize = 10;

/// `Math.min(30, …)` — the label column's upper bound (`settings-list.ts:121`). There is **no**
/// lower bound upstream: a list of short labels hugs them (S33). cyrup previously routed the rows
/// through `ColumnLayout::SLASH` (`{primary_min: 12, primary_max: 32}`), which is
/// `SLASH_COMMAND_SELECT_LIST_LAYOUT`'s policy on a *different* upstream component
/// (`select-list.ts`), so short labels were padded out to 12 and long ones capped at 32.
const LABEL_COLUMN_MAX: usize = 30;

/// The two-space label↔value separator (`settings-list.ts:137`).
const LABEL_VALUE_SEPARATOR: &str = "  ";

/// `addHintLine`'s search-enabled text (`settings-list.ts:242`). `/settings` constructs its
/// `SettingsList` with `{ enableSearch: true }` (`settings-selector.ts:872`), so it is always this
/// arm, never the `"  Enter/Space to change · Esc to cancel"` one.
const SETTINGS_HINT: &str = "  Type to search · Enter/Space to change · Esc to cancel";

/// The `/settings` picker — a port of pi's `SettingsList` (`packages/tui/src/components/settings-list.ts`)
/// as mounted by `SettingsSelectorComponent` (`settings-selector.ts:765,873,874`).
///
/// **Not** a [`crate::select_list::SelectList`]. `SettingsList` is a separate upstream component
/// with its own geometry (a `min(30, widest)` label column, a `"  "` separator, a `"→ "` accent
/// cursor), its own search box, its own scroll readout, its own description block and its own hint
/// row. Borrowing `SelectList`'s column policy for it was the root of S16/S33 — see the
/// per-property citations on [`SettingsSelector::lines`].
///
/// The component's whole child list is `DynamicBorder` (`:765`), the `SettingsList` (`:873`),
/// `DynamicBorder` (`:874`): **no title row and no `Spacer`**. Every blank this dialog shows is one
/// `SettingsList` pushes itself.
///
/// `Enter` cycles the highlighted row's value in place and emits [`SelectorOutcome::Apply`] (the
/// slot stays open); `Space` does the same but only while the search box is empty
/// (`settings-list.ts:186-188`); `Cancel` closes.
pub struct SettingsSelector {
    /// Retained for the chrome/tests. Upstream draws **no** title row for `/settings` (S16), so it
    /// is deliberately never rendered; see [`SettingsSelector::title`].
    title: String,
    rows: Vec<SettingRow>,
    /// Indices into `rows` surviving the fuzzy filter — `this.filteredItems`
    /// (`settings-list.ts:232` `fuzzyFilter(this.items, query, (item) => item.label)`).
    filtered: Vec<usize>,
    /// The highlight, an index into [`Self::filtered`] (`this.selectedIndex`).
    selected: usize,
    /// The search box (`this.searchInput`, `settings-list.ts:65`) — the shared single-line editing
    /// surface, caret and all.
    input: crate::text_input::Input,
}

impl SettingsSelector {
    /// Build from `title` + `rows`, preselecting the first row.
    pub fn new(title: impl Into<String>, rows: Vec<SettingRow>) -> Self {
        let mut sel = SettingsSelector {
            title: title.into(),
            rows,
            filtered: Vec::new(),
            selected: 0,
            input: crate::text_input::Input::new(),
        };
        sel.apply_filter();
        sel
    }

    /// The title the chrome opened this selector with. Never rendered — `SettingsSelectorComponent`
    /// has no title child (`settings-selector.ts:765-874`) — but kept so the chrome can still name
    /// the slot.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// `applyFilter` (`settings-list.ts:231-234`): refilter by the search text and reset the
    /// highlight to 0. `fuzzy::filter` is the port of `fuzzyFilter`, so an empty query keeps every
    /// row in its original order.
    fn apply_filter(&mut self) {
        self.filtered = crate::fuzzy::filter(&self.rows, self.input.value(), |r| r.label.as_str())
            .into_iter()
            .map(|m| m.index)
            .collect();
        self.selected = 0;
    }

    /// The displayed rows (`displayItems`, `settings-list.ts:106`) as `rows` indices.
    fn display(&self) -> &[usize] {
        &self.filtered
    }

    /// The `rows` index under the highlight, if any.
    fn selected_row_index(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    /// The highlighted row (tests / inspection).
    pub fn current(&self) -> Option<&SettingRow> {
        self.selected_row_index().and_then(|i| self.rows.get(i))
    }

    /// The highlight, as an index into the filtered list (tests / inspection).
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The live search query (tests / inspection).
    pub fn query(&self) -> &str {
        self.input.value()
    }

    /// `updateValue(id, newValue)` (`settings-list.ts:74-80`): write a row's displayed value in
    /// place, found by `id`, a no-op when there is no such row. Used by the submenu return path —
    /// upstream's `done()` does `item.currentValue = selectedValue` before `closeSubmenu()`
    /// (`:222-225`), so the row the user pops back to already shows what they just chose.
    ///
    /// Deliberately does NOT re-run [`Self::apply_filter`]: upstream never refilters here, and
    /// doing so would reset `selected` to 0 and throw away the cursor row `closeSubmenu` exists to
    /// restore. The row's position in `filtered` is unaffected by its value (the filter matches on
    /// `label`), so the untouched indices stay valid.
    pub fn update_value(&mut self, id: &str, value: &str) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.id == id) {
            row.value = value.to_string();
        }
    }

    /// `maxLabelWidth = Math.min(30, Math.max(...this.items.map((i) => visibleWidth(i.label))))`
    /// — `settings-list.ts:121`. **S33.** Two things this spelling pins down:
    ///
    /// * the reduce is over **`this.items`**, the full row set, not `displayItems` — so the value
    ///   column does not jump sideways as the search narrows the list;
    /// * `Math.max()` of an empty array is `-Infinity`, but `:98-104` returns before line 121 when
    ///   `items` is empty, so the empty case never reaches here — `max().unwrap_or(0)` matches.
    fn label_column_width(&self) -> usize {
        self.rows
            .iter()
            .map(|r| str_width(&r.label))
            .max()
            .unwrap_or(0)
            .min(LABEL_COLUMN_MAX)
    }

    /// `renderMainList` (`settings-list.ts:90-166`), line for line.
    pub fn lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let width_u16 = width;
        let width = usize::from(width);
        let mut lines: Vec<Line<'static>> = Vec::new();

        // `:93-96` — the search `Input` and the blank under it. Unconditional for `/settings`,
        // which is constructed `{ enableSearch: true }` (`settings-selector.ts:872`).
        // [`crate::selector::input_line_spans`] is the shared `Input.render` port (S31), so the
        // prompt here is upstream's bare unstyled `"> "` at column 0, same as every other dialog's.
        lines.push(Line::from(input_line_spans(
            self.input.value(),
            self.input.cursor(),
            width_u16,
            theme,
        )));
        lines.push(Line::from(""));

        // `:98-104` — no rows at all. NOT truncated upstream (only the "no matching" arm is), and
        // the hint follows because search is enabled.
        if self.rows.is_empty() {
            lines.push(Line::from(Span::styled("  No settings available", theme.dim_style())));
            self.push_hint_line(&mut lines, width, theme);
            return lines;
        }

        // `:106-111` — rows exist but the query matched none.
        let display = self.display();
        if display.is_empty() {
            lines.push(Line::from(Span::styled(
                truncate_to_width("  No matching settings", width, "..."),
                theme.dim_style(),
            )));
            self.push_hint_line(&mut lines, width, theme);
            return lines;
        }

        // `:113-118` — the window, centred on the highlight. `Math.min(a, len - maxVisible)` goes
        // negative when the list is shorter than the window and the outer `Math.max(0, …)` catches
        // it; `centered_window` performs that same clamp without underflowing (its own doc carries
        // the `select-list.ts:86-90` citation), including the `total <= visible` short-circuit.
        let total = display.len();
        let (start, end) = centered_window(self.selected, total, SETTINGS_MAX_VISIBLE);

        let label_w = self.label_column_width();
        // `:138-139` — `usedWidth = prefixWidth + maxLabelWidth + visibleWidth(separator)`, then
        // `valueMaxWidth = width - usedWidth - 2`. The trailing `- 2` is upstream's right gutter.
        let used = 2usize
            .saturating_add(label_w)
            .saturating_add(str_width(LABEL_VALUE_SEPARATOR));
        let value_max = width.saturating_sub(used).saturating_sub(2);

        for i in start..end {
            let Some(row) = display.get(i).and_then(|r| self.rows.get(*r)) else { continue };
            let is_sel = i == self.selected;
            // `theme.cursor` is `theme.fg("accent", "→ ")` (`theme/theme.ts:1313`); the unselected
            // prefix is two plain spaces (`:129`).
            let prefix = if is_sel { "→ " } else { "  " };
            let prefix_style = if is_sel { theme.accent_style() } else { theme.base_style() };
            // `:133` — pad the label to the column with `visibleWidth`-measured spaces. A label
            // wider than the clamp is NOT truncated upstream; `Math.max(0, …)` just pads by zero
            // and the row-level `truncateToWidth` at `:143` handles the overflow.
            let pad = label_w.saturating_sub(str_width(&row.label));
            let label_padded = format!("{}{}", row.label, " ".repeat(pad));
            // `getSettingsListTheme` (`theme/theme.ts:1310-1311`): the label is accent when
            // selected and otherwise UNSTYLED; the value is accent when selected and muted
            // otherwise.
            let label_style = if is_sel { theme.accent_style() } else { theme.base_style() };
            let value_style = if is_sel { theme.accent_style() } else { theme.muted_style() };
            // `:141` truncates the value with an EMPTY ellipsis; `:143` truncates the whole row
            // with the default `"..."`.
            let value = truncate_to_width(&row.value, value_max, "");
            let row_line = Line::from(vec![
                Span::styled(prefix.to_string(), prefix_style),
                Span::styled(label_padded, label_style),
                // The separator is a bare string concatenated between two themed runs (`:143`), so
                // it carries no colour of its own.
                Span::styled(LABEL_VALUE_SEPARATOR, theme.base_style()),
                Span::styled(value, value_style),
            ]);
            lines.push(truncate_line_to_width(row_line, width, "..."));
        }

        // `:146-150` — the scroll readout, `theme.hint` = dim, truncated to `width - 2` with an
        // empty ellipsis. Counts the FILTERED rows, and the numerator is `selectedIndex + 1`.
        if start > 0 || end < total {
            let text = format!("  ({}/{})", self.selected.saturating_add(1), total);
            lines.push(Line::from(Span::styled(
                truncate_to_width(&text, width.saturating_sub(2), ""),
                theme.dim_style(),
            )));
        }

        // `:152-160` — a blank, then the highlighted row's description wrapped at `width - 4`,
        // every wrapped row prefixed `"  "` and coloured `theme.description` = dim
        // (`theme/theme.ts:1312`). The `"  "` is applied AFTER wrapping, exactly as upstream does.
        if let Some(desc) = self.current().and_then(|r| r.description.as_deref())
            && !desc.is_empty()
        {
            lines.push(Line::from(""));
            let src = Line::from(Span::styled(desc.to_string(), theme.dim_style()));
            for wrapped in crate::transcript::wrap_line(&src, width.saturating_sub(4)) {
                let mut spans = vec![Span::styled("  ", theme.dim_style())];
                spans.extend(wrapped.spans);
                lines.push(Line::from(spans));
            }
        }

        // `:163` — the hint, always last.
        self.push_hint_line(&mut lines, width, theme);
        lines
    }

    /// `addHintLine` (`settings-list.ts:236-248`): a blank, then the dim hint truncated to `width`.
    fn push_hint_line(&self, lines: &mut Vec<Line<'static>>, width: usize, theme: &UiTheme) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            truncate_to_width(SETTINGS_HINT, width, "..."),
            theme.dim_style(),
        )));
    }

    /// `activateItem` (`settings-list.ts:198-220`) restricted to the cycle arm: advance the
    /// highlighted row's value and return the `"id\u{1f}value"` payload to persist (`None` for a
    /// row with no `values`).
    fn cycle_current(&mut self) -> Option<String> {
        let idx = self.selected_row_index()?;
        let row = self.rows.get_mut(idx)?;
        if row.cycle.is_empty() {
            return None;
        }
        let pos = row.cycle.iter().position(|v| *v == row.value).unwrap_or(0);
        let next = (pos + 1) % row.cycle.len();
        row.value = row.cycle.get(next).cloned().unwrap_or_default();
        Some(format!("{}{}{}", row.id, FIELD_SEP, row.value))
    }

    /// `tui.select.up`/`down` (`settings-list.ts:179-184`) — both **wrap**, and both no-op on an
    /// empty display list.
    fn select_up(&mut self) {
        let len = self.filtered.len();
        if len == 0 {
            return;
        }
        self.selected = if self.selected == 0 { len - 1 } else { self.selected - 1 };
    }

    fn select_down(&mut self) {
        let len = self.filtered.len();
        if len == 0 {
            return;
        }
        self.selected = if self.selected + 1 >= len { 0 } else { self.selected + 1 };
    }
}

impl Selector for SettingsSelector {
    fn desired_height(&self, width: u16) -> u16 {
        // `DynamicBorder`(:765) + the `SettingsList`'s own natural lines + `DynamicBorder`(:874).
        // Measured from the real `lines()` so the height can never disagree with the render.
        let body =
            self.lines(width, UiTheme::default_ref()).len().min(usize::from(u16::MAX)) as u16;
        body.saturating_add(2)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // L4/SYS-3 — **no envelope spacers here, and that is upstream-correct.** The audit lists
        // this site among the six that need `Spacer(1)` rows; the source disagrees.
        // `SettingsSelectorComponent`'s constructor adds exactly three children —
        // `new DynamicBorder()` (`settings-selector.ts:765`), the `SettingsList` (`:873`) and
        // `new DynamicBorder()` (`:874`) — with no `Spacer` and no title `Text` anywhere.
        //
        // The blanks the dialog *does* show all belong to `SettingsList.renderMainList`
        // (`packages/tui/src/components/settings-list.ts:90-166`): one under the search box
        // (`:95`), one above the description block (`:155`) and one above the hint (`:237`). They
        // are part of the body, not the envelope, which is why they are built in `lines`.
        //
        // `stack_rows` fills the regions from the TOP and starves the trailing ones — a prefix of the
        // natural render, which is what pi's layout engine does to an over-tall
        // `Container` (`packages/tui/src/layout.ts:113,307-310`), so the surviving row on a one-row
        // slot is the top `DynamicBorder` — upstream's own first child.
        let lines = self.lines(area.width, theme);
        let body_h = lines.len().min(usize::from(u16::MAX)) as u16;
        let [top, body, bottom] = stack_rows(area, [1, body_h, 1]);
        frame.render_widget(border_rule(top.width, theme), top);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), body);
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // `handleInput` (`settings-list.ts:168-196`) tests the bindings first and only then hands
        // the key to the search `Input`.
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.select_up();
                return SelectorOutcome::Redraw;
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                self.select_down();
                return SelectorOutcome::Redraw;
            }
            // Enter on a submenu row opens the nested picker (Pi `SettingItem.submenu`); otherwise it
            // cycles the value in place and applies it live (Pi `activateItem` → `onChange`), the
            // slot staying open.
            Some(SelectAction::Confirm) => {
                if let Some(id) = self.current().and_then(|r| r.submenu.clone()) {
                    return SelectorOutcome::OpenSubmenu(id);
                }
                return match self.cycle_current() {
                    Some(payload) => SelectorOutcome::Apply(payload),
                    None => SelectorOutcome::Redraw,
                };
            }
            Some(SelectAction::Cancel) => return SelectorOutcome::Cancel,
            None => {}
        }

        // Everything else drives the search box (`:192-195`).
        //
        // There is deliberately NO `if key.modifiers.intersects(CONTROL|ALT|SUPER) { Ignored }`
        // guard any more: upstream hands every unclaimed key straight to the search `Input`
        // (`settings-list.ts:192-195`), and the `Input` rejects control characters itself
        // (`input.ts:202-210`) — which is where that rejection now lives in cyrup too. The guard
        // made Ctrl+W / Ctrl+U / Ctrl+K / Alt+B / Alt+F / Alt+D unreachable in this dialog.
        match key.code {
            // `:187` — Space activates the row ONLY while the search box is empty; otherwise it is
            // a literal space typed into the query.
            KeyCode::Char(' ')
                if self.input.value().is_empty()
                    && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(id) = self.current().and_then(|r| r.submenu.clone()) {
                    return SelectorOutcome::OpenSubmenu(id);
                }
                match self.cycle_current() {
                    Some(payload) => SelectorOutcome::Apply(payload),
                    None => SelectorOutcome::Redraw,
                }
            }
            _ => match self.input.handle_key(key) {
                crate::text_input::InputOutcome::Edited => {
                    self.apply_filter();
                    SelectorOutcome::Redraw
                }
                crate::text_input::InputOutcome::Moved => SelectorOutcome::Redraw,
                crate::text_input::InputOutcome::Ignored => SelectorOutcome::Ignored,
            },
        }
    }

    fn set_editor_keymap(&mut self, keymap: &crate::keymap::EditorKeymap) {
        self.input.set_editor_keymap(keymap);
    }

    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        self.input.paste(text);
        self.apply_filter();
        SelectorOutcome::Redraw
    }

    fn as_settings_mut(&mut self) -> Option<&mut SettingsSelector> {
        Some(self)
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
    /// The live selector bindings, so the hint row names the keys the user actually has bound
    /// (`keyHint` → `keyText` → `getKeybindings().getKeys(...)`, `keybinding-hints.ts:34-44`).
    /// Defaults to the stock table and is refreshed from whatever keymap actually routed a key, the
    /// same way [`crate::selector::ListSelector`] does it.
    keymap: SelectKeymap,
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
            keymap: SelectKeymap::default(),
        }
    }

    /// Adopt the app's merged `tui.select.*` table so the hint row names the user's real bindings
    /// (Pi resolves `keyHint` through `getKeybindings()` on every render, `keybinding-hints.ts:34-44`).
    #[must_use]
    pub fn with_hints(mut self, keymap: &SelectKeymap) -> Self {
        self.keymap = keymap.clone();
        self
    }

    /// Record which option the persisted decision corresponds to, so it carries the ` ✓`
    /// saved-decision marker (S20 — Pi `isSavedOption` → `checkmark`, `trust-selector.ts:109-110`).
    /// `None` (the default) means no saved decision and draws no marker anywhere.
    ///
    /// Additive rather than a `new` parameter: both callers already compute this predicate to
    /// derive the preselected index (`app.rs` `/trust`, `cyrup/src/startup_ui.rs`
    /// `trust_selected_index`), but they collapse "no match" onto index 0, which cannot be
    /// distinguished afterwards.
    #[must_use]
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
                // `new Text(`${prefix}${label}${checkmark}`, 1, 0)` (`trust-selector.ts:113`) —
                // `paddingX = 1`, so the option rows carry the SAME one-column left margin the
                // header and hint rows already do (`text.ts:70-76`). cyrup started them at column 0,
                // leaving the cursor hanging one column left of every other row in the dialog.
                let mut spans = vec![
                    Span::raw(" "),
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

    /// The `/trust` keyboard-hint row — `trust-selector.ts:74-83`, quoted whole:
    ///
    /// ```text
    /// rawKeyHint("↑↓", "navigate") +
    ///     "  " +
    ///     keyHint("tui.select.confirm", "save") +
    ///     "  " +
    ///     keyHint("tui.select.cancel", "cancel"),
    /// ```
    ///
    /// wrapped in `new Text(…, 1, 0)` (`:75`), whose `paddingX = 1` prefixes exactly one space
    /// (`text.ts:70-76`).
    ///
    /// **S34.** Two separator defects at once: the join is the literal two-space string `"  "`, not
    /// `" · "`, and the arrows are `rawKeyHint("↑↓", …)` — `formatKeyText` splits on `/` and `+`,
    /// finds neither in `"↑↓"`, and joins back to `"↑↓"` (`keybinding-hints.ts:17-27`), so there is
    /// no slash between them. cyrup drew `" ↑/↓ navigate · enter save · esc cancel"`.
    ///
    /// **S4.** Each pair is two-tone — `theme.fg("dim", keyText(kb)) + theme.fg("muted", `
    /// ${description}`)` (`keybinding-hints.ts:42-44`) — via [`crate::chrome::key_hint_spans`], not
    /// one flat `dim` run across the whole row. Keys come from [`SelectKeymap::keys_label`], which
    /// joins **all** bound keys with `/` exactly as `keyText` does (`:29-36`), so the stock cancel
    /// hint reads `escape/ctrl+c cancel`.
    ///
    /// Note this is `ListSelector`'s row with **"save"** in place of "select"; it is composed here
    /// rather than shared because upstream composes it per component and only two components build
    /// such a row at all (see [`crate::selector::SelectorKind::draws_hint_row`]).
    fn hint_line(&self, theme: &UiTheme) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(crate::chrome::key_hint_spans("↑↓", "navigate", theme));
        if let Some(keys) = self.keymap.keys_label(SelectAction::Confirm) {
            spans.push(Span::raw("  "));
            spans.extend(crate::chrome::key_hint_spans(&keys, "save", theme));
        }
        if let Some(keys) = self.keymap.keys_label(SelectAction::Cancel) {
            spans.push(Span::raw("  "));
            spans.extend(crate::chrome::key_hint_spans(&keys, "cancel", theme));
        }
        Line::from(spans)
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
        // Unconditional, because upstream's `Spacer` children are — a `Paragraph` draws
        // `lines[0..area.height]` and drops the TRAILING rows, so a short slot shows a strict
        // PREFIX of this vector, matching pi's layout engine (see `crate::selector::stack_rows`).
        let blank = || Line::from("");
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(border_rule_line(area.width, theme));
        lines.push(blank());
        lines.extend(self.header_lines(theme));
        lines.push(blank());
        lines.extend(self.option_lines(theme));
        lines.push(blank());
        lines.push(self.hint_line(theme));
        lines.push(blank());
        lines.push(border_rule_line(area.width, theme));
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Keep the hint row honest even for a selector built without `with_hints`: adopt whatever
        // table actually routed this key (same discipline as `ListSelector::handle`).
        self.keymap = keymap.clone();
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
