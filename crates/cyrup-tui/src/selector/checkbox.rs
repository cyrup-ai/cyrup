use super::*;

use crate::text_input::{Input, InputOutcome};

/// The sentinel a [`CheckboxSelector`] confirm carries when **all** models are enabled (`enabledIds =
/// null`, `scoped-models-selector.ts:18`), distinct from an explicit ordered list. The run loop maps
/// this to "scope = full catalog".
pub const SCOPED_MODELS_ALL: &str = "*";

/// One catalog model in the scoped-models picker — upstream's `modelsById` entry.
#[derive(Clone, Debug)]
struct ModelRow {
    /// The model id (the `enabledIds` element, the confirm value **and** the row's primary text —
    /// `item.model?.id`, `scoped-models-selector.ts:249`).
    id: String,
    /// The model *name*, shown only in the `Model Name:` row (`:274`), never in the list rows.
    label: String,
    /// Provider id — the ` [provider]` badge (`:251`) and the `toggleProvider` grouping.
    provider: String,
}

/// The scoped-models checkbox + reorder selector (`scoped-models-selector.ts`, spec/tui/05 §6). Unlike
/// the plain [`ListSelector`], this renders the **full catalog** with per-row enable markers
/// (`✓`/`✗`), `Enter` **toggles** membership (it does *not* confirm), Alt+Up/Down **reorder** an
/// enabled model in cycle order, Ctrl+A/Ctrl+X enable/clear all, Ctrl+P toggles a whole provider, and
/// **Ctrl+S** confirms+persists. The `enabled` set mirrors Pi's `EnabledIds` (`None` = all enabled).
///
/// **This component owns its own row rendering.** Upstream's `updateList` (`:230-280`) adds bare
/// `Text` children — `prefix + id + " [provider]" + status` — it does **not** drive a `SelectList`,
/// so nothing here goes through [`SelectList`]'s padded two-column
/// layout. That is what put the enable marker in front of the label and the provider in a
/// right-aligned description column (S6/S7).
pub struct CheckboxSelector {
    /// The catalog, in catalog order — upstream's `modelsById` + `allIds` (`:93-94`).
    rows: Vec<ModelRow>,
    /// `None` = all enabled (no filter); `Some(ordered ids)` = the explicit cycle set, in order.
    enabled: Option<Vec<String>>,
    /// Highlighted index into the *filtered item* list (`selectedIndex`, `:97`).
    selected: usize,
    /// `maxVisible` — **8** here (`scoped-models-selector.ts:112`), not the 10 `/model` uses.
    max_visible: usize,
    /// The scoped-models bespoke bindings (Alt+Up/Down, Ctrl+A/X/P/S).
    models_keymap: ModelsKeymap,
    /// The shared `tui.select.*` bindings, so the footer can name the live confirm key
    /// (`keyText("tui.select.confirm")`, `:198`) instead of hardcoding `enter`.
    select_keymap: SelectKeymap,
    /// `isDirty` (`:113`): set by every mutation, cleared on save — drives the `(unsaved)` warning.
    dirty: bool,
    /// `config.refreshStatus` (`:149-152`): an optional `muted` `  {status}` row between the list
    /// spacer and the footer.
    refresh_status: Option<String>,
    /// The live search query — `this.searchInput` (`scoped-models-selector.ts:139`). **S5.** The
    /// shared single-line editing surface, so this box has the same word motion / kill ring / undo
    /// / paste as every other one.
    input: Input,
}

/// One built row — upstream's `ModelItem` (`scoped-models-selector.ts:68-72`). `model` is `None` for
/// an enabled id that is no longer in the catalog; upstream renders those ` [unavailable]` with a
/// dim `✗` (`:251`, `:258`) and counts them in the footer's `N unavailable`.
struct ModelItem {
    full_id: String,
    /// Index into [`CheckboxSelector::rows`], or `None` when the id is not in the catalog.
    model: Option<usize>,
    enabled: bool,
}

