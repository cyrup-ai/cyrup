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

use crate::keymap::{ModelsAction, ModelsKeymap, SelectAction, SelectKeymap};
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
    /// Model picker (`/model`, `model-selector.ts`) — rows sourced from the model catalog (L5).
    Model,
    /// Settings menu (`/settings`, `settings-selector.ts`).
    Settings,
    /// Scoped-models enable/order picker (`/scoped-models`, `scoped-models-selector.ts`).
    ScopedModels,
    /// Resume-session picker (`/resume`, `session-selector.ts`).
    Session,
    /// Session-tree navigator (`/tree`, `tree-selector.ts`).
    Tree,
    /// Project-trust picker (`/trust`, `trust-selector.ts`).
    Trust,
    /// Fork-from-message picker (`/fork`, `user-message-selector.ts`).
    UserMessage,
    /// Provider login picker (`/login`, `oauth-selector.ts`).
    Login,
    /// Provider logout picker (`/logout`, `oauth-selector.ts`).
    Logout,
}

impl SelectorKind {
    /// Whether confirming this selector applies in-crate (theme/thinking/show-images) or hands the
    /// chosen value to the run loop as an [`crate::app::AppCommand`] (the data-bound selectors, whose
    /// effect — set model, switch branch, login — lives at the session layer).
    pub fn is_data_bound(self) -> bool {
        !matches!(self, SelectorKind::Thinking | SelectorKind::ShowImages | SelectorKind::Theme)
    }

