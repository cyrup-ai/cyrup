//! The `/resume` session selector (spec/tui/05 §6; port of Pi's
//! `components/session-selector.ts` (1031) over `session-selector-search.ts`). Unlike the plain
//! [`ListSelector`](crate::selector::ListSelector), the resume picker carries an embedded **search
//! input** driving the query-DSL ([`crate::session_search`]), a cyclable **sort mode**, a **name
//! filter** toggle, a **delete-confirmation** state, and an in-list **rename** input — the full
//! interaction set Pi's session selector exposes (`session-selector.ts:532-637`).
//!
//! It occupies the input slot like every other editor-swap selector and emits its effects through the
//! shared [`SelectorOutcome`]: a plain row confirm is [`SelectorOutcome::Confirm`] (resume), while
//! delete/rename ride [`SelectorOutcome::Apply`] with a unit-separator-tagged payload the chrome
//! decodes via [`SessionSelectorOutcome::parse_apply`] and drives against the additive
//! `delete_session_file` / `rename_session_file` session seams. Mutations are reflected in the
//! selector's own row list first, so no callback back into the slot is needed.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{Selector, SelectorOutcome};
use crate::session_search::{filter_and_sort, NameFilter, SearchRow, SortMode};
use crate::settings_selector::FIELD_SEP;
use crate::theme::UiTheme;

/// One persisted session offered by the `/resume` picker.
#[derive(Clone, Debug)]
pub struct SessionRow {
    /// The session file path (the confirm/delete/rename value).
    pub path: String,
    /// The display label (name, else first message, else id).
    pub label: String,
    /// The session display name, if any (drives the `Named` filter + rename prefill).
    pub name: Option<String>,
    /// A secondary line (message count, `(current)` marker).
    pub desc: Option<String>,
    /// The assembled search text (`{id} {name} {allMessagesText} {cwd}`).
    pub search_text: String,
    /// A recency key (newer = larger) for the `Relevance` tie-break.
    pub recency: u128,
}

/// The decoded effect of a session-selector [`SelectorOutcome`] (the chrome's view). `Resume` comes
/// from a plain [`SelectorOutcome::Confirm`]; `Delete`/`Rename` are decoded from the tagged
/// [`SelectorOutcome::Apply`] payload via [`Self::parse_apply`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSelectorOutcome {
    /// Resume the session at `path`.
    Resume(String),
    /// Delete the session file at `path`.
    Delete(String),
    /// Rename the session at `path` to `name`.
    Rename { path: String, name: String },
}

/// The leading byte that marks a session-selector [`SelectorOutcome::Apply`] payload (so the chrome's
/// `/settings` `Apply` handler — which splits a non-prefixed `"id\u{1f}value"` — never mis-routes it).
const SESSION_TAG: char = FIELD_SEP;

impl SessionSelectorOutcome {
    /// Decode a tagged session `Apply` payload (`"\u{1f}delete\u{1f}<path>"` /
    /// `"\u{1f}rename\u{1f}<path>\u{1f}<name>"`). Returns `None` for any non-session payload (e.g. a
    /// `/settings` `"id\u{1f}value"`), so the chrome can fall through to its settings handler.
    pub fn parse_apply(payload: &str) -> Option<Self> {
        let rest = payload.strip_prefix(SESSION_TAG)?;
        let (action, args) = rest.split_once(FIELD_SEP)?;
        match action {
            "delete" => Some(SessionSelectorOutcome::Delete(args.to_string())),
            "rename" => {
                let (path, name) = args.split_once(FIELD_SEP)?;
                Some(SessionSelectorOutcome::Rename {
                    path: path.to_string(),
                    name: name.to_string(),
                })
            }
            _ => None,
        }
    }

    /// Encode a delete `Apply` payload.
    fn delete_payload(path: &str) -> String {
        format!("{SESSION_TAG}delete{FIELD_SEP}{path}")
    }

    /// Encode a rename `Apply` payload.
    fn rename_payload(path: &str, name: &str) -> String {
        format!("{SESSION_TAG}rename{FIELD_SEP}{path}{FIELD_SEP}{name}")
    }
}

