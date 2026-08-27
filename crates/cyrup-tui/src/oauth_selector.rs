//! The `/login` + `/logout` provider picker (spec/tui/05 §6; a 1:1 port of
//! `components/oauth-selector.ts`, 214 lines).
//!
//! `OAuthSelectorComponent` is **not** a `SelectList` consumer. It builds each row itself as a
//! concatenation — `prefix + name + authTypeLabel + statusIndicator` (`:134-142`) — under a real
//! search [`Input`](crate::selector::INPUT_PROMPT) with a fuzzy filter over
//! `` `${name} ${id} ${authType} ${method?.name}` `` (`:102-112`). cyrup routed both commands through
//! [`ListSelector::data`](crate::selector::ListSelector), which has neither:
//!
//! * **S5** — no search box at all. `/login` lists every catalog provider that supports a login (30+
//!   on a full install) and `/logout` every stored credential, with no way to narrow either.
//! * **S21** — the ` [subscription]` badge was folded into the label string and the status into the
//!   `SelectItem.description`, which `select_list.rs` paints uniformly `muted` in a padded right
//!   column. `✓ configured` came out grey instead of `success` green, a credential-kind mismatch
//!   lost its `warning` colour, and the status started at roughly x=14 instead of one space after
//!   the provider name.
//!
//! Confirming carries the chosen row's **INDEX** into the original `options` slice (the chrome maps
//! it back through `AppState::{login,logout}_options`), not the provider id: one provider can offer
//! both a subscription and an API-key row and the id alone cannot tell them apart. The fuzzy filter
//! reorders rows, so the index is resolved through the filtered view exactly as upstream's
//! `this.filteredProviders[this.selectedIndex]` (`:199`) is.

use cyrup_config::login::LoginProviderOption;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::auth_select::{format_auth_selector_provider_type, status_indicator_runs, StatusTone};
use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{border_rule_line, centered_window, input_line_spans, Selector, SelectorOutcome};
use crate::text_width::truncate_line_to_width;
use crate::theme::UiTheme;

/// `maxVisible` (`oauth-selector.ts:117`) — **8**, not the 10 the data selectors use.
const MAX_VISIBLE: usize = 8;

/// Which command opened the picker (`mode`, `oauth-selector.ts:46`). It picks the title (`:72`) and
/// the empty-catalog copy (`:154-159`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthMode {
    Login,
    Logout,
}

impl OAuthMode {
    /// `const title = mode === "login" ? … : …` (`oauth-selector.ts:72`), verbatim.
    fn title(self) -> &'static str {
        match self {
            OAuthMode::Login => "Select provider to configure:",
            OAuthMode::Logout => "Select provider to logout:",
        }
    }

    /// The `allProviders.length === 0` copy (`oauth-selector.ts:155-158`), verbatim.
    fn empty_message(self) -> &'static str {
        match self {
            OAuthMode::Login => "No providers available",
            OAuthMode::Logout => "No providers logged in. Use /login first.",
        }
    }
}

/// One provider row, pre-resolved from a [`LoginProviderOption`] so `render` stays a pure
/// `state -> lines`.
struct ProviderRow {
    /// `provider.name` — the row's primary text (`:137`/`:140`).
    name: String,
    /// `` ` [${formatAuthSelectorProviderType(authType)}]` `` (`:132`), or `None` when the list does
    /// not mix kinds (`showAuthTypeLabels`, `:63`).
    auth_type_label: Option<String>,
    /// The styled status runs (`formatStatusIndicator`, `:164-181`).
    status: Vec<(StatusTone, String)>,
    /// `` `${provider.name} ${provider.id} ${provider.authType} ${provider.method?.name ?? ""}` ``
    /// — the fuzzy key (`:107`).
    search_text: String,
}

/// The `/login` + `/logout` picker.
pub struct OAuthSelector {
    mode: OAuthMode,
    rows: Vec<ProviderRow>,
    /// Indices into [`Self::rows`] surviving the query, in fuzzy-match order
    /// (`this.filteredProviders`, `:44`).
    filtered: Vec<usize>,
    /// Highlighted index into [`Self::filtered`] (`selectedIndex`, `:45`).
    selected: usize,
    /// The search `Input`'s value (`:76`) and caret offset.
    query: String,
    cursor: usize,
}