    /// The selector's title shown above the list (`*-selector.ts` headers).
    pub fn title(self) -> &'static str {
        match self {
            SelectorKind::Thinking => "Thinking Level",
            SelectorKind::ShowImages => "Show Images",
            SelectorKind::Theme => "Theme",
            SelectorKind::Model => "Select Model",
            SelectorKind::Settings => "Settings",
            SelectorKind::ScopedModels => "Scoped Models",
            SelectorKind::Session => "Resume Session",
            SelectorKind::Tree => "Session Tree",
            SelectorKind::Trust => "Project Trust",
            SelectorKind::UserMessage => "Fork from Message",
            SelectorKind::Login => "Login",
            SelectorKind::Logout => "Logout",
        }
    }
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
    /// A setting was changed **in place** without closing the slot (Pi's settings/config selectors
    /// apply each cycle live via `onChange` and stay open — settings-list.ts `cycleValue`). Carries an
    /// `"id\u{1f}value"` payload the chrome persists; the selector remains open and redraws.
    Apply(String),
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
    /// An optional bold title rendered between the top rule and the list (`*-selector.ts` headers).
    title: Option<String>,
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
        ListSelector { list, values, preview, title: None }
    }

    /// A data-bound selector (`model`/`session`/`tree`/… — `*-selector.ts`): build the windowed list
    /// from `(value, label, description)` rows sourced from an L5 service (model catalog, session list,
    /// branch tree), with a bold `title` header and a `no_match` empty-state line. Confirming yields the
    /// row's `value` for the run loop to apply (set model, switch branch, login…). `maxVisible = 10`
    /// matches the data selectors (`model-selector.ts:244`, `session-selector.ts`).
    pub fn data(
        kind: SelectorKind,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
    ) -> Self {
        let empty = format!("No {} available", kind.title().to_lowercase());
        let mut values = Vec::with_capacity(rows.len());
        let mut items = Vec::with_capacity(rows.len());
        for (value, label, desc) in rows {
            values.push(value);
            items.push(SelectItem::new(label, desc));
        }
        let mut list = SelectList::new(items, ColumnLayout::SLASH).with_no_match(empty);
        list.set_max_visible(10);
        list.set_selected(selected);
        ListSelector { list, values, preview: false, title: Some(kind.title().to_string()) }
    }

    /// A generic titled prompt (Pi `showStartupSelector`, startup-ui.ts:134-163): the pre-launch
    /// Continue/Cancel-style selector the bin mounts before the agent runtime is built (e.g. the
    /// missing-session-cwd prompt). Rows are `(value, label, description)`; confirming yields the
    /// highlighted row's value. `selected` preselects a row; `maxVisible` is the row count.
    pub fn prompt(title: String, rows: Vec<(String, String, Option<String>)>, selected: usize) -> Self {
        let count = rows.len().clamp(1, u16::MAX as usize) as u16;
        let mut selector = ListSelector::new(rows, count, selected, false);
        selector.title = Some(title);
        selector
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
        // Top `DynamicBorder` + optional title + list body + bottom `DynamicBorder` (spec/tui/05 §3).
        let title_h = u16::from(self.title.is_some());
        self.list.rendered_height().saturating_add(2).saturating_add(title_h)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let title_h = u16::from(self.title.is_some());
        let [top, title_area, body, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(title_h),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(border_rule(top.width, theme), top);
        if let Some(title) = &self.title {
            let styled = Span::styled(
                format!(" {title}"),
                theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD),
            );
            frame.render_widget(Paragraph::new(Line::from(styled)), title_area);
        }
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

/// The sentinel a [`CheckboxSelector`] confirm carries when **all** models are enabled (`enabledIds =
/// null`, `scoped-models-selector.ts:18`), distinct from an explicit ordered list. The run loop maps
/// this to "scope = full catalog".
pub const SCOPED_MODELS_ALL: &str = "*";

/// One catalog model in the scoped-models picker.
#[derive(Clone, Debug)]
struct ModelRow {
    /// The model id (the `enabledIds` element + confirm value).
    id: String,
    /// Display label (model name).
    label: String,
    /// Provider id (drives `toggleProvider`).
    provider: String,
    /// Secondary text (provider/desc shown in the right column).
    desc: Option<String>,
}

/// The scoped-models checkbox + reorder selector (`scoped-models-selector.ts`, spec/tui/05 §6). Unlike
/// the plain [`ListSelector`], this renders the **full catalog** with per-row enable checkboxes
/// (`✓`/`✗`), `Enter` **toggles** membership (it does *not* confirm), Alt+Up/Down **reorder** an
/// enabled model in cycle order, Ctrl+A/Ctrl+X enable/clear all, Ctrl+P toggles a whole provider, and
/// **Ctrl+S** confirms+persists. The `enabled` set mirrors Pi's `EnabledIds` (`None` = all enabled).
pub struct CheckboxSelector {
    rows: Vec<ModelRow>,
    /// `None` = all enabled (no filter); `Some(ordered ids)` = the explicit cycle set, in order.
    enabled: Option<Vec<String>>,
    /// The rendered list (rebuilt from `rows` + `enabled` on every state change to refresh markers).
    list: SelectList,
    /// The scoped-models bespoke bindings (Alt+Up/Down, Ctrl+A/X/P/S).
    models_keymap: ModelsKeymap,
}

impl CheckboxSelector {
    /// Build from the full catalog `(id, label, provider, desc)` rows and the current scoped set
    /// (`None` = all enabled). The highlight preselects the first row.
    pub fn scoped_models(
        catalog: Vec<(String, String, String, Option<String>)>,
        enabled: Option<Vec<String>>,
    ) -> Self {
        let rows: Vec<ModelRow> = catalog
            .into_iter()
            .map(|(id, label, provider, desc)| ModelRow { id, label, provider, desc })
            .collect();
        let mut sel = CheckboxSelector {
            rows,
            enabled,
            list: SelectList::new(Vec::new(), ColumnLayout::SLASH),
            models_keymap: ModelsKeymap::default(),
        };
        sel.refresh();
        sel.list.set_max_visible(10);
        sel
    }

    /// Override the scoped-models bindings (JSON-configured `app.models.*`).
    pub fn set_models_keymap(&mut self, keymap: ModelsKeymap) {
        self.models_keymap = keymap;
    }

    /// `true` when model `id` is in the scoped set (`isEnabled`, `scoped-models-selector.ts:21`).
    fn is_enabled(&self, id: &str) -> bool {
        match &self.enabled {
            None => true,
            Some(list) => list.iter().any(|e| e == id),
        }
    }

    /// The current scoped set: `None` = all enabled, else the explicit ordered ids
    /// (test/inspection + confirm sourcing).
    pub fn enabled_ids(&self) -> Option<&[String]> {
        self.enabled.as_deref()
    }

    /// Rebuild the rendered list from `rows` + `enabled`, refreshing the `✓`/`✗` markers while
    /// preserving the highlight. When **all** are enabled (`None`) no marker is drawn, matching Pi
    /// (`allEnabled ? "" : ✓/✗`, `:221`).
    fn refresh(&mut self) {
        let selected = self.list.selected();
        let all = self.enabled.is_none();
        let mut items = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let label = if all {
                row.label.clone()
            } else if self.is_enabled(&row.id) {
                format!("✓ {}", row.label)
            } else {
                format!("✗ {}", row.label)
            };
            items.push(SelectItem::new(label, row.desc.clone()));
        }
        let mut list = SelectList::new(items, ColumnLayout::SLASH).with_no_match("No models");
        list.set_max_visible(10);
        list.set_selected(selected.min(self.rows.len().saturating_sub(1)));
        self.list = list;
    }

    /// The highlighted model id, if any.
    fn current_id(&self) -> Option<String> {
        self.rows.get(self.list.selected()).map(|r| r.id.clone())
    }

    /// Toggle membership of `id` (`toggle`, `:25-31`): from "all" the first toggle starts a set with
    /// only `id`; a member is removed; a non-member is appended.
    fn toggle(&mut self, id: &str) {
        self.enabled = match self.enabled.take() {
            None => Some(vec![id.to_string()]),
            Some(mut list) => {
                if let Some(pos) = list.iter().position(|e| e == id) {
                    list.remove(pos);
                } else {
                    list.push(id.to_string());
                }
                Some(list)
            }
        };
    }

    /// Move `id` by `delta` within the enabled order (`move`, `:50-60`). No-op when all-enabled or
    /// `id` is not a member / would move out of bounds.
    fn reorder(&mut self, id: &str, delta: isize) {
        let Some(list) = self.enabled.as_mut() else { return };
        let Some(idx) = list.iter().position(|e| e == id) else { return };
        let new = idx as isize + delta;
        if new < 0 || new as usize >= list.len() {
            return;
        }
        list.swap(idx, new as usize);
    }

    /// Enable/clear every model of `id`'s provider (`toggleProvider`, `:311-323`): clear them if all
    /// are already enabled, else enable them all.
    fn toggle_provider(&mut self, id: &str) {
        let Some(provider) = self.rows.iter().find(|r| r.id == id).map(|r| r.provider.clone()) else {
            return;
        };
        let provider_ids: Vec<String> =
            self.rows.iter().filter(|r| r.provider == provider).map(|r| r.id.clone()).collect();
        let all_on = provider_ids.iter().all(|pid| self.is_enabled(pid));
        // Materialize the current set as an explicit list, then add/remove the provider's ids.
        let mut list: Vec<String> = match &self.enabled {
            None => self.rows.iter().map(|r| r.id.clone()).collect(),
            Some(l) => l.clone(),
        };
        if all_on {
            list.retain(|e| !provider_ids.contains(e));
        } else {
            for pid in provider_ids {
                if !list.contains(&pid) {
                    list.push(pid);
                }
            }
        }
        // Collapse back to "all" when every catalog model ended up enabled (Pi's null normalization).
        self.enabled = if list.len() == self.rows.len() { None } else { Some(list) };
    }

    /// The confirm value: [`SCOPED_MODELS_ALL`] when all are enabled, else the ordered ids joined by
    /// `\n` (the run loop splits this to rebuild the scoped set).
    fn confirm_value(&self) -> String {
        match &self.enabled {
            None => SCOPED_MODELS_ALL.to_string(),
            Some(list) => list.join("\n"),
        }
    }
}

