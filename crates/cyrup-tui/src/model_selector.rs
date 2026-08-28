//! The `/model` model selector (spec/tui/05 §5.2, §6.2; port of Pi's
//! `components/model-selector.ts`). Unlike the bare titled [`ListSelector`](crate::selector::ListSelector)
//! the audit flagged (`/model` degraded to a plain list), this is the full picker: an embedded fuzzy
//! **search `Input`**, an `all | scoped` **scope** toggle (`⇥`), a `→` cursor + `[provider]` badge on
//! every row, a `✓` marker on the **active** model (sorted to the top), a windowed `(i/N)` scroll
//! indicator, and a `Model Name:` footer — exactly `model-selector.ts:229-283`.
//!
//! It occupies the editor slot like every other first-party selector (spec/tui/05 §1.1) and emits the
//! chosen model id via [`SelectorOutcome::Confirm`], which the chrome maps to
//! [`AppCommand::ConfirmSelection`](crate::app::AppCommand) → `set_model` at the session layer.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::fuzzy;
use crate::keymap::{EditorAction, EditorKeymap, SelectAction, SelectKeymap};
use crate::selector::{border_rule_line, centered_window, Selector, SelectorOutcome};
use crate::text_input::{Input, InputOutcome};
use crate::theme::UiTheme;
use crate::transcript::text_lines_of;

/// One catalog model offered by `/model`.
#[derive(Clone, Debug)]
pub struct ModelEntry {
    /// The model id (the confirm value + primary row text).
    pub id: String,
    /// The display name (shown in the `Model Name:` footer).
    pub name: String,
    /// The provider id (the `[provider]` badge + scope grouping).
    pub provider: String,
    /// Whether this is the currently-active model (sorted to top, `✓`-marked).
    pub current: bool,
    /// Whether this model is in the scoped set (drives the `scoped` scope filter).
    pub scoped: bool,
}

impl ModelEntry {
    /// The fuzzy search text (Pi `getModelSelectorSearchText`, `model-search.ts:16-19`): provider-first
    /// so exact provider-prefixed queries rank before proxy-provider ids.
    fn search_text(&self) -> String {
        format!(
            "{p} {p}/{id} {p} {id} {name}",
            p = self.provider,
            id = self.id,
            name = self.name
        )
    }
}

/// The model-list scope (`model-selector.ts:30`): the full catalog or just the scoped set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    All,
    Scoped,
}

/// The `/model` selector.
pub struct ModelSelector {
    /// The full catalog, current-first then provider-sorted (Pi `sortModels`, `:184-195`).
    models: Vec<ModelEntry>,
    /// Whether any scoped models exist (gates the `⇥` scope toggle + the scope header).
    has_scoped: bool,
    /// The PERSISTED default `(provider, id)` (`defaultProvider` + `defaultModel`), when one is set
    /// AND this picker was opened on a path that can write it.
    ///
    /// Drives all three of Pi's default behaviours: the ` · default` badge
    /// (`model-selector.ts:317`), sorting that row SECOND behind the current model (`:226-238`),
    /// and — because Pi guards its `Ctrl+S` on `onSelectAsDefaultCallback` being wired
    /// (`:401`, `:138`) — whether the `Ctrl+S` key and its footer hint exist at all.
    ///
    /// `Some(("", ""))` is therefore meaningful: "persisting is available, nothing is default yet".
    /// [`Self::with_default_model`] is the only way to set it.
    default_model: Option<(String, String)>,
    scope: Scope,
    /// The live fuzzy search query — pi's embedded `Input` (`model-selector.ts:117`), which is now
    /// literally the shared component and brings word motion / kill ring / undo / paste with it.
    input: Input,
    /// Highlighted index into the *filtered* list.
    selected: usize,
    max_visible: usize,
    /// `errorMessage` (`model-selector.ts:58`): a catalog-refresh failure. When set it **replaces**
    /// the `No matching models` / `Model Name:` block and every one of its `\n`-separated lines is
    /// drawn in `error` (`:299-304`).
    error_message: Option<String>,
    /// `refreshStatusMessage` + `refreshStatusSuccess` (`model-selector.ts:59-60`): the
    /// `Spacer(1)` + `  {message}` row drawn below the list, `success` when the refresh landed and
    /// `muted` while it is in flight (`:312-317`).
    ///
    /// **Empty by default, unlike upstream.** pi seeds `"Refreshing model catalogs…"` in the field
    /// initializer because its constructor kicks off `refreshModels()` (`:136`); cyrup is handed an
    /// already-resolved catalog by `App::open_model_selector`, so seeding the same string would
    /// pin a "Refreshing…" row that nothing ever clears. Drivers that do refresh in the background
    /// call [`Self::set_refresh_status`] / [`Self::set_error_message`], which reproduce upstream's
    /// rows exactly.
    refresh_status: Option<(String, bool)>,
    /// The label of the key bound to `tui.input.tab` (cyrup's `editor.tab`), for the scope hint row
    /// — Pi `keyHint("tui.input.tab", "scope")` reads the LIVE keymap (`:228-230`), never a glyph.
    scope_key: String,
}

impl ModelSelector {
    /// Build from the catalog `(id, name, provider, current, scoped)` rows. The list is sorted
    /// current-first then by provider (Pi `sortModels`); the scope defaults to `scoped` when any scoped
    /// models exist, else `all` (`model-selector.ts:82`).
    /// Enable Pi's second confirm key on this picker and tell it which row is the persisted
    /// default (`interactive-mode.ts:5000` passes `defaultProvider && defaultModel` into the
    /// component, and wires `onSelectAsDefault` at `:4999`).
    ///
    /// Pass `("", "")` when nothing is persisted yet — the binding and its footer still appear
    /// (Pi's guard is on the CALLBACK, not on a default existing), but no row is badged.
    /// Re-sorts so the default lands second, behind the current model.
    #[must_use]
    pub fn with_default_model(mut self, provider: &str, id: &str) -> Self {
        self.default_model = Some((provider.to_string(), id.to_string()));
        self.sort_models();
        self
    }