impl CheckboxSelector {
    /// Build from the full catalog `(id, name, provider, desc)` rows and the current scoped set
    /// (`None` = all enabled). The highlight preselects the first row.
    ///
    /// The fourth tuple element is **ignored**: upstream builds the row's badge from the model's own
    /// provider (`` ` [${item.model.provider}]` ``, `:251`), immediately after the id — there is no
    /// free-form description column in this component for it to land in.
    pub fn scoped_models(
        catalog: Vec<(String, String, String, Option<String>)>,
        enabled: Option<Vec<String>>,
    ) -> Self {
        let rows: Vec<ModelRow> = catalog
            .into_iter()
            .map(|(id, label, provider, _desc)| ModelRow {
                id,
                label,
                provider,
            })
            .collect();
        CheckboxSelector {
            rows,
            enabled,
            selected: 0,
            max_visible: 8,
            models_keymap: ModelsKeymap::default(),
            select_keymap: SelectKeymap::default(),
            dirty: false,
            refresh_status: None,
            input: Input::new(),
        }
    }

    /// The live search query (test/inspection) — `getSearchInput().getValue()`.
    pub fn query(&self) -> &str {
        self.input.value()
    }

    /// The number of rows surviving the query (test/inspection) — `this.filteredItems.length`.
    pub fn visible_len(&self) -> usize {
        self.items().len()
    }

    /// Override the scoped-models bindings (JSON-configured `app.models.*`).
    pub fn set_models_keymap(&mut self, keymap: ModelsKeymap) {
        self.models_keymap = keymap;
    }

    /// Adopt the live `tui.select.*` bindings so the footer names the user's confirm key
    /// (`keyText("tui.select.confirm")`, `:198`) rather than the stock `enter`.
    pub fn set_select_keymap(&mut self, keymap: SelectKeymap) {
        self.select_keymap = keymap;
    }