impl Selector for CheckboxSelector {
    fn desired_height(&self, _width: u16) -> u16 {
        // Top rule + title + list body + footer-hint row + bottom rule.
        self.list.rendered_height().saturating_add(4)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let [top, title_area, body, hint, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(border_rule(top.width, theme), top);
        let title = Span::styled(
            " Scoped Models",
            theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD),
        );
        frame.render_widget(Paragraph::new(Line::from(title)), title_area);
        let lines = self.list.lines(body.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), body);
        // Footer hint (`getFooterText`, `:166-174`): the bespoke action keys.
        let hint_text = " enter toggle · alt+↑/↓ reorder · ctrl+a all · ctrl+x clear · ctrl+s save";
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(hint_text, theme.dim_style()))),
            hint,
        );
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Bespoke scoped-models bindings take precedence over the shared select map.
        if let Some(action) = self.models_keymap.action_for(key) {
            let Some(id) = self.current_id() else { return SelectorOutcome::Redraw };
            match action {
                ModelsAction::ReorderUp => {
                    self.reorder(&id, -1);
                    self.refresh();
                }
                ModelsAction::ReorderDown => {
                    self.reorder(&id, 1);
                    self.refresh();
                }
                ModelsAction::EnableAll => {
                    self.enabled = None;
                    self.refresh();
                }
                ModelsAction::ClearAll => {
                    self.enabled = Some(Vec::new());
                    self.refresh();
                }
                ModelsAction::ToggleProvider => {
                    self.toggle_provider(&id);
                    self.refresh();
                }
                ModelsAction::Save => return SelectorOutcome::Confirm(self.confirm_value()),
            }
            return SelectorOutcome::Redraw;
        }
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.list.select_up();
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                self.list.select_down();
                SelectorOutcome::Redraw
            }
            // Enter TOGGLES membership (it does NOT confirm) — `:278-289`.
            Some(SelectAction::Confirm) => {
                if let Some(id) = self.current_id() {
                    self.toggle(&id);
                    self.refresh();
                }
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => SelectorOutcome::Ignored,
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