impl OAuthSelector {
    /// Build from the resolved `AuthSelectorProvider[]`
    /// (`cyrup_config::login::{login_provider_options, logout_provider_options}`), already sorted by
    /// display name. `initial_search` seeds the box exactly as upstream's `initialSearchInput`
    /// (`:56`, `:77-79`, `:99`).
    pub fn new(
        mode: OAuthMode,
        options: &[LoginProviderOption],
        initial_search: Option<String>,
    ) -> Self {
        // `new Set(providers.map(p => p.authType)).size > 1` (`:63`).
        let show_auth_type_labels = options
            .iter()
            .any(|o| Some(o.auth_type) != options.first().map(|f| f.auth_type));
        let rows: Vec<ProviderRow> = options
            .iter()
            .map(|o| {
                let auth_type = match o.auth_type {
                    cyrup_config::login::AuthType::Oauth => "oauth",
                    cyrup_config::login::AuthType::ApiKey => "api_key",
                };
                ProviderRow {
                    name: o.name.clone(),
                    auth_type_label: show_auth_type_labels.then(|| {
                        format!(" [{}]", format_auth_selector_provider_type(o.auth_type))
                    }),
                    status: status_indicator_runs(o),
                    search_text: format!(
                        "{} {} {auth_type} {}",
                        o.name,
                        o.id,
                        o.method_name.as_deref().unwrap_or("")
                    ),
                }
            })
            .collect();
        let mut sel = OAuthSelector {
            mode,
            rows,
            filtered: Vec::new(),
            selected: 0,
            query: initial_search.unwrap_or_default(),
            cursor: 0,
        };
        sel.cursor = sel.query.len();
        // `this.filterProviders(initialSearchInput ?? "")` (`:99`).
        sel.apply_filter();
        sel
    }

    /// `filterProviders(query)` (`:102-112`): fuzzy over the composed search text (best match
    /// first), else the identity; then clamp the highlight into range.
    fn apply_filter(&mut self) {
        self.filtered = if self.query.is_empty() {
            (0..self.rows.len()).collect()
        } else {
            let texts: Vec<&str> = self.rows.iter().map(|r| r.search_text.as_str()).collect();
            crate::fuzzy::filter(&texts, &self.query, |t| t).into_iter().map(|m| m.index).collect()
        };
        // `Math.max(0, Math.min(selectedIndex, Math.max(0, filtered.length - 1)))` (`:110`).
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    /// The live query (test/inspection).
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The number of rows surviving the query (test/inspection).
    pub fn visible_len(&self) -> usize {
        self.filtered.len()
    }

    /// The highlighted row's index into the ORIGINAL `options` slice, if any.
    pub fn current_index(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    fn insert_char(&mut self, c: char) {
        self.query.insert(self.cursor, c);
        self.cursor = self.cursor.saturating_add(c.len_utf8());
        self.apply_filter();
    }

    fn backspace(&mut self) {
        let Some(ch) = self.query.get(..self.cursor).and_then(|s| s.chars().next_back()) else {
            return;
        };
        let start = self.cursor.saturating_sub(ch.len_utf8());
        self.query.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.apply_filter();
    }

    /// The visible window `[start, end)` — `updateList`'s centred window (`:117-122`), `maxVisible`
    /// = [`MAX_VISIBLE`].
    fn window(&self) -> (usize, usize) {
        centered_window(self.selected, self.filtered.len(), MAX_VISIBLE)
    }

    /// The `listContainer` rows (`updateList`, `:114-161`), each already reduced to one `Line`.
    ///
    /// Every child is a `new TruncatedText(line, 1, 0)` (`:144`, `:149`, `:160`), so each row is
    /// inset one column and hard-truncated to `width - 2` (`text.ts:64`/`truncated-text.ts:34-44`).
    fn body_lines(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let avail = usize::from(width).saturating_sub(2);
        let inset = |line: Line<'static>| {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(truncate_line_to_width(line, avail, "...").spans);
            Line::from(spans)
        };
        // "Show `no providers` if empty" (`:152-161`) — note upstream appends this AFTER the loop
        // and the scroll indicator, but both are empty when the filtered list is, so it is the only
        // row drawn.
        if self.filtered.is_empty() {
            let message = if self.rows.is_empty() {
                self.mode.empty_message()
            } else {
                "No matching providers"
            };
            return vec![inset(Line::from(Span::styled(
                format!("  {message}"),
                theme.muted_style(),
            )))];
        }
        let (start, end) = self.window();
        let mut lines = Vec::with_capacity(end.saturating_sub(start).saturating_add(1));
        for i in start..end {
            let Some(row) = self.filtered.get(i).and_then(|r| self.rows.get(*r)) else { continue };
            let is_selected = i == self.selected;
            // `:135-142`: the prefix and the NAME take the accent on the highlighted row; the
            // badge and the status keep their own colours either way.
            let mut spans = if is_selected {
                vec![
                    Span::styled("→ ", theme.accent_style()),
                    Span::styled(row.name.clone(), theme.accent_style()),
                ]
            } else {
                vec![Span::raw("  "), Span::styled(row.name.clone(), theme.base_style())]
            };
            if let Some(label) = &row.auth_type_label {
                spans.push(Span::styled(label.clone(), theme.muted_style()));
            }
            for (tone, text) in &row.status {
                let style = match tone {
                    StatusTone::Muted => theme.muted_style(),
                    StatusTone::Warning => theme.warning_style(),
                    StatusTone::Success => theme.success_style(),
                };
                spans.push(Span::styled(text.clone(), style));
            }
            lines.push(inset(Line::from(spans)));
        }
        // `  (i/N)` scroll indicator, muted (`:147-150`).
        if start > 0 || end < self.filtered.len() {
            lines.push(inset(Line::from(Span::styled(
                format!("  ({}/{})", self.selected + 1, self.filtered.len()),
                theme.muted_style(),
            ))));
        }
        lines
    }

    /// The title row (`:73`): `new TruncatedText(theme.fg("accent", theme.bold(title)), 1, 0)`.
    fn title_line(&self, width: u16, theme: &UiTheme) -> Line<'static> {
        let avail = usize::from(width).saturating_sub(2);
        let line = Line::from(Span::styled(
            self.mode.title(),
            theme.accent_style().add_modifier(Modifier::BOLD),
        ));
        let mut spans = vec![Span::raw(" ")];
        spans.extend(truncate_line_to_width(line, avail, "...").spans);
        Line::from(spans)
    }
}

