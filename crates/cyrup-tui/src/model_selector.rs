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
use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{Selector, SelectorOutcome};
use crate::theme::UiTheme;

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
    scope: Scope,
    /// The live fuzzy search query (embedded `Input`).
    query: String,
    /// Cursor byte offset within `query`.
    cursor: usize,
    /// Highlighted index into the *filtered* list.
    selected: usize,
    max_visible: usize,
}

impl ModelSelector {
    /// Build from the catalog `(id, name, provider, current, scoped)` rows. The list is sorted
    /// current-first then by provider (Pi `sortModels`); the scope defaults to `scoped` when any scoped
    /// models exist, else `all` (`model-selector.ts:82`).
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
            scope,
            query: String::new(),
            cursor: 0,
            selected: 0,
            max_visible: 10,
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
        if self.query.is_empty() {
            return active;
        }
        let texts: Vec<String> = active.iter().map(|m| m.search_text()).collect();
        let matches = fuzzy::filter(&texts, &self.query, String::as_str);
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
        self.cursor = term.len();
        self.query = term;
        self.selected = 0;
    }

    /// Insert a printable char into the search query, resetting the highlight (Pi feeds everything else
    /// to `searchInput`, `:322-325`).
    fn insert_char(&mut self, c: char) {
        self.query.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.selected = 0;
    }

    /// Backspace the search query.
    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some(ch) = self.query[..self.cursor].chars().next_back() {
            let start = self.cursor - ch.len_utf8();
            self.query.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.selected = 0;
        }
    }

    /// The scope header line (`Scope: all | scoped`, Pi `getScopeText`, `:197-201`), or, when no
    /// providers/scoped models exist, the `warning` hint (Pi `:97-98`).
    fn scope_line(&self, theme: &UiTheme) -> Line<'static> {
        if !self.has_scoped {
            return Line::from(Span::styled(
                " Only showing models from configured providers. Use /login to add providers.",
                theme.warning_style(),
            ));
        }
        let (all_style, scoped_style) = match self.scope {
            Scope::All => (theme.accent_style(), theme.muted_style()),
            Scope::Scoped => (theme.muted_style(), theme.accent_style()),
        };
        Line::from(vec![
            Span::styled(" Scope: ", theme.muted_style()),
            Span::styled("all", all_style),
            Span::styled(" | ", theme.muted_style()),
            Span::styled("scoped", scoped_style),
            Span::styled("      ⇥ scope (all/scoped)", theme.muted_style()),
        ])
    }

    /// The windowed list body (Pi `updateList`, `:229-283`): `→ ` cursor + `id` + `[provider]` badge +
    /// `✓` on the active model, a `(i/N)` scroll indicator, and the `Model Name:` footer.
    fn body_lines(&self, filtered: &[&ModelEntry], theme: &UiTheme) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if filtered.is_empty() {
            lines.push(Line::from(Span::styled("  No matching models", theme.muted_style())));
            return lines;
        }
        let len = filtered.len();
        let start = self
            .selected
            .saturating_sub(self.max_visible / 2)
            .min(len.saturating_sub(self.max_visible));
        let end = (start + self.max_visible).min(len);
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
        // `Model Name:` footer for the highlighted model (Pi `:280-283`).
        if let Some(sel) = filtered.get(self.selected) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  Model Name: {}", sel.name),
                theme.muted_style(),
            )));
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
    fn desired_height(&self, _width: u16) -> u16 {
        let filtered = self.filtered();
        let body = self.body_lines(&filtered, &UiTheme::default()).len() as u16;
        // top rule + scope + blank + search + blank + body + bottom rule.
        body.saturating_add(6)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let filtered = self.filtered();
        // Top rule → scope header → blank → search box → blank, then the windowed body + bottom rule.
        let mut lines: Vec<Line<'static>> = vec![
            border_rule_line(area.width, theme),
            self.scope_line(theme),
            Line::from(""),
            {
                // Search box with a visible block cursor (feature #9 "selector IME cursor").
                let mut spans = vec![Span::styled(" ▏", theme.accent_style())];
                spans.extend(crate::selector::search_input_spans(&self.query, self.cursor, theme));
                spans.push(Span::styled("▏", theme.accent_style()));
                Line::from(spans)
            },
            Line::from(""),
        ];
        lines.extend(self.body_lines(&filtered, theme));
        lines.push(border_rule_line(area.width, theme));
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // `⇥` toggles scope (Pi `tui.input.tab`, `:288-296`).
        if key.code == KeyCode::Tab {
            self.toggle_scope();
            return SelectorOutcome::Redraw;
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
            Some(SelectAction::Confirm) => match self.current() {
                // Confirm the fully-qualified `provider/id` (Pi `handleSelect` →
                // `setDefaultModelAndProvider(model.provider, model.id)`, model-selector.ts:330) so a
                // cross-provider selection resolves against the right provider and swaps it.
                Some(m) => SelectorOutcome::Confirm(format!("{}/{}", m.provider, m.id)),
                None => SelectorOutcome::Redraw,
            },
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => {
                // Everything else feeds the search input (Pi `:322-325`).
                if let KeyCode::Char(c) = key.code
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.insert_char(c);
                    return SelectorOutcome::Redraw;
                }
                if key.code == KeyCode::Backspace {
                    self.backspace();
                    return SelectorOutcome::Redraw;
                }
                SelectorOutcome::Ignored
            }
        }
    }
}

/// A full-width `─` rule line (Pi `DynamicBorder`; mirrors the other selectors).
fn border_rule_line(width: u16, theme: &UiTheme) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width.max(1) as usize), theme.border_style()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
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
}