    /// True when `m` is the persisted default (Pi `isDefaultModel`, `model-selector.ts:252-254`).
    fn is_default(&self, m: &ModelEntry) -> bool {
        self.default_model
            .as_ref()
            .is_some_and(|(p, i)| *p == m.provider && *i == m.id)
    }

    /// Pi `sortModels` (`model-selector.ts:226-238`): current model first, **persisted default
    /// second**, then by provider. The default tier is why this is a method rather than the inline
    /// sort the constructor used to do — it has to re-run when the default is threaded in.
    fn sort_models(&mut self) {
        let default = self.default_model.clone();
        let is_default = |m: &ModelEntry| {
            default.as_ref().is_some_and(|(p, i)| *p == m.provider && *i == m.id)
        };
        self.models.sort_by(|a, b| match (a.current, b.current) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => match (is_default(a), is_default(b)) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.provider.cmp(&b.provider),
            },
        });
    }

    pub fn new(mut models: Vec<ModelEntry>) -> Self {
        models.sort_by(|a, b| match (a.current, b.current) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.provider.cmp(&b.provider),
        });
        let has_scoped = models.iter().any(|m| m.scoped);
        let scope = if has_scoped { Scope::Scoped } else { Scope::All };
        let mut sel = ModelSelector {
            models,
            has_scoped,
            default_model: None,
            scope,
            input: Input::new(),
            selected: 0,
            max_visible: 10,
            error_message: None,
            refresh_status: None,
            scope_key: EditorKeymap::default()
                .keys_label(EditorAction::Tab)
                .unwrap_or_else(|| "tab".to_string()),
        };
        // Preselect the current model within the initial (scope-filtered) view.
        sel.selected = sel.filtered().iter().position(|m| m.current).unwrap_or(0);
        sel
    }

    /// The models in the active scope (Pi `activeModels`, `:177`).
    fn active(&self) -> Vec<&ModelEntry> {
        match self.scope {
            Scope::All => self.models.iter().collect(),
            Scope::Scoped => self.models.iter().filter(|m| m.scoped).collect(),
        }
    }

    /// The active-scope models passing the fuzzy query (Pi `filterModels`, `:219-227`). The fuzzy score
    /// runs over each model's provider-first search text and reorders best-match-first.
    fn filtered(&self) -> Vec<&ModelEntry> {
        let active = self.active();
        if self.input.value().is_empty() {
            return active;
        }
        let texts: Vec<String> = active.iter().map(|m| m.search_text()).collect();
        let matches = fuzzy::filter(&texts, self.input.value(), String::as_str);
        matches.into_iter().filter_map(|mm| active.get(mm.index).copied()).collect()
    }

    /// The highlighted model, if any (test/inspection).
    pub fn current(&self) -> Option<&ModelEntry> {
        self.filtered().into_iter().nth(self.selected)
    }

    /// The number of currently-visible (filtered) rows (test/inspection).
    pub fn visible_len(&self) -> usize {
        self.filtered().len()
    }

    /// Whether the scope is currently `scoped` (test/inspection).
    pub fn is_scoped(&self) -> bool {
        self.scope == Scope::Scoped
    }

    /// Toggle the scope (Pi `⇥`, `:288-296`) — only when scoped models exist.
    fn toggle_scope(&mut self) {
        if !self.has_scoped {
            return;
        }
        self.scope = match self.scope {
            Scope::All => Scope::Scoped,
            Scope::Scoped => Scope::All,
        };
        self.selected = self.filtered().iter().position(|m| m.current).unwrap_or(0);
    }

    /// Pre-fill the search box (Pi `showModelSelector(initialSearchInput)` → `ModelSelectorComponent`'s
    /// `initialSearchInput`, interactive-mode.ts:4307,4333): seed the fuzzy query from `/model <text>`
    /// when no exact match set the model directly, so the picker opens already narrowed to `<text>`. The
    /// caret lands at the end of the seeded text and the highlight resets to the top of the filtered set.
    pub fn set_search(&mut self, term: String) {
        self.input = Input::with_value(term);
        self.selected = 0;
    }

    /// Set the catalog-refresh status row (Pi `refreshStatusMessage`/`refreshStatusSuccess`,
    /// `:172-184`): `success` colours it `success`, otherwise `muted`. An empty message clears it,
    /// exactly like upstream's `refreshStatusMessage = ""` (`:172`, `:191`).
    ///
    /// **BLOCKED, not unfinished.** Upstream's producer is `refreshModels()`
    /// (`model-selector.ts:162-200`), which the constructor fires at `:136` — so the user action
    /// that should reach this is *opening `/model`*. cyrup cannot run it from here: the driver of
    /// that refresh is `ModelRuntime.refresh`, and `AgentSession` exposes no analogue —
    /// `available_model_catalog()` is a snapshot, `AgentSessionServices`
    /// (`cyrup-session-svc/src/services.rs:84-152`) publishes `catalog_overlay` but not the live
    /// `cyrup_provider::Collection` that owns `Collection::refresh` (`collection.rs:317`), and the
    /// one refresh cyrup does run (`cyrup::provider::spawn_model_catalog_refresh`,
    /// `crates/cyrup/src/provider.rs:132`) is a detached bin-level task with no channel back into
    /// `App`. Unblocking it is a `cyrup-session-svc` API addition plus a `ModelRefreshMsg` channel
    /// alongside `install_login_channel` (`app.rs:2045`) — both outside this crate's edit scope.
    pub fn set_refresh_status(&mut self, message: impl Into<String>, success: bool) {
        let message = message.into();
        self.refresh_status = if message.is_empty() { None } else { Some((message, success)) };
    }

    /// Set (or clear) the catalog-refresh error (Pi `errorMessage`, `:174-194`). Every
    /// `\n`-separated line renders in `error` and the whole block replaces the `Model Name:` /
    /// `No matching models` row (`:299-311`).
    ///
    /// **BLOCKED** on the same missing seam as [`Self::set_refresh_status`] — every one of
    /// upstream's four assignments to `errorMessage` (`:174`, `:176`, `:178`, `:180`) is a branch
    /// on a `modelRuntime.refresh()` result.
    pub fn set_error_message(&mut self, message: Option<String>) {
        self.error_message = message.filter(|m| !m.is_empty());
    }

    /// The scope header line (`Scope: all | scoped`, Pi `getScopeText`, `:222-226`), or, when no
    /// providers/scoped models exist, the `warning` hint (Pi `:102-103`).
    ///
    /// Both are `new Text(…, 0, 0)` — `paddingX = 0` (`:97`, `:103`), so neither carries a leading
    /// space; they start at column 0, flush with the `  `-prefixed list rows below (S32).
    fn scope_line(&self, theme: &UiTheme) -> Line<'static> {
        if !self.has_scoped {
            return Line::from(Span::styled(
                "Only showing models from configured providers. Use /login to add providers.",
                theme.warning_style(),
            ));
        }
        let (all_style, scoped_style) = match self.scope {
            Scope::All => (theme.accent_style(), theme.muted_style()),
            Scope::Scoped => (theme.muted_style(), theme.accent_style()),
        };
        Line::from(vec![
            Span::styled("Scope: ", theme.muted_style()),
            Span::styled("all", all_style),
            Span::styled(" | ", theme.muted_style()),
            Span::styled("scoped", scoped_style),
        ])
    }

    /// The scope hint row (Pi `getScopeHintText`, `:228-230`), added as its **own** `Text` child
    /// (`:99-100`) directly under the scope line — and **only** when scoped models exist, since the
    /// `else` branch at `:101-104` adds the warning `Text` alone (S30).
    ///
    /// `keyHint(binding, description)` is `dim(keyText(binding)) + muted(" " + description)`
    /// (`keybinding-hints.ts:42-44`), and `getScopeHintText` appends a second `muted` run — so the
    /// row is two-tone: the key dim, `scope (all/scoped)` muted.
    fn scope_hint_line(&self, theme: &UiTheme) -> Line<'static> {
        Line::from(vec![
            Span::styled(self.scope_key.clone(), theme.dim_style()),
            Span::styled(" scope", theme.muted_style()),
            Span::styled(" (all/scoped)", theme.muted_style()),
        ])
    }

    /// The whole scope block: the scope line plus its own hint `Text` when scoped models exist
    /// (`:96-100`), else the lone warning row (`:101-104`).
    ///
    /// Routed through [`text_lines_of`] — the `Text.render` port — because these are `Text`
    /// children like any other and upstream wraps them at the dialog width (`text.ts:60-87`). The
    /// warning string is 75 columns and so wraps on any terminal narrower than that; it used to be
    /// truncated instead.
    fn scope_block_lines(&self, width: usize, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut out = text_lines_of(&self.scope_line(theme), width, 0);
        if self.has_scoped {
            out.extend(text_lines_of(&self.scope_hint_line(theme), width, 0));
        }
        out
    }

    /// The windowed list body (Pi `updateList`, `:257-318`): `→ ` cursor + `id` + `[provider]` badge +
    /// `✓` on the active model, a `(i/N)` scroll indicator, then — in upstream's exact order — the
    /// **error block** *or* `No matching models` *or* the `Model Name:` footer, and finally the
    /// refresh-status row.
    ///
    /// Note the three-way `if/else if/else` at `:299-311`: an `errorMessage` **replaces** both the
    /// empty-list message and the `Model Name:` block. The refresh-status row at `:312-317` is
    /// independent of all three and is emitted even when the filtered list is empty — which is why
    /// this no longer early-returns on an empty list the way it used to.
    fn body_lines(
        &self,
        filtered: &[&ModelEntry],
        width: usize,
        theme: &UiTheme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let len = filtered.len();
        let (start, end) = centered_window(self.selected, len, self.max_visible);
        for (i, m) in filtered.iter().enumerate().take(end).skip(start) {
            let is_sel = i == self.selected;
            let mut spans: Vec<Span<'static>> = Vec::new();
            if is_sel {
                spans.push(Span::styled("→ ", theme.accent_style()));
                spans.push(Span::styled(m.id.clone(), theme.accent_style()));
            } else {
                spans.push(Span::styled(format!("  {}", m.id), theme.base_style()));
            }
            spans.push(Span::styled(format!(" [{}]", m.provider), theme.muted_style()));
            // `const defaultBadge = isDefault ? theme.fg("muted", " · default") : ""`, drawn
            // AFTER the provider badge and BEFORE the `✓` (`model-selector.ts:317`, `:325`,
            // `:330`). The identical string the thinking picker uses (`thinking-selector.ts:73`).
            if self.is_default(m) {
                spans.push(Span::styled(" \u{b7} default".to_string(), theme.muted_style()));
            }
            if m.current {
                spans.push(Span::styled(" ✓".to_string(), theme.success_style()));
            }
            lines.push(Line::from(spans));
        }
        // Scroll indicator (Pi `:264-268`).
        if start > 0 || end < len {
            lines.push(Line::from(Span::styled(
                format!("  ({}/{})", self.selected + 1, len),
                theme.muted_style(),
            )));
        }
        // Error block / `No matching models` / `Model Name:` footer — Pi's three-way branch
        // (`:299-311`). The error block wins over both of the others.
        if let Some(err) = &self.error_message {
            // `errorMessage.split("\n")` → one `Text` per line, each `theme.fg("error", line)`
            // (`:301-304`). Each `Text` then wraps at the dialog width like any other.
            for logical in err.split('\n') {
                lines.extend(text_lines_of(
                    &Line::from(Span::styled(logical.to_string(), theme.error_style())),
                    width,
                    0,
                ));
            }
        } else if len == 0 {
            lines.push(Line::from(Span::styled("  No matching models", theme.muted_style())));
        } else if let Some(sel) = filtered.get(self.selected) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  Model Name: {}", sel.name),
                theme.muted_style(),
            )));
        }
        // Refresh-status row (`:312-317`): `Spacer(1)` then `  {message}`, `success` when the
        // refresh landed, `muted` while it is in flight. Independent of the branch above.
        if let Some((message, success)) = &self.refresh_status {
            lines.push(Line::from(""));
            let style = if *success { theme.success_style() } else { theme.muted_style() };
            lines.extend(text_lines_of(
                &Line::from(Span::styled(format!("  {message}"), style)),
                width,
                0,
            ));
        }
        lines
    }
}