/// The interactive `/resume` selector.
pub struct SessionSelector {
    rows: Vec<SessionRow>,
    /// The live search query (the embedded `Input`).
    query: String,
    /// Cursor byte offset within `query`.
    cursor: usize,
    sort: SortMode,
    name_filter: NameFilter,
    /// Show the full path under each row (`Ctrl+P`).
    show_path: bool,
    /// Highlighted index into the *filtered* list.
    selected: usize,
    /// When `Some(path)`, a delete confirmation is pending for that row.
    confirming_delete: Option<String>,
    /// When `Some((path, buffer)）`, a rename input is open for that row.
    renaming: Option<(String, String)>,
    max_visible: usize,
}

impl SessionSelector {
    /// Build from the resume `rows` (newest-first). Defaults to recent sort / all names, like Pi's
    /// non-threaded resume entry (cyrup has no thread builder yet, so `Threaded` aliases `Recent`).
    pub fn new(rows: Vec<SessionRow>) -> Self {
        SessionSelector {
            rows,
            query: String::new(),
            cursor: 0,
            sort: SortMode::Recent,
            name_filter: NameFilter::All,
            show_path: false,
            selected: 0,
            confirming_delete: None,
            renaming: None,
            max_visible: 10,
        }
    }

    /// The filtered + sorted rows for the current query/sort/name-filter (clones for borrow ease; the
    /// list is small).
    fn filtered(&self) -> Vec<SessionRow> {
        let search_rows: Vec<SearchRow<usize>> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| SearchRow {
                text: r.search_text.clone(),
                name: r.name.clone(),
                recency: r.recency,
                item: i,
            })
            .collect();
        let idxs = filter_and_sort(&search_rows, &self.query, self.sort, self.name_filter);
        idxs.into_iter().filter_map(|i| self.rows.get(i).cloned()).collect()
    }

    /// The highlighted row of the *filtered* list, if any (test/inspection).
    pub fn current(&self) -> Option<SessionRow> {
        self.filtered().into_iter().nth(self.selected)
    }

    /// The number of currently-visible (filtered) rows (test/inspection).
    pub fn visible_len(&self) -> usize {
        self.filtered().len()
    }

    /// The active sort mode (test/inspection).
    pub fn sort_mode(&self) -> SortMode {
        self.sort
    }

    /// The active name filter (test/inspection).
    pub fn name_filter(&self) -> NameFilter {
        self.name_filter
    }

    /// Whether a delete confirmation is pending (test/inspection).
    pub fn is_confirming_delete(&self) -> bool {
        self.confirming_delete.is_some()
    }

    /// Whether a rename input is open (test/inspection).
    pub fn is_renaming(&self) -> bool {
        self.renaming.is_some()
    }

    /// The sort label shown in the header (`Threaded`/`Recent`/`Fuzzy`, `session-selector.ts:134`).
    fn sort_label(&self) -> &'static str {
        match self.sort {
            SortMode::Threaded => "Threaded",
            SortMode::Recent => "Recent",
            SortMode::Relevance => "Fuzzy",
        }
    }

    /// Cycle the sort mode (`toggleSort`, `session-selector.ts:986`):
    /// threaded → recent → relevance → threaded.
    fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            SortMode::Threaded => SortMode::Recent,
            SortMode::Recent => SortMode::Relevance,
            SortMode::Relevance => SortMode::Threaded,
        };
        self.clamp_selection();
    }

    /// Toggle the name filter (all ↔ named, `session-selector.ts:356`).
    fn toggle_name_filter(&mut self) {
        self.name_filter = match self.name_filter {
            NameFilter::All => NameFilter::Named,
            NameFilter::Named => NameFilter::All,
        };
        self.clamp_selection();
    }

    /// Keep the highlight inside the (possibly shrunken) filtered list.
    fn clamp_selection(&mut self) {
        let len = self.filtered().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// Insert a printable char into the active text field (search or rename input).
    fn insert_char(&mut self, c: char) {
        if let Some((_, buf)) = self.renaming.as_mut() {
            buf.push(c);
            return;
        }
        self.query.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.selected = 0;
    }

    /// Backspace the active text field.
    fn backspace(&mut self) {
        if let Some((_, buf)) = self.renaming.as_mut() {
            buf.pop();
            return;
        }
        if self.cursor == 0 {
            return;
        }
        // Remove the char ending at `cursor`.
        let prev = self.query[..self.cursor].chars().next_back();
        if let Some(ch) = prev {
            let start = self.cursor - ch.len_utf8();
            self.query.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.selected = 0;
        }
    }

    /// Build the body display lines for the windowed filtered list at `width` columns.
    ///
    /// `width` is needed for the `selectedBg` fill (S2): `session-selector.ts:505-509` builds the
    /// row as `leftPart + " ".repeat(spacing) + styledRight` where
    /// `spacing = max(1, width - leftWidth - rightWidth)`, so the string the fill wraps spans the
    /// whole row — the selection reads as a full-width bar, not a ragged one ending at the label.
    fn body_lines(&self, theme: &UiTheme, filtered: &[SessionRow], width: u16) -> Vec<Line<'static>> {
        if filtered.is_empty() {
            return vec![Line::from(Span::styled("  No sessions found", theme.muted_style()))];
        }
        let len = filtered.len();
        let start = self
            .selected
            .saturating_sub(self.max_visible / 2)
            .min(len.saturating_sub(self.max_visible));
        let end = (start + self.max_visible).min(len);
        let mut lines = Vec::new();
        for (i, row) in filtered.iter().enumerate().take(end).skip(start) {
            let is_sel = i == self.selected;
            // S10: the cursor glyph is U+203A `› ` (`session-selector.ts:476`
            // `isSelected ? theme.fg("accent", "› ") : "  "`), not U+2192 `→ `. `→ ` is correct in
            // `SelectList` (`select-list.ts:146`) — only this selector diverged.
            let cursor = if is_sel { "› " } else { "  " };
            let style = if is_sel {
                theme.accent_style().add_modifier(Modifier::BOLD)
            } else {
                theme.base_style()
            };
            let mut spans =
                vec![Span::styled(cursor.to_string(), style), Span::styled(row.label.clone(), style)];
            if let Some(desc) = &row.desc {
                spans.push(Span::styled(format!("  {desc}"), theme.muted_style()));
            }
            // S2/SYS-4: `session-selector.ts:506-508` `if (isSelected) line = theme.bg("selectedBg",
            // line);` over the WHOLE row. Upstream's row is width-wide by construction (its
            // right-hand metadata column is flush-right, S9); cyrup's is short, so pad to `width`
            // before laying the fill over every span — otherwise the bar ends raggedly mid-row and
            // a long `/resume` list gives no strong selection cue at all.
            if is_sel {
                let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                let pad = usize::from(width).saturating_sub(used);
                if pad > 0 {
                    spans.push(Span::styled(" ".repeat(pad), style));
                }
                for span in &mut spans {
                    span.style = theme.selected_bg_over(span.style);
                }
            }
            lines.push(Line::from(spans));
            if self.show_path {
                lines.push(Line::from(Span::styled(format!("    {}", row.path), theme.dim_style())));
            }
        }
        lines
    }

    /// The two hint lines, varying with the delete/rename/normal state (`session-selector.ts:155-185`).
    fn hint_lines(&self, theme: &UiTheme) -> Vec<Line<'static>> {
        if self.confirming_delete.is_some() {
            return vec![Line::from(Span::styled(
                " Delete session? enter confirm · esc cancel",
                theme.error_style(),
            ))];
        }
        if self.renaming.is_some() {
            return vec![Line::from(Span::styled(
                " New name — enter save · esc cancel",
                theme.dim_style(),
            ))];
        }
        let path_state = if self.show_path { "(on)" } else { "(off)" };
        vec![
            Line::from(Span::styled(
                " tab scope · re:<pattern> regex · \"phrase\" exact",
                theme.muted_style(),
            )),
            Line::from(Span::styled(
                format!(" ctrl+s sort · ctrl+n named · ctrl+d delete · ctrl+p path {path_state} · ctrl+r rename"),
                theme.dim_style(),
            )),
        ]
    }
}