    /// Set the optional catalog-refresh status row (`config.refreshStatus`, `:149-152`;
    /// `setRefreshStatus`, `:178-180`). An empty message clears it.
    pub fn set_refresh_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.refresh_status = if message.is_empty() {
            None
        } else {
            Some(message)
        };
    }

    /// The fuzzy search text for one catalog row — `getModelSearchText({id, provider, name})`
    /// (`model-search.ts:16-19`), the same provider-first shape [`crate::model_selector`] uses so a
    /// `provider/id` query ranks the way it does in `/model`.
    fn search_text(row: &ModelRow) -> String {
        format!(
            "{p} {p}/{id} {p} {id} {name}",
            p = row.provider,
            id = row.id,
            name = row.label
        )
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

    /// `getSortedIds` (`:62-66`): the enabled ids **first, in cycle order**, then every remaining
    /// catalog id. This is why Alt+↑/↓ is visible at all — the reorder shows up as the row moving.
    /// An enabled id that is no longer in the catalog stays in the list (upstream's
    /// `[...enabledIds, ...]` does not filter) and renders as `[unavailable]`.
    fn sorted_ids(&self) -> Vec<String> {
        match &self.enabled {
            None => self.rows.iter().map(|r| r.id.clone()).collect(),
            Some(en) => {
                let mut out: Vec<String> = en.clone();
                out.extend(
                    self.rows
                        .iter()
                        .filter(|r| !en.iter().any(|e| e == &r.id))
                        .map(|r| r.id.clone()),
                );
                out
            }
        }
    }

    /// `buildItems` + `refresh`'s `fuzzyFilter` (`:182-188`, `:211-224`): the sorted items, narrowed
    /// by the live query. Upstream falls back to the bare `fullId` as search text for an
    /// unavailable model (`:215-219`), so those stay searchable by id.
    fn items(&self) -> Vec<ModelItem> {
        let all: Vec<ModelItem> = self
            .sorted_ids()
            .into_iter()
            .map(|id| ModelItem {
                model: self.rows.iter().position(|r| r.id == id),
                enabled: self.is_enabled(&id),
                full_id: id,
            })
            .collect();
        if self.input.value().is_empty() {
            return all;
        }
        let texts: Vec<String> = all
            .iter()
            .map(|it| {
                it.model
                    .and_then(|i| self.rows.get(i))
                    .map_or_else(|| it.full_id.clone(), Self::search_text)
            })
            .collect();
        let matched = crate::fuzzy::filter(&texts, self.input.value(), String::as_str);
        let mut out = Vec::with_capacity(matched.len());
        for m in matched {
            if let Some(it) = all.get(m.index) {
                out.push(ModelItem {
                    full_id: it.full_id.clone(),
                    model: it.model,
                    enabled: it.enabled,
                });
            }
        }
        out
    }

    /// The ids the bulk operations act on — `const targetIds = this.searchInput.getValue() ?
    /// this.filteredItems.map((i) => i.fullId) : undefined` (`scoped-models-selector.ts:334`, `:344`).
    /// `None` stands in for upstream's `undefined`, i.e. "no search active, act on the whole
    /// catalog"; a live query narrows the bulk operation to exactly the rows on screen.
    fn target_ids(&self) -> Option<Vec<String>> {
        if self.input.value().is_empty() {
            None
        } else {
            Some(self.items().into_iter().map(|it| it.full_id).collect())
        }
    }

    /// `this.allIds` (`scoped-models-selector.ts:94`, `:123`): the **catalog** ids in catalog order.
    /// Deliberately not [`Self::sorted_ids`] — that view leads with the enabled set and so carries
    /// ids that are no longer in the catalog, which would corrupt both the `enableAll` collapse test
    /// and `clearAll`'s "everything except the targets".
    fn all_ids(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.id.clone()).collect()
    }

    /// `enableAll` (`scoped-models-selector.ts:32-40`). `targets` is [`Self::target_ids`]: `None`
    /// means the whole catalog.
    ///
    /// Append-only over the existing order — upstream's `result.push` never sorts or rebuilds, so the
    /// user's hand-built cycle order survives a bulk enable. Collapsing to `None` ("all enabled")
    /// needs **both** halves of `:39`: the same length as the catalog *and* every entry in the
    /// catalog. The length alone is not enough, because an enabled-but-unavailable id (kept by
    /// [`Self::sorted_ids`], rendered `[unavailable]`) can pad the list to the catalog's length while
    /// a real catalog model is still disabled.
    fn enable_all(&mut self, targets: Option<&[String]>) {
        // `if (enabledIds === null) return null` (`:33`) — already all-enabled, nothing to do.
        let Some(list) = self.enabled.as_ref() else {
            return;
        };
        let all_ids = self.all_ids();
        let mut result = list.clone();
        for id in targets.unwrap_or(&all_ids) {
            if !result.iter().any(|e| e == id) {
                result.push(id.clone());
            }
        }
        let covers_catalog =
            result.len() == all_ids.len() && result.iter().all(|id| all_ids.contains(id));
        self.enabled = if covers_catalog { None } else { Some(result) };
    }

    /// `clearAll` (`scoped-models-selector.ts:42-47`). `targets` is [`Self::target_ids`]: `None`
    /// means "everything currently enabled".
    ///
    /// From all-enabled *with* targets the result is every catalog id **minus** the targets (`:44`) —
    /// a filtered clear disables only what the query matched, it does not wipe the set. Never
    /// collapses back to `None`: upstream's `clearAll` has no `null` return.
    fn clear_all(&mut self, targets: Option<&[String]>) {
        match self.enabled.take() {
            None => {
                self.enabled = Some(match targets {
                    Some(t) => self
                        .rows
                        .iter()
                        .map(|r| r.id.clone())
                        .filter(|id| !t.contains(id))
                        .collect(),
                    None => Vec::new(),
                });
            }
            Some(mut list) => {
                // `new Set(targetIds ?? enabledIds)` (`:45`): with no targets the target set *is* the
                // enabled set, so the filter empties it.
                match targets {
                    Some(t) => list.retain(|id| !t.contains(id)),
                    None => list.clear(),
                }
                self.enabled = Some(list);
            }
        }
    }

    /// Clamp the highlight to the filtered length — `refresh`'s
    /// `Math.min(selectedIndex, max(0, filteredItems.length - 1))` (`:221`).
    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.visible_len().saturating_sub(1));
    }

    /// The highlighted model id, if any.
    fn current_id(&self) -> Option<String> {
        self.items()
            .into_iter()
            .nth(self.selected)
            .map(|it| it.full_id)
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

    /// Move `id` by `delta` within the enabled order (`move`, `:50-60`). No-op (returning `false`)
    /// when all-enabled, when `id` is not a member, or when the move would leave the list — the
    /// three cases upstream's `:302-318` also treats as "nothing happened", so neither `isDirty` nor
    /// `selectedIndex` moves.
    fn reorder(&mut self, id: &str, delta: isize) -> bool {
        let Some(list) = self.enabled.as_mut() else {
            return false;
        };
        let Some(idx) = list.iter().position(|e| e == id) else {
            return false;
        };
        let new = idx as isize + delta;
        if new < 0 || new as usize >= list.len() {
            return false;
        }
        list.swap(idx, new as usize);
        true
    }

    /// Enable/clear every model of `id`'s provider (`toggleProvider`, `:354-368`): clear them if all
    /// are already enabled, else enable them all.
    fn toggle_provider(&mut self, id: &str) {
        let Some(provider) = self
            .rows
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.provider.clone())
        else {
            return;
        };
        let provider_ids: Vec<String> = self
            .rows
            .iter()
            .filter(|r| r.provider == provider)
            .map(|r| r.id.clone())
            .collect();
        let all_on = provider_ids.iter().all(|pid| self.is_enabled(pid));
        // `:356-362` — upstream routes the provider toggle through the very same two bulk helpers,
        // with the provider's ids as the target set.
        if all_on {
            self.clear_all(Some(&provider_ids));
        } else {
            self.enable_all(Some(&provider_ids));
        }
    }

    /// The confirm value: [`SCOPED_MODELS_ALL`] when all are enabled, else the ordered ids joined by
    /// `\n` (the run loop splits this to rebuild the scoped set).
    fn confirm_value(&self) -> String {
        match &self.enabled {
            None => SCOPED_MODELS_ALL.to_string(),
            Some(list) => list.join("\n"),
        }
    }

    /// The title row: `theme.fg("accent", theme.bold("Model Configuration"))` at `paddingX = 0`
    /// (`:132`). **S7** — not `" Scoped Models"`; upstream's text is `Model Configuration` and it
    /// carries no leading space.
    fn title_line(theme: &UiTheme) -> Line<'static> {
        Line::from(Span::styled(
            "Model Configuration",
            theme
                .accent_style()
                .add_modifier(ratatui::style::Modifier::BOLD),
        ))
    }

    /// The subtitle row (`:133-135`): `muted` `Session-only. {keyText("app.models.save")} to save to
    /// settings.` — the guidance that explains why Enter does not close the dialog.
    fn subtitle_line(&self, theme: &UiTheme) -> Line<'static> {
        let save = self
            .models_keymap
            .keys_label(ModelsAction::Save)
            .unwrap_or_default();
        Line::from(Span::styled(
            format!("Session-only. {save} to save to settings."),
            theme.muted_style(),
        ))
    }

    /// `getFooterText` (`:190-209`) — **S29**. Seven `·`-joined parts behind a two-space indent, the
    /// whole run `dim`, and when dirty a trailing space plus a `warning` `(unsaved)`.
    ///
    /// Every key comes from the live keymaps (`keyText`), never a literal: the `provider` toggle and
    /// the `N/M enabled` count were both missing entirely, and the indent was one column.
    fn footer_spans(&self, theme: &UiTheme) -> Vec<Span<'static>> {
        let k = |a: ModelsAction| self.models_keymap.keys_label(a).unwrap_or_default();
        let confirm = self
            .select_keymap
            .keys_label(SelectAction::Confirm)
            .unwrap_or_default();
        // `countText` (`:191-196`): `enabledCount` counts only ids still in the catalog, and the
        // rest are reported as `N unavailable`.
        let count_text = match &self.enabled {
            None => "all enabled".to_string(),
            Some(en) => {
                let enabled_count = en
                    .iter()
                    .filter(|id| self.rows.iter().any(|r| &&r.id == id))
                    .count();
                let unavailable = en.len().saturating_sub(enabled_count);
                let total = self.rows.len();
                if unavailable > 0 {
                    format!("{enabled_count}/{total} enabled · {unavailable} unavailable")
                } else {
                    format!("{enabled_count}/{total} enabled")
                }
            }
        };
        let parts = [
            format!("{confirm} toggle"),
            format!("{} all", k(ModelsAction::EnableAll)),
            format!("{} clear", k(ModelsAction::ClearAll)),
            format!("{} provider", k(ModelsAction::ToggleProvider)),
            format!(
                "{}/{} reorder",
                k(ModelsAction::ReorderUp),
                k(ModelsAction::ReorderDown)
            ),
            format!("{} save", k(ModelsAction::Save)),
            count_text,
        ];
        let joined = parts.join(" · ");
        if self.dirty {
            vec![
                Span::styled(format!("  {joined} "), theme.dim_style()),
                Span::styled("(unsaved)", theme.warning_style()),
            ]
        } else {
            vec![Span::styled(format!("  {joined}"), theme.dim_style())]
        }
    }

    /// `updateList` (`:230-280`) — the whole `listContainer`, in upstream's order.
    ///
    /// **S6.** The enable marker is *appended after* the id **and** the provider badge, and it is
    /// coloured: `theme.fg("success", " ✓")` / `theme.fg("dim", " ✗")` (`:252-258`). It is omitted
    /// entirely while every model is enabled (`allEnabled ? "" : …`). It was previously prepended
    /// into the label, uncoloured, which both shifted the id two columns right and lost the colour.
    ///
    /// **S7.** The provider is `theme.fg("muted", " [provider]")` immediately after the id (`:251`),
    /// and the highlighted model's *name* gets its own `Spacer(1)` + `  Model Name: …` row
    /// (`:269-279`) — the only place `label` is used.
    fn body_lines(&self, width: usize, theme: &UiTheme) -> Vec<Line<'static>> {
        let items = self.items();
        let mut lines: Vec<Line<'static>> = Vec::new();
        // `:233-236` — the empty case RETURNS, so no `Model Name:` row follows it.
        if items.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No matching models",
                theme.muted_style(),
            )));
            return lines;
        }
        let len = items.len();
        let (start, end) = centered_window(self.selected, len, self.max_visible);
        let all_enabled = self.enabled.is_none();
        for (i, item) in items.iter().enumerate().take(end).skip(start) {
            let is_sel = i == self.selected;
            let row = item.model.and_then(|idx| self.rows.get(idx));
            let id = row.map_or(item.full_id.as_str(), |r| r.id.as_str());
            let mut spans: Vec<Span<'static>> = Vec::new();
            // `prefix` (`:248`): the `→ ` is accent, the unselected `  ` is a plain two-space pad.
            if is_sel {
                spans.push(Span::styled("→ ", theme.accent_style()));
                spans.push(Span::styled(id.to_string(), theme.accent_style()));
            } else {
                spans.push(Span::styled("  ", theme.base_style()));
                spans.push(Span::styled(id.to_string(), theme.base_style()));
            }
            spans.push(Span::styled(
                row.map_or_else(
                    || " [unavailable]".to_string(),
                    |r| format!(" [{}]", r.provider),
                ),
                theme.muted_style(),
            ));
            match (row, all_enabled, item.enabled) {
                (None, _, _) => spans.push(Span::styled(" ✗", theme.dim_style())),
                (Some(_), true, _) => {}
                (Some(_), false, true) => spans.push(Span::styled(" ✓", theme.success_style())),
                (Some(_), false, false) => spans.push(Span::styled(" ✗", theme.dim_style())),
            }
            lines.extend(crate::transcript::text_lines_of(
                &Line::from(spans),
                width,
                0,
            ));
        }
        // Scroll indicator (`:263-267`).
        if start > 0 || end < len {
            lines.push(Line::from(Span::styled(
                format!("  ({}/{})", self.selected.saturating_add(1), len),
                theme.muted_style(),
            )));
        }
        // `Spacer(1)` + `  Model Name: {name}` for the highlighted item (`:269-279`).
        if let Some(item) = items.get(self.selected) {
            let text = match item.model.and_then(|idx| self.rows.get(idx)) {
                Some(r) => format!("  Model Name: {}", r.label),
                None => "  Model unavailable".to_string(),
            };
            lines.push(Line::from(""));
            lines.extend(crate::transcript::text_lines_of(
                &Line::from(Span::styled(text, theme.muted_style())),
                width,
                0,
            ));
        }
        lines
    }

    /// The complete natural render, top to bottom — the single source both
    /// [`Selector::desired_height`] and [`Selector::render`] read, so the measured height can never
    /// disagree with what is drawn.
    ///
    /// `ScopedModelsSelectorComponent`'s children (`scoped-models-selector.ts:130-156`):
    /// `DynamicBorder`(:130) · `Spacer`(:131) · title(:132) · subtitle(:133-135) · `Spacer`(:136) ·
    /// search `Input`(:140) · `Spacer`(:141) · listContainer(:145) · `Spacer`(:148) ·
    /// [refreshStatus(:150-151)] · footer(:154) · `DynamicBorder`(:156). **Four** spacers, and note
    /// this component — unlike `extension-selector.ts:74` — has NO spacer between its footer row and
    /// the bottom border.
    fn all_lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let w = usize::from(width);
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(border_rule_line(width, theme));
        lines.push(Line::from(""));
        lines.push(Self::title_line(theme));
        lines.extend(crate::transcript::text_lines_of(
            &self.subtitle_line(theme),
            w,
            0,
        ));
        lines.push(Line::from(""));
        // The `Input` is a bare container child (`:140`), so it renders at column 0 behind the
        // shared unstyled `"> "` prompt (S31, `input.ts:380`).
        lines.push(Line::from(input_line_spans(
            self.input.value(),
            self.input.cursor(),
            width,
            theme,
        )));
        lines.push(Line::from(""));
        lines.extend(self.body_lines(w, theme));
        lines.push(Line::from(""));
        if let Some(status) = &self.refresh_status {
            lines.extend(crate::transcript::text_lines_of(
                &Line::from(Span::styled(format!("  {status}"), theme.muted_style())),
                w,
                0,
            ));
        }
        lines.extend(crate::transcript::text_lines_of(
            &Line::from(self.footer_spans(theme)),
            w,
            0,
        ));
        lines.push(border_rule_line(width, theme));
        lines
    }
}