/// Find an EXACT model reference match (Pi `findExactModelReferenceMatch`, core/model-resolver.ts:76):
/// a canonical `provider/id`, a `provider/id` split, or a bare `id`, matched case-insensitively.
/// Ambiguous matches (a bare id shared across providers, or a duplicate canonical) are REJECTED
/// (`None`), exactly like Pi. Used by `/model <text>` to set the model directly instead of opening the
/// picker when the argument names a single model unambiguously.
pub fn find_exact_model_reference_match<'a>(
    models: &'a [ModelEntry],
    reference: &str,
) -> Option<&'a ModelEntry> {
    let trimmed = reference.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.to_lowercase();

    // 1. Canonical `provider/id` (exactly one wins; >1 is ambiguous → reject).
    let mut canonical =
        models.iter().filter(|m| format!("{}/{}", m.provider, m.id).to_lowercase() == normalized);
    match (canonical.next(), canonical.next()) {
        (Some(m), None) => return Some(m),
        (Some(_), Some(_)) => return None,
        _ => {}
    }

    // 2. Split `provider/id` (exactly one wins; >1 is ambiguous → reject).
    if let Some((provider, model_id)) = trimmed.split_once('/') {
        let provider = provider.trim();
        let model_id = model_id.trim();
        if !provider.is_empty() && !model_id.is_empty() {
            let mut matches = models.iter().filter(|m| {
                m.provider.to_lowercase() == provider.to_lowercase()
                    && m.id.to_lowercase() == model_id.to_lowercase()
            });
            match (matches.next(), matches.next()) {
                (Some(m), None) => return Some(m),
                (Some(_), Some(_)) => return None,
                _ => {}
            }
        }
    }

    // 3. Bare `id` (exactly one wins; a cross-provider duplicate is ambiguous → reject).
    let mut id_matches = models.iter().filter(|m| m.id.to_lowercase() == normalized);
    match (id_matches.next(), id_matches.next()) {
        (Some(m), None) => Some(m),
        _ => None,
    }
}