impl Selector for SessionSelector {
    fn desired_height(&self, width: u16) -> u16 {
        let filtered = self.filtered();
        let body = self.body_lines(&UiTheme::default(), &filtered, width).len() as u16;
        // blank + top rule + blank + header + blank + search input + blank + body + hints + blank
        // + bottom rule (L4/SYS-3 — see `render`) = body + hints + 9.
        let hints = self.hint_lines(&UiTheme::default()).len() as u16;
        body.saturating_add(9).saturating_add(hints)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let filtered = self.filtered();
        // L4/SYS-3. `SessionSelectorComponent` builds its envelope in one place —
        // `buildBaseLayout` (`session-selector.ts:735-747`):
        //   `Spacer`(:737) · `DynamicBorder`(:738) · `Spacer`(:739) · header(:741) ·
        //   `Spacer`(:742) · content(:744) · `Spacer`(:745) · `DynamicBorder`(:746).
        // **Four** spacers, and note the FIRST one sits *above* the top rule — this dialog opens
        // with a blank row, unlike the extension/oauth/trust envelopes.
        //
        // The `content` child (:744) is `SessionList`, and its OWN first three lines are the search
        // `Input`, a blank (`session-selector.ts:418-419` — `lines.push("")`, "Blank line after
        // search") and then the rows. So cyrup's header block is the title row ALONE: the blank
        // between it and the search input is `buildBaseLayout`'s `:742`, and the blank cyrup
        // already drew *below* the input is `SessionList.render`'s `:419` — a different `Spacer`
        // that does not discharge `:742`. All four of `:737`, `:739`, `:742` and `:745` are added
        // here; only `:419` was already present.
        //
        // Blanks are unconditional (upstream's `Spacer` children are), and `Paragraph` clips the
        // line vector top-first, which is what pi's layout engine does to an over-tall `Container`
        // — see `crate::selector::stack_rows`' doc. A short slot therefore leads with `:737`'s
        // blank, exactly as upstream does.
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(""));
        lines.push(border_rule_line(area.width, theme));
        lines.push(Line::from(""));
        // Header: title (left) + Name/Sort (right, simplified single line).
        lines.push(Line::from(vec![
            Span::styled(
                " Resume Session",
                theme.accent_style().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   Name: {}   Sort: {}", name_label(self.name_filter), self.sort_label()),
                theme.muted_style(),
            ),
        ]));
        // `Spacer`(:742) — `buildBaseLayout` puts one between the header child and the content
        // child, and the content child is the `SessionList` whose first line is the search input.
        lines.push(Line::from(""));
        // Search / rename input (`SessionList.render`, `session-selector.ts:418`).
        if let Some((_, buf)) = &self.renaming {
            lines.push(Line::from(vec![
                Span::styled(" rename ", theme.accent_style()),
                Span::styled(buf.clone(), theme.base_style()),
            ]));
        } else {
            // Search box with a visible block cursor (feature #9 "selector IME cursor").
            let mut spans = vec![Span::styled(" > ", theme.accent_style())];
            spans.extend(crate::selector::search_input_spans(&self.query, self.cursor, theme));
            lines.push(Line::from(spans));
        }
        // The blank `SessionList.render` itself pushes after the search input (`:419`).
        lines.push(Line::from(""));
        lines.extend(self.body_lines(theme, &filtered, area.width));
        lines.extend(self.hint_lines(theme));
        // `Spacer`(:745).
        lines.push(Line::from(""));
        lines.push(border_rule_line(area.width, theme));
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // 1) Delete-confirmation state intercepts everything (`session-selector.ts:536-548`).
        if let Some(path) = self.confirming_delete.clone() {
            match keymap.action_for(key) {
                Some(SelectAction::Confirm) => {
                    self.confirming_delete = None;
                    self.rows.retain(|r| r.path != path);
                    self.clamp_selection();
                    return SelectorOutcome::Apply(SessionSelectorOutcome::delete_payload(&path));
                }
                Some(SelectAction::Cancel) => {
                    self.confirming_delete = None;
                    return SelectorOutcome::Redraw;
                }
                _ => return SelectorOutcome::Redraw,
            }
        }