impl Selector for OAuthSelector {
    fn desired_height(&self, width: u16) -> u16 {
        // `DynamicBorder`(:68) · `Spacer`(:69) · title(:73) · `Spacer`(:74) · `Input`(:86) ·
        // `Spacer`(:87) · list(:91) · `Spacer`(:93) · `DynamicBorder`(:96) — **eight** fixed rows
        // around the body. All FOUR spacers land now that the `Input` exists (S5); the previous
        // `ListSelector` route drew three and left the `:87` blank floating. The title is a
        // `TruncatedText`, which is always exactly one row (`truncated-text.ts:36-56`), so this
        // constant does not depend on `width`.
        let body = self.body_lines(width, UiTheme::default_ref()).len();
        body.saturating_add(8).min(usize::from(u16::MAX)) as u16
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        // `DynamicBorder`(:68) · `Spacer`(:69) · title(:73) · `Spacer`(:74) · `Input`(:86) ·
        // `Spacer`(:87). The `Input` is a bare container child (`:86`), so it renders at column 0
        // behind the shared unstyled `"> "` prompt (S31, `input.ts:380`).
        let mut lines: Vec<Line<'static>> = vec![
            border_rule_line(area.width, theme),
            Line::from(""),
            self.title_line(area.width, theme),
            Line::from(""),
            Line::from(input_line_spans(&self.query, self.cursor, theme)),
            Line::from(""),
        ];
        lines.extend(self.body_lines(area.width, theme));
        lines.push(Line::from(""));
        lines.push(border_rule_line(area.width, theme));
        // Over-tall: a `Paragraph` draws `lines[0..area.height]` and DROPS THE TRAILING rows, so
        // what survives is a strict PREFIX of the natural render — which is what pi's layout engine
        // does to an over-tall `Container` (`packages/tui/src/layout.ts:113,307-310`; see
        // `crate::selector::stack_rows`' doc for the full argument, and
        // `tests/dialog_envelope_spacers.rs::a_paragraph_keeps_the_first_rows_and_drops_the_rest`
        // for the ratatui half).
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        match keymap.action_for(key) {
            // Up/Down CLAMP here — `Math.max(0, …)` / `Math.min(len - 1, …)` (`:186-196`), unlike
            // `SelectList`, which wraps. An empty filtered list swallows the key (`:187`, `:193`).
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                if !self.filtered.is_empty() {
                    self.selected = self.selected.saturating_sub(1);
                }
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                if !self.filtered.is_empty() {
                    self.selected =
                        self.selected.saturating_add(1).min(self.filtered.len().saturating_sub(1));
                }
                SelectorOutcome::Redraw
            }
            // `:198-203` — confirming an empty list does nothing.
            Some(SelectAction::Confirm) => match self.current_index() {
                Some(i) => SelectorOutcome::Confirm(i.to_string()),
                None => SelectorOutcome::Redraw,
            },
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            // "Pass everything else to search input" (`:208-212`).
            None => {
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