impl Selector for ModelSelector {
    fn desired_height(&self, width: u16) -> u16 {
        let filtered = self.filtered();
        let body =
            self.body_lines(&filtered, usize::from(width), UiTheme::default_ref()).len() as u16;
        // top rule + blank + scope block + blank + search + blank + body + blank + bottom rule
        // (L4/SYS-3 — see `render`). The scope block is TWO rows when scoped models exist (the
        // scope line plus its own hint `Text`, `model-selector.ts:96-100`) and one otherwise.
        let scope = self.scope_block_lines(usize::from(width), UiTheme::default_ref()).len() as u16;
        body.saturating_add(7).saturating_add(scope)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let filtered = self.filtered();
        // L4/SYS-3. `ModelSelectorComponent`'s child list (`model-selector.ts:92-129`):
        //   `DynamicBorder`(:92) · `Spacer`(:93) · scope/hint `Text`(:96-104) · `Spacer`(:105) ·
        //   search `Input`(:118) · `Spacer`(:120) · listContainer(:124) · `Spacer`(:126) ·
        //   `DynamicBorder`(:129).
        // **Four** spacers. cyrup already drew `:105` and `:120`; `:93` and `:126` are added here.
        // Unconditional, because upstream's `Spacer` children are — a `Paragraph` draws
        // `lines[0..area.height]` and drops the TRAILING rows, so a short slot shows a strict
        // PREFIX of this vector, matching pi's layout engine (see `crate::selector::stack_rows`).
        let mut lines: Vec<Line<'static>> = vec![border_rule_line(area.width, theme)];
        lines.push(Line::from(""));
        // The scope hint is its OWN `Text` child (`:99-100`) and exists only on the scoped branch.
        lines.extend(self.scope_block_lines(usize::from(area.width), theme));
        lines.push(Line::from(""));
        // Search box with a visible block cursor (feature #9 "selector IME cursor").
        //
        // S31: the prompt is `Input.render`'s shared, unstyled `"> "` at column 0 (`input.ts:380`),
        // because `model-selector.ts:118` adds `this.searchInput` to the container as a bare child.
        // cyrup drew accent `" ▏"…"▏"` bars around the value — one column in, coloured, and U+258F
        // occurs nowhere in pi's TUI sources.
        lines.push(Line::from(crate::selector::input_line_spans(
            self.input.value(),
            self.input.cursor(),
            area.width,
            theme,
        )));
        lines.push(Line::from(""));
        lines.extend(self.body_lines(&filtered, usize::from(area.width), theme));
        // `if (this.onSelectAsDefaultCallback) { addChild(new Text(theme.fg("dim", "  Enter to
        // select · Ctrl+S to set as default · Esc to cancel"))) }` (`model-selector.ts:138-142`).
        // CONDITIONAL upstream — unlike the thinking picker's, which is unconditional
        // (`thinking-selector.ts:94`) — so a picker opened on a path that cannot persist does not
        // advertise the key. `default_model.is_some()` is that callback's presence here.
        if self.default_model.is_some() {
            lines.push(Line::from(Span::styled(
                "  Enter to select \u{b7} Ctrl+S to set as default \u{b7} Esc to cancel",
                theme.dim_style(),
            )));
        }
        lines.push(Line::from(""));
        lines.push(border_rule_line(area.width, theme));
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // `⇥` toggles scope (Pi `tui.input.tab`, `:288-296`).
        if key.code == KeyCode::Tab {
            self.toggle_scope();
            return SelectorOutcome::Redraw;
        }
        // Pi's second confirm key — a LITERAL `matchesKey(keyData, "ctrl+s")` (`:401`), not a
        // binding id, and guarded on the persist callback being wired. When it is NOT wired this
        // early-return is skipped and `Ctrl+S` falls to the `None` arm below, which refuses it as a
        // control char and returns `Ignored`. Pi instead hands the key to its search input
        // (`:409-412`), where `Input.handleInput` drops it as a C0 control char before inserting
        // (`input.ts:203-209`) — same net effect, one layer down. Checked before the keymap so a
        // rebound `tui.select.*` cannot shadow it.
        if self.default_model.is_some()
            && key.code == KeyCode::Char('s')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return match self.current() {
                Some(m) => SelectorOutcome::ConfirmDefault(format!("{}/{}", m.provider, m.id)),
                None => SelectorOutcome::Redraw,
            };
        }
        match keymap.action_for(key) {
            Some(SelectAction::Up) => {
                let len = self.filtered().len();
                if len == 0 {
                    return SelectorOutcome::Redraw;
                }
                // Wrap to bottom at the top (Pi `:299-303`).
                self.selected = if self.selected == 0 { len - 1 } else { self.selected - 1 };
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) => {
                let len = self.filtered().len();
                if len == 0 {
                    return SelectorOutcome::Redraw;
                }
                // Wrap to top at the bottom (Pi `:305-309`).
                self.selected = if self.selected + 1 >= len { 0 } else { self.selected + 1 };
                SelectorOutcome::Redraw
            }
            Some(SelectAction::PageUp) => {
                self.selected = self.selected.saturating_sub(self.max_visible);
                SelectorOutcome::Redraw
            }
            Some(SelectAction::PageDown) => {
                let len = self.filtered().len();
                if len > 0 {
                    self.selected = (self.selected + self.max_visible).min(len - 1);
                }
                SelectorOutcome::Redraw
            }
            // Handled below, ahead of this match — see the `Ctrl+S` guard at the top of `handle`.
            Some(SelectAction::Confirm) => match self.current() {
                // Confirm the fully-qualified `provider/id` (Pi `handleSelect` →
                // `setDefaultModelAndProvider(model.provider, model.id)`, model-selector.ts:330) so a
                // cross-provider selection resolves against the right provider and swaps it.
                Some(m) => SelectorOutcome::Confirm(format!("{}/{}", m.provider, m.id)),
                None => SelectorOutcome::Redraw,
            },
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => {
                // Everything else feeds the search input (Pi `:409-411`: `searchInput.handleInput`
                // then `filterModels(searchInput.getValue())`) — which is now the shared `Input`,
                // so Ctrl+W / Ctrl+U / Ctrl+K / Alt+B / Alt+F / Ctrl+Y / Ctrl+- all land here.
                match self.input.handle_key(key) {
                    // A changed query re-filters, so the highlight returns to the top — what
                    // `insert_char`/`backspace` used to do inline.
                    InputOutcome::Edited => {
                        self.selected = 0;
                        SelectorOutcome::Redraw
                    }
                    InputOutcome::Moved => SelectorOutcome::Redraw,
                    InputOutcome::Ignored => SelectorOutcome::Ignored,
                }
            }
        }
    }

    /// Adopt the live editor keymap: the scope hint names the user's `tui.input.tab` binding rather
    /// than a hardcoded glyph (Pi `keyHint("tui.input.tab", "scope")`, `:228-230`), and the embedded
    /// [`Input`] resolves its own bindings through the same table.
    fn set_editor_keymap(&mut self, keymap: &EditorKeymap) {
        if let Some(label) = keymap.keys_label(EditorAction::Tab) {
            self.scope_key = label;
        }
        self.input.set_editor_keymap(keymap);
    }

    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        self.input.paste(text);
        self.selected = 0;
        SelectorOutcome::Redraw
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn entry(id: &str, provider: &str, current: bool, scoped: bool) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            name: id.to_uppercase(),
            provider: provider.to_string(),
            current,
            scoped,
        }
    }

    fn catalog() -> Vec<ModelEntry> {
        vec![
            entry("claude-opus-4-6", "anthropic", true, true),
            entry("claude-sonnet-4-6", "anthropic", false, true),
            entry("gpt-5.1", "openai", false, false),
            entry("gemini-3-pro", "google", false, false),
        ]
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn current_model_sorted_first_and_preselected() {
        let sel = ModelSelector::new(catalog());
        assert_eq!(sel.models[0].id, "claude-opus-4-6");
        assert!(sel.current().unwrap().current);
    }

    #[test]
    fn typing_fuzzy_filters() {
        let mut sel = ModelSelector::new(catalog());
        let km = SelectKeymap::default();
        // Switch to `all` so openai is visible, then type `gpt`.
        sel.handle(&key(KeyCode::Tab), &km);
        for c in "gpt".chars() {
            sel.handle(&key(KeyCode::Char(c)), &km);
        }
        assert_eq!(sel.visible_len(), 1);
        assert_eq!(sel.current().unwrap().id, "gpt-5.1");
    }

    #[test]
    fn tab_toggles_scope() {
        let mut sel = ModelSelector::new(catalog());
        let km = SelectKeymap::default();
        assert!(sel.is_scoped());
        assert_eq!(sel.visible_len(), 2); // the two scoped anthropic models
        sel.handle(&key(KeyCode::Tab), &km);
        assert!(!sel.is_scoped());
        assert_eq!(sel.visible_len(), 4);
    }

    #[test]
    fn renders_search_badges_and_active_check() {
        let mut sel = ModelSelector::new(catalog());
        // Move to `all` scope so all providers appear.
        sel.handle(&key(KeyCode::Tab), &SelectKeymap::default());
        let theme = UiTheme::default();
        let backend = TestBackend::new(72, 16);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
        let text: String =
            term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Scope:"));
        assert!(text.contains("[anthropic]"));
        assert!(text.contains("[openai]"));
        assert!(text.contains("✓"));
    }

    // ---- F6: `/model <text>` exact match + pre-filter --------------------------------------------

    fn unscoped() -> Vec<ModelEntry> {
        vec![
            entry("claude-opus-4-6", "anthropic", true, false),
            entry("gpt-5.1", "openai", false, false),
            entry("gemini-3-pro", "google", false, false),
        ]
    }

    #[test]
    fn exact_match_by_canonical_reference() {
        let cat = catalog();
        let m = find_exact_model_reference_match(&cat, "openai/gpt-5.1").unwrap();
        assert_eq!(m.id, "gpt-5.1");
        assert_eq!(m.provider, "openai");
    }

    #[test]
    fn exact_match_by_canonical_is_case_insensitive() {
        let cat = catalog();
        let m = find_exact_model_reference_match(&cat, "OpenAI/GPT-5.1").unwrap();
        assert_eq!(m.id, "gpt-5.1");
    }

    #[test]
    fn exact_match_by_bare_unique_id() {
        let cat = catalog();
        let m = find_exact_model_reference_match(&cat, "gpt-5.1").unwrap();
        assert_eq!(m.provider, "openai");
        // A `provider/id` split also resolves.
        let m2 = find_exact_model_reference_match(&cat, "anthropic/claude-opus-4-6").unwrap();
        assert_eq!(m2.id, "claude-opus-4-6");
    }

    #[test]
    fn ambiguous_bare_id_across_providers_is_rejected() {
        // Same bare id under two providers → no unambiguous match (Pi rejects cross-provider dupes).
        let models = vec![
            entry("shared-id", "anthropic", false, false),
            entry("shared-id", "openai", false, false),
        ];
        assert!(find_exact_model_reference_match(&models, "shared-id").is_none());
        // …but the canonical `provider/id` still disambiguates.
        let m = find_exact_model_reference_match(&models, "openai/shared-id").unwrap();
        assert_eq!(m.provider, "openai");
    }

    #[test]
    fn partial_reference_has_no_exact_match() {
        let cat = catalog();
        // `qwen` names no model → the caller opens the picker pre-filtered instead of setting directly.
        assert!(find_exact_model_reference_match(&cat, "qwen").is_none());
        // A partial that is a substring of one id is still NOT an exact match.
        assert!(find_exact_model_reference_match(&cat, "gpt").is_none());
        // Empty / whitespace never matches.
        assert!(find_exact_model_reference_match(&cat, "   ").is_none());
    }

    #[test]
    fn set_search_prefilters_the_picker() {
        // Pre-fill mirrors `/model gpt` opening the picker already narrowed. Use an unscoped catalog so
        // the default scope is `all` and the query filters across every provider.
        let mut sel = ModelSelector::new(unscoped());
        sel.set_search("gpt".to_string());
        assert_eq!(sel.visible_len(), 1, "picker opens pre-filtered to the seeded term");
        assert_eq!(sel.current().unwrap().id, "gpt-5.1");
        // The seeded query renders in the search box.
        let theme = UiTheme::default();
        let mut term = Terminal::new(TestBackend::new(72, 16)).unwrap();
        term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
        let text: String =
            term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("gpt"), "seeded search term shown in the box: {text}");
    }
    // ---- S32 / S30 / S23: the scope block and the refresh-status + error rows ---------------------

    /// Render at the selector's own natural height and return the rows, trailing-trimmed, together
    /// with the buffer so a test can also probe per-cell colour.
    fn draw(
        sel: &mut ModelSelector,
        w: u16,
        theme: &UiTheme,
    ) -> (Vec<String>, ratatui::buffer::Buffer) {
        let h = sel.desired_height(w);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| sel.render(f, f.area(), theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let rows = (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..buf.area.width {
                    if let Some(c) = buf.cell((x, y)) {
                        line.push_str(c.symbol());
                    }
                }
                line.trim_end().to_string()
            })
            .collect();
        (rows, buf)
    }

    fn fg_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Option<ratatui::style::Color> {
        buf.cell((x, y)).map(|c| c.style().fg).unwrap_or(None)
    }

    /// **S32.** Both `Text` children of the scope block are `new Text(…, 0, 0)` — `paddingX = 0`
    /// (`model-selector.ts:97`, `:103`) — and `getScopeText` starts with the literal `"Scope: "`
    /// (`:225`), no leading space. cyrup indented both one column, out of line with the `  `-prefixed
    /// rows below.
    ///
    /// **S30.** `getScopeHintText` is its **own** `Text` child added at `:99-100`, not a tail
    /// appended to the scope line, and it is two-tone: `keyHint` is
    /// `dim(keyText(binding)) + muted(" " + description)` (`keybinding-hints.ts:42-44`) with a second
    /// muted run for ` (all/scoped)` (`:229`). The key comes from the LIVE `tui.input.tab` binding —
    /// cyrup hardcoded a `⇥` glyph that appears in no upstream string.
    #[test]
    fn scope_row_is_flush_and_its_hint_is_a_separate_two_tone_row() {
        let mut sel = ModelSelector::new(catalog());
        let theme = UiTheme::dark();
        let (rows, buf) = draw(&mut sel, 72, &theme);
        assert_eq!(rows[2], "Scope: all | scoped", "no leading space (S32): {rows:?}");
        assert_eq!(rows[3], "tab scope (all/scoped)", "its own row (S30): {rows:?}");
        assert_eq!(rows[4], "", "Spacer(1) (`:105`) follows the whole scope block: {rows:?}");
        assert!(!rows[2].contains('⇥'), "the hardcoded glyph is gone: {rows:?}");
        // Two-tone: the key `dim`, the description `muted` (they are different tokens — `#666666`
        // vs `#808080` — so this cannot pass by accident).
        assert_ne!(theme.dim_style().fg, theme.muted_style().fg);
        assert_eq!(fg_at(&buf, 0, 3), theme.dim_style().fg, "`tab` is dim");
        assert_eq!(fg_at(&buf, 4, 3), theme.muted_style().fg, "`scope …` is muted");
    }

    /// The hint row belongs to the SCOPED branch only: `model-selector.ts:101-104`'s `else` adds the
    /// warning `Text` and nothing else, so a catalog with no scoped models must show one row here,
    /// still flush at column 0 (S32).
    #[test]
    fn unscoped_catalog_shows_only_the_flush_warning_row() {
        let mut sel = ModelSelector::new(unscoped());
        let theme = UiTheme::dark();
        let (rows, _) = draw(&mut sel, 90, &theme);
        assert_eq!(
            rows[2], "Only showing models from configured providers. Use /login to add providers.",
            "no leading space (S32): {rows:?}"
        );
        assert_eq!(rows[3], "", "no scope-hint row on this branch (S30): {rows:?}");
        assert!(rows.iter().all(|r| !r.contains("all/scoped")), "{rows:?}");
    }

    /// **S23.** `model-selector.ts:312-317` appends `Spacer(1)` + `  ${refreshStatusMessage}`,
    /// coloured `success` when the refresh landed and `muted` while it is in flight. cyrup emitted
    /// neither, so `/model` was silent about catalog refreshes.
    #[test]
    fn refresh_status_row_is_muted_in_flight_and_success_when_done() {
        let mut sel = ModelSelector::new(catalog());
        let theme = UiTheme::dark();
        let (before, _) = draw(&mut sel, 72, &theme);
        assert!(before.iter().all(|r| !r.contains("Refreshing")), "{before:?}");

        sel.set_refresh_status("Refreshing model catalogs…", false);
        let (rows, buf) = draw(&mut sel, 72, &theme);
        let n = rows.len();
        assert_eq!(rows[n - 3], "  Refreshing model catalogs…", "{rows:?}");
        assert_eq!(rows[n - 4], "", "Spacer(1) above it (`:313`): {rows:?}");
        assert_eq!(fg_at(&buf, 2, (n - 3) as u16), theme.muted_style().fg, "in flight ⇒ muted");

        sel.set_refresh_status("Model catalogs refreshed.", true);
        let (rows, buf) = draw(&mut sel, 72, &theme);
        let n = rows.len();
        assert_eq!(rows[n - 3], "  Model catalogs refreshed.", "{rows:?}");
        assert_eq!(fg_at(&buf, 2, (n - 3) as u16), theme.success_style().fg, "done ⇒ success");

        sel.set_refresh_status("", false);
        let (rows, _) = draw(&mut sel, 72, &theme);
        assert!(rows.iter().all(|r| !r.contains("catalogs")), "empty clears the row: {rows:?}");
    }

    /// **S23.** `:299-311` is a three-way branch: an `errorMessage` REPLACES both `No matching
    /// models` and the `Model Name:` block, and every one of its `\n`-separated lines is drawn in
    /// `error`.
    #[test]
    fn error_message_replaces_the_model_name_row_and_is_error_coloured() {
        let mut sel = ModelSelector::new(catalog());
        let theme = UiTheme::dark();
        let (before, _) = draw(&mut sel, 72, &theme);
        assert!(before.iter().any(|r| r.starts_with("  Model Name:")), "{before:?}");

        sel.set_error_message(Some(
            "Could not refresh openai; showing cached models.\nRetry with /model.".to_string(),
        ));
        let (rows, buf) = draw(&mut sel, 72, &theme);
        assert!(
            rows.iter().all(|r| !r.contains("Model Name:")),
            "the error block replaces it: {rows:?}"
        );
        let first =
            rows.iter().position(|r| r.starts_with("Could not refresh openai")).unwrap_or(usize::MAX);
        assert!(first != usize::MAX, "error line 1 missing: {rows:?}");
        assert_eq!(rows[first + 1], "Retry with /model.", "one row per \\n (`:301-304`): {rows:?}");
        assert_eq!(fg_at(&buf, 0, first as u16), theme.error_style().fg);
        assert_eq!(fg_at(&buf, 0, (first + 1) as u16), theme.error_style().fg);
    }

    /// **S23.** The refresh-status row is emitted from `updateList`'s tail (`:312`), OUTSIDE the
    /// three-way branch above it — so it still shows when the filtered list is empty. cyrup's
    /// `body_lines` used to early-return on an empty list, which would have swallowed it.
    #[test]
    fn refresh_status_survives_an_empty_filtered_list() {
        let mut sel = ModelSelector::new(catalog());
        sel.set_search("zzzqqq".to_string());
        sel.set_refresh_status("Refreshing model catalogs…", false);
        let theme = UiTheme::dark();
        let (rows, _) = draw(&mut sel, 72, &theme);
        assert_eq!(sel.visible_len(), 0, "the query matches nothing");
        assert!(rows.iter().any(|r| r == "  No matching models"), "{rows:?}");
        assert!(rows.iter().any(|r| r == "  Refreshing model catalogs…"), "{rows:?}");
    }

    /// `Ctrl+S` → `ConfirmDefault`, the confirm key → `Confirm`, both carrying the
    /// fully-qualified `provider/id` (Pi `handleSelect` vs `onSelectAsDefaultCallback`,
    /// `model-selector.ts:401-408`). The two must not collapse: the chrome routes on the VARIANT,
    /// and only `ConfirmDefault` reaches the arm that writes `defaultProvider`/`defaultModel`.
    ///
    /// `catalog()`'s `claude-opus-4-6` is `current: true`, and `with_default_model` sorts
    /// current-first, so it stays row 0 and the highlighted value is unambiguous.
    #[test]
    fn ctrl_s_confirms_as_default_while_enter_stays_session_only() {
        let km = SelectKeymap::default();
        // No helper builds a MODIFIED bare `KeyEvent` — the local `key()` hardcodes
        // `KeyModifiers::NONE` — so this is constructed inline.
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);

        let mut sel =
            ModelSelector::new(catalog()).with_default_model("anthropic", "claude-opus-4-6");
        assert_eq!(
            sel.handle(&ctrl_s, &km),
            SelectorOutcome::ConfirmDefault("anthropic/claude-opus-4-6".to_string()),
            "Ctrl+S persists the highlighted model"
        );

        let mut sel =
            ModelSelector::new(catalog()).with_default_model("anthropic", "claude-opus-4-6");
        assert_eq!(
            sel.handle(&key(KeyCode::Enter), &km),
            SelectorOutcome::Confirm("anthropic/claude-opus-4-6".to_string()),
            "Enter is session-only and must NOT be ConfirmDefault"
        );
    }

    /// With no persist path wired the key does nothing — in Pi and here alike, by different routes.
    /// Pi guards `Ctrl+S` on `onSelectAsDefaultCallback` (`model-selector.ts:401`), so an un-wired
    /// picker takes the else-branch and hands the key to its search input (`:409-412`) — which then
    /// drops it: `Input.handleInput` consults only the `tui.editor.*`, `tui.input.submit` and
    /// `tui.select.cancel` ids, none of which is bound to `ctrl+s`, so the key falls to its C0
    /// control-character rejection (`input.ts:203-209`). Pi does bind `ctrl+s` elsewhere —
    /// `app.session.toggleSort` and `app.models.save` (`keybindings.ts:166,182`), both of which
    /// cyrup mirrors — but `Input` never consults those. `ModelSelector::new` alone is
    /// that state, and reaches the same end one step earlier: the guard is skipped, `action_for`
    /// returns `None` (`Ctrl+S` is unbound in `SelectKeymap::default()`), and the `None` arm's
    /// insert is gated on `!CONTROL` — so `handle` returns `Ignored` without the query ever seeing
    /// the key. Asserted by equality, not as "not `ConfirmDefault`": that weaker form also passes
    /// for `Confirm(_)`, and an un-wired picker that confirmed a selection would be a real bug.
    #[test]
    fn ctrl_s_is_ignored_when_no_default_is_wired() {
        let km = SelectKeymap::default();
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        let mut sel = ModelSelector::new(catalog());
        assert_eq!(
            sel.handle(&ctrl_s, &km),
            SelectorOutcome::Ignored,
            "an un-wired picker must neither confirm nor type — the `None` arm refuses control chars"
        );
    }

    /// MIRROR for the per-component discipline. The `/scoped-models` footer
    /// (`scoped-models-selector.ts:197-208`) and its `(unsaved)` / `N/M enabled` run belong to
    /// `ScopedModelsSelectorComponent` alone; `ModelSelectorComponent` has no footer hint at all —
    /// its children stop at the list `Container` and a `Spacer` (`model-selector.ts:124-129`). If a
    /// later change pushes that footer into shared code, this fails.
    #[test]
    fn model_selector_has_no_scoped_models_footer() {
        let mut sel = ModelSelector::new(catalog());
        let theme = UiTheme::dark();
        let (rows, _) = draw(&mut sel, 72, &theme);
        for needle in ["toggle", "(unsaved)", "enabled", "provider ·", "Model Configuration"] {
            assert!(rows.iter().all(|r| !r.contains(needle)), "{needle:?} leaked into /model: {rows:?}");
        }
    }
}