        // 2) Rename-input state.
        if let Some((path, buf)) = self.renaming.clone() {
            match key.code {
                KeyCode::Enter => {
                    let name = buf.trim().to_string();
                    self.renaming = None;
                    if let Some(row) = self.rows.iter_mut().find(|r| r.path == path) {
                        row.name = Some(name.clone());
                        row.label = if name.is_empty() { row.label.clone() } else { name.clone() };
                    }
                    return SelectorOutcome::Apply(SessionSelectorOutcome::rename_payload(&path, &name));
                }
                KeyCode::Esc => {
                    self.renaming = None;
                    return SelectorOutcome::Redraw;
                }
                KeyCode::Backspace => {
                    self.backspace();
                    return SelectorOutcome::Redraw;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.insert_char(c);
                    return SelectorOutcome::Redraw;
                }
                _ => return SelectorOutcome::Redraw,
            }
        }

        // 3) Bespoke session chords (Pi defaults: ctrl+s/n/d/p/r).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = key.code
        {
            match c.to_ascii_lowercase() {
                's' => {
                    self.cycle_sort();
                    return SelectorOutcome::Redraw;
                }
                'n' => {
                    self.toggle_name_filter();
                    return SelectorOutcome::Redraw;
                }
                'p' => {
                    self.show_path = !self.show_path;
                    return SelectorOutcome::Redraw;
                }
                'd' => {
                    if let Some(row) = self.current() {
                        self.confirming_delete = Some(row.path);
                    }
                    return SelectorOutcome::Redraw;
                }
                'r' => {
                    if let Some(row) = self.current() {
                        self.renaming = Some((row.path, row.name.clone().unwrap_or_default()));
                    }
                    return SelectorOutcome::Redraw;
                }
                _ => {}
            }
        }