impl Selector for CheckboxSelector {
    fn desired_height(&self, width: u16) -> u16 {
        self.all_lines(width, UiTheme::default_ref())
            .len()
            .min(usize::from(u16::MAX)) as u16
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // A `Vec<Line>` handed to a `Paragraph` draws `lines[0..area.height]` and drops the
        // TRAILING rows, so an over-tall dialog renders a strict PREFIX of this vector — exactly
        // what pi's layout engine does (`packages/tui/src/layout.ts:113,307-310`); see
        // `stack_rows`' doc for the full argument.
        let lines = self.all_lines(area.width, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Bespoke scoped-models bindings take precedence over the shared select map.
        if let Some(action) = self.models_keymap.action_for(key) {
            match action {
                // `:300-319` — a successful move also advances the highlight so it tracks the model
                // that moved, and only a successful move sets `isDirty`.
                ModelsAction::ReorderUp => {
                    let Some(id) = self.current_id() else {
                        return SelectorOutcome::Redraw;
                    };
                    if self.reorder(&id, -1) {
                        self.selected = self.selected.saturating_sub(1);
                        self.dirty = true;
                    }
                }
                ModelsAction::ReorderDown => {
                    let Some(id) = self.current_id() else {
                        return SelectorOutcome::Redraw;
                    };
                    if self.reorder(&id, 1) {
                        self.selected = self.selected.saturating_add(1);
                        self.dirty = true;
                    }
                }
                // `:332-340` — "enable all (filtered if search active, otherwise all)". With no
                // query the targets are the whole catalog, so the collapse test in
                // [`Self::enable_all`] fires and this still lands on `enabled == None`.
                ModelsAction::EnableAll => {
                    let targets = self.target_ids();
                    self.enable_all(targets.as_deref());
                    self.dirty = true;
                }
                // `:342-350` — with no query the target set is the enabled set itself, so this
                // still empties it; with a query it disables only the matched rows.
                ModelsAction::ClearAll => {
                    let targets = self.target_ids();
                    self.clear_all(targets.as_deref());
                    self.dirty = true;
                }
                ModelsAction::ToggleProvider => {
                    // `:353-355` — upstream requires `item?.model`, i.e. an *available* model; an
                    // `[unavailable]` row has no provider to toggle. [`Self::toggle_provider`]
                    // returns early for an id that is not in the catalog.
                    let Some(id) = self.current_id() else {
                        return SelectorOutcome::Redraw;
                    };
                    self.toggle_provider(&id);
                    self.dirty = true;
                }
                ModelsAction::Save => {
                    self.dirty = false;
                    return SelectorOutcome::Confirm(self.confirm_value());
                }
            }
            self.clamp_selection();
            return SelectorOutcome::Redraw;
        }
        // "Ctrl+C - clear search or cancel if empty" (`scoped-models-selector.ts:378-387`), tested
        // by `matchesKey(data, Key.ctrl("c"))` — a LITERAL upstream, not a `tui.select.*` id, which
        // is why this arm has to sit ahead of the generic `Cancel` below: cyrup's stock
        // `tui.select.cancel` binds `esc` AND `ctrl+c`, so routing Ctrl+C through `action_for`
        // first would close the dialog on the press that upstream spends clearing the query. Escape
        // is unconditional (`:390-392`) and stays with `Cancel`.
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !self.input.value().is_empty()
        {
            self.input.clear();
            self.clamp_selection();
            return SelectorOutcome::Redraw;
        }
        match keymap.action_for(key) {
            // Up/Down WRAP (`:286-297`).
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                let len = self.visible_len();
                if len > 0 {
                    self.selected = if self.selected == 0 {
                        len.saturating_sub(1)
                    } else {
                        self.selected - 1
                    };
                }
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                let len = self.visible_len();
                if len > 0 {
                    self.selected = if self.selected.saturating_add(1) >= len {
                        0
                    } else {
                        self.selected + 1
                    };
                }
                SelectorOutcome::Redraw
            }
            // Enter TOGGLES membership (it does NOT confirm) — `:322-331`.
            Some(SelectAction::Confirm) => {
                if let Some(id) = self.current_id() {
                    self.toggle(&id);
                    self.dirty = true;
                    self.clamp_selection();
                }
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            // Everything else feeds the search `Input` (`:396-397`). The bespoke
            // [`ModelsKeymap`] block above deliberately stays AHEAD of this: `ctrl+w` is
            // `ModelsAction::Save` here, not `deleteWordBackward`.
            None => match self.input.handle_key(key) {
                InputOutcome::Edited => {
                    self.clamp_selection();
                    SelectorOutcome::Redraw
                }
                InputOutcome::Moved => SelectorOutcome::Redraw,
                InputOutcome::Ignored => SelectorOutcome::Ignored,
            },
        }
    }

    fn set_editor_keymap(&mut self, keymap: &crate::keymap::EditorKeymap) {
        self.input.set_editor_keymap(keymap);
    }

    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        self.input.paste(text);
        self.clamp_selection();
        SelectorOutcome::Redraw
    }
}