        // 4) Navigation / confirm / cancel.
        match keymap.action_for(key) {
            Some(SelectAction::Up) => {
                self.selected = self.selected.saturating_sub(1);
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) => {
                let len = self.filtered().len();
                if len > 0 && self.selected + 1 < len {
                    self.selected += 1;
                }
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
                Some(row) => SelectorOutcome::Confirm(row.path),
                None => SelectorOutcome::Redraw,
            },
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => {
                // 5) Printable text → search input.
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

/// The header name-filter label (`All`/`Named`, `session-selector.ts:137`).
fn name_label(filter: NameFilter) -> &'static str {
    match filter {
        NameFilter::All => "All",
        NameFilter::Named => "Named",
    }
}

/// A full-width `─` rule line (`DynamicBorder`).
fn border_rule_line(width: u16, theme: &UiTheme) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width.max(1) as usize), theme.border_style()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn row(path: &str, name: Option<&str>, text: &str, recency: u128) -> SessionRow {
        SessionRow {
            path: path.to_string(),
            label: name.unwrap_or(path).to_string(),
            name: name.map(str::to_string),
            desc: Some("3 msgs".to_string()),
            search_text: text.to_string(),
            recency,
        }
    }

    fn rows() -> Vec<SessionRow> {
        vec![
            row("/s/a.jsonl", Some("Build pipeline"), "a build pipeline ci", 3),
            row("/s/b.jsonl", None, "b fixing node cve today", 2),
            row("/s/c.jsonl", Some("Docs"), "c writing docs readme", 1),
        ]
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn typing_filters_via_the_query_dsl() {
        let mut sel = SessionSelector::new(rows());
        assert_eq!(sel.visible_len(), 3);
        let km = SelectKeymap::default();
        // A quoted phrase matches a contiguous normalized substring — only the "node cve" session.
        for c in "\"node cve\"".chars() {
            sel.handle(&key(KeyCode::Char(c)), &km);
        }
        assert_eq!(sel.visible_len(), 1);
        assert_eq!(sel.current().unwrap().path, "/s/b.jsonl");
    }

    #[test]
    fn ctrl_n_named_filter_drops_unnamed_sessions() {
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        assert_eq!(sel.name_filter(), NameFilter::All);
        sel.handle(&ctrl('n'), &km);
        assert_eq!(sel.name_filter(), NameFilter::Named);
        assert_eq!(sel.visible_len(), 2); // a + c are named; b is not
    }

    #[test]
    fn ctrl_s_cycles_sort_mode() {
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        assert_eq!(sel.sort_mode(), SortMode::Recent);
        sel.handle(&ctrl('s'), &km);
        assert_eq!(sel.sort_mode(), SortMode::Relevance);
        sel.handle(&ctrl('s'), &km);
        assert_eq!(sel.sort_mode(), SortMode::Threaded);
    }

    #[test]
    fn enter_confirms_resume_with_the_path() {
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        let out = sel.handle(&key(KeyCode::Enter), &km);
        assert_eq!(out, SelectorOutcome::Confirm("/s/a.jsonl".to_string()));
    }

    #[test]
    fn delete_flow_confirms_removes_and_emits_tagged_apply() {
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        sel.handle(&ctrl('d'), &km);
        assert!(sel.is_confirming_delete());
        let out = sel.handle(&key(KeyCode::Enter), &km);
        match out {
            SelectorOutcome::Apply(payload) => {
                assert_eq!(
                    SessionSelectorOutcome::parse_apply(&payload),
                    Some(SessionSelectorOutcome::Delete("/s/a.jsonl".to_string()))
                );
            }
            other => panic!("expected Apply, got {other:?}"),
        }
        assert!(!sel.is_confirming_delete());
        assert_eq!(sel.visible_len(), 2); // row removed
    }

    #[test]
    fn delete_esc_cancels_without_removing() {
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        sel.handle(&ctrl('d'), &km);
        sel.handle(&key(KeyCode::Esc), &km);
        assert!(!sel.is_confirming_delete());
        assert_eq!(sel.visible_len(), 3);
    }

    #[test]
    fn rename_flow_edits_and_emits_tagged_apply() {
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        sel.handle(&ctrl('r'), &km);
        assert!(sel.is_renaming());
        for c in "X".chars() {
            sel.handle(&key(KeyCode::Char(c)), &km);
        }
        let out = sel.handle(&key(KeyCode::Enter), &km);
        match out {
            SelectorOutcome::Apply(payload) => {
                assert_eq!(
                    SessionSelectorOutcome::parse_apply(&payload),
                    Some(SessionSelectorOutcome::Rename {
                        path: "/s/a.jsonl".to_string(),
                        name: "Build pipelineX".to_string(),
                    })
                );
            }
            other => panic!("expected Apply, got {other:?}"),
        }
        assert!(!sel.is_renaming());
    }

    #[test]
    fn parse_apply_ignores_a_settings_payload() {
        // A `/settings` payload `"id\u{1f}value"` must not decode as a session action.
        let payload = format!("terminal.showImages{FIELD_SEP}true");
        assert_eq!(SessionSelectorOutcome::parse_apply(&payload), None);
    }

    #[test]
    fn renders_header_search_and_rows() {
        let mut sel = SessionSelector::new(rows());
        let theme = UiTheme::default();
        let backend = TestBackend::new(60, 16);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Resume Session"));
        assert!(text.contains("Build pipeline"));
        assert!(text.contains("Sort: Recent"));
    }

    #[test]
    fn confirm_delete_hint_renders() {
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        sel.handle(&ctrl('d'), &km);
        let theme = UiTheme::default();
        let backend = TestBackend::new(60, 16);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Delete session?"));
    }
}
