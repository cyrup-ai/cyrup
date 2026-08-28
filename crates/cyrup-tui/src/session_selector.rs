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

use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{
    EditorAction, EditorKeymap, Key, SelectAction, SelectKeymap, SessionAction, SessionKeymap,
};
use crate::selector::{centered_window, rule_line, Selector, SelectorOutcome};
use crate::text_input::{Input, InputOutcome};
use crate::session_search::{filter_and_sort, NameFilter, SearchRow, SortMode};
use crate::settings_selector::FIELD_SEP;
use crate::text_width::{spans_width, str_width, truncate_spans_to_width, truncate_to_width};
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

/// Which session set the picker is showing (`SessionScope`, `session-selector.ts:24`). The header
/// radio at `:144-148` reports it and the `Tab` toggle at `:551-556` flips it
/// ([`SessionSelector::set_all_rows`] supplies the second set, upstream's `allSessionsLoader`).
/// The scope also decides `showCwd` — `const showCwd = scope === "all"` (`:844`, `:923`) — which is
/// why the cwd column in [`SessionSelector::set_session_cwds`] appears only here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionScope {
    /// Sessions rooted at the current folder (pi's `"current"`).
    #[default]
    Current,
    /// Every session on disk (pi's `"all"`).
    All,
}

/// One row of the display list plus the tree metadata pi's `FlatSessionNode`
/// (`session-selector.ts:197-203`) carries: the nesting `depth`, whether the node is the last of its
/// siblings, and for each ancestor level whether that ancestor has more siblings after it.
#[derive(Clone, Debug)]
struct FlatSessionNode {
    row: SessionRow,
    depth: usize,
    is_last: bool,
    ancestor_continues: Vec<bool>,
}

impl FlatSessionNode {
    /// A root-level (untreed) node — what `filterSessions`' non-threaded branch builds
    /// (`session-selector.ts:379-384`: `{ session, depth: 0, isLast: true, ancestorContinues: [] }`).
    fn flat(row: SessionRow) -> Self {
        FlatSessionNode { row, depth: 0, is_last: true, ancestor_continues: Vec::new() }
    }

    /// `buildTreePrefix` (`session-selector.ts:522-530`): `""` at depth 0, else one
    /// `"│  "`/`"   "` per ancestor followed by `"└─ "`/`"├─ "`.
    fn tree_prefix(&self) -> String {
        if self.depth == 0 {
            return String::new();
        }
        let mut prefix = String::new();
        for continues in &self.ancestor_continues {
            prefix.push_str(if *continues { "│  " } else { "   " });
        }
        prefix.push_str(if self.is_last { "└─ " } else { "├─ " });
        prefix
    }
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
    /// The set currently on screen — pi's `SessionList.allSessions` (`session-selector.ts:288`),
    /// i.e. whichever of [`Self::current_rows`] / [`Self::all_rows`] the live [`Self::scope`] names.
    /// Always re-derived through [`Self::resync_rows`]; never edited independently of the two caches.
    rows: Vec<SessionRow>,
    /// pi's `SessionSelectorComponent.currentSessions` (`:707`) — the `currentSessionsLoader` result,
    /// the cwd's own sessions.
    current_rows: Vec<SessionRow>,
    /// pi's `allSessions` (`:708`) — the `allSessionsLoader` result, every project's sessions.
    /// `None` is upstream's un-wired `onToggleScope` (`:551-556`): `Tab` is still SWALLOWED, it just
    /// has nothing to switch to. A host that can reach other projects calls [`Self::set_all_rows`].
    all_rows: Option<Vec<SessionRow>>,
    /// Session file path → the session's stored `cwd` (`SessionInfo.cwd`), for the `showCwd` column
    /// at `:468-470`. Fed like [`Self::set_parent_paths`] rather than carried on [`SessionRow`], so
    /// the row struct every existing caller builds stays source-compatible.
    cwds: HashMap<String, String>,
    /// The live search query — pi's embedded `Input` (`session-selector.ts:332`), the shared
    /// single-line editing surface.
    input: Input,
    sort: SortMode,
    name_filter: NameFilter,
    /// Show the full path under each row (`Ctrl+P`).
    show_path: bool,
    /// Highlighted index into the *filtered* list.
    selected: usize,
    /// When `Some(path)`, a delete confirmation is pending for that row.
    confirming_delete: Option<String>,
    /// When `Some((path, input)）`, a rename input is open for that row — pi's SECOND `Input`
    /// (`session-selector.ts:718`), so it carries a caret and the same editing surface as the search
    /// box (it used to be a bare `String` edited with `push`/`pop`, with no caret at all).
    renaming: Option<(String, Input)>,
    max_visible: usize,
    /// Which session set is on screen (drives the header radio, `session-selector.ts:144-148`).
    scope: SessionScope,
    /// The session file path of the session in progress — pi's `currentSessionCanonicalPath`
    /// (`session-selector.ts:298`, set from the `currentSessionFilePath` constructor argument at
    /// `:337`). Drives the `accent` row colour at `:489-490`.
    current_path: Option<String>,
    /// child session path → parent session path (`SessionInfo.parentSessionPath`,
    /// `session-selector.ts:222`), the edge set `buildSessionTree` walks.
    parents: HashMap<String, String>,
    /// The live `app.session.*` table. Both the handler (`:532-637`) and the header's second hint
    /// row (`:171-179`) read it, so a rebind changes the keys the dialog listens for **and** the
    /// keys it advertises.
    session_keymap: SessionKeymap,
    /// The live `tui.select.*` table, for the delete-confirmation hint's `confirm`/`cancel` keys
    /// (`:159`). Refreshed from whatever table actually routed a key, like [`crate::ListSelector`].
    select_keymap: SelectKeymap,
    /// The keys bound to `tui.input.tab`. They answer pi's `kb.matches(keyData, "tui.input.tab")`
    /// (`:551`) AND label the `scope` hint at `:170` — one source, so a rebind moves both. cyrup's
    /// editor tier owns that binding (`EditorAction::Tab`), the same source `/model`'s scope hint
    /// reads. Matching per-id like this is what upstream does; resolving the event against the whole
    /// `EditorKeymap` instead would let unrelated editor bindings fire inside a list selector.
    ///
    /// Empty means the user unbound `tui.input.tab`: `Tab` then matches nothing and the hint drops
    /// its key run, exactly as `keyText`'s `keys.length === 0` → `""` arm produces
    /// (`keybinding-hints.ts:29-36`).
    tab_keys: Vec<Key>,
    /// `showRenameHint` (`:102-104`, defaulted from `!!renameSession` at `:772`): whether the
    /// `rename` pair is appended to hint row 2 (`:177-179`). Upstream hides it when the host wired
    /// no rename callback.
    show_rename_hint: bool,
}

impl SessionSelector {
    /// Build from the resume `rows` (newest-first). Defaults to **threaded** sort / all names, like
    /// pi (`session-selector.ts:293` `private sortMode: SortMode = "threaded"`, `:706` on the
    /// component). S8: `Threaded` is a real mode now — with an empty query it groups the rows into
    /// the session DAG ([`Self::set_parent_paths`]) and draws `buildTreePrefix`'s connectors; with
    /// no edges every row is a root, so the list degrades to the same flat, newest-first order.
    pub fn new(rows: Vec<SessionRow>) -> Self {
        SessionSelector {
            current_rows: rows.clone(),
            all_rows: None,
            cwds: HashMap::new(),
            rows,
            input: Input::new(),
            sort: SortMode::Threaded,
            name_filter: NameFilter::All,
            show_path: false,
            selected: 0,
            confirming_delete: None,
            renaming: None,
            max_visible: 10,
            scope: SessionScope::Current,
            current_path: None,
            parents: HashMap::new(),
            session_keymap: SessionKeymap::default(),
            select_keymap: SelectKeymap::default(),
            tab_keys: EditorKeymap::default().keys_for(EditorAction::Tab),
            // `options?.showRenameHint ?? this.canRename` (`:772`), and cyrup's `/resume` always
            // wires the rename path (`SessionSelectorOutcome::Rename` → `rename_session`).
            show_rename_hint: true,
        }
    }

    /// Adopt the app's merged `app.session.*` + `tui.input.tab` tables, so both the handler and the
    /// header hints follow a rebind (Pi resolves `keyHint` through `getKeybindings()` on every
    /// render, `keybinding-hints.ts:34-44`).
    #[must_use]
    pub fn with_keymaps(mut self, session: &SessionKeymap, editor: &EditorKeymap) -> Self {
        self.session_keymap = session.clone();
        // The handler and the hint move together because they are one field: pi resolves BOTH
        // through the same `tui.input.tab` binding (`:551` and `:170`), so a user who rebinds tab
        // gets a picker that listens for — and advertises — the key they chose.
        self.tab_keys = editor.keys_for(EditorAction::Tab);
        self
    }

    /// `setShowRenameHint` (`session-selector.ts:102-104`): drop the `rename` pair from hint row 2
    /// when the host wired no rename callback (`:772` defaults it from `!!renameSession`).
    pub fn set_show_rename_hint(&mut self, show: bool) {
        self.show_rename_hint = show;
    }

    /// Set the parent-session edges (child path → parent path) the threaded view walks — pi reads
    /// them straight off `SessionInfo.parentSessionPath` (`session-selector.ts:222`). cyrup's
    /// `SessionInfo` carries `parent_session_path` too; feeding it here is the whole wiring.
    pub fn set_parent_paths(&mut self, edges: impl IntoIterator<Item = (String, String)>) {
        self.parents = edges.into_iter().collect();
    }

    /// Mark the session in progress (pi's `currentSessionFilePath`, `session-selector.ts:328/337`);
    /// its row renders `accent` (`:489-490`) and pi refuses to delete it (`:399-402`).
    pub fn set_current_session_path(&mut self, path: Option<String>) {
        self.current_path = path;
    }

    /// Supply the **all-projects** session set — pi's `allSessionsLoader`
    /// (`cli/session-picker.ts:15-19`, `:26-28` / `interactive-mode.ts:4787-4790`, both
    /// `SessionManager.listAll`). Wiring it is what makes `Tab` a scope toggle: upstream's handler
    /// is `if (this.onToggleScope) this.onToggleScope();` (`session-selector.ts:551-556`), so a host
    /// that never wires the second loader still SWALLOWS `Tab` — it just has nowhere to go.
    ///
    /// pi loads this set lazily on the first toggle (`toggleScope`, `:1003-1018`) because its loader
    /// is async and can report progress; cyrup's picker is fed synchronously, so the host hands both
    /// sets over up front and the "loading" header branch (`:141-143`) has nothing to report.
    pub fn set_all_rows(&mut self, rows: Vec<SessionRow>) {
        self.all_rows = Some(rows);
        self.resync_rows();
    }

    /// Session file path → that session's stored `cwd` (`SessionInfo.cwd`), for the extra right-hand
    /// column pi draws when `showCwd` is on: `rightPart = ${shortenPath(session.cwd)} ${rightPart}`
    /// (`session-selector.ts:468-470`). Only the `All` scope shows it (`showCwd = scope === "all"`,
    /// `:844`/`:923`), which is the whole point — in the merged listing a row from another project
    /// is otherwise indistinguishable from one of this folder's.
    pub fn set_session_cwds(&mut self, cwds: impl IntoIterator<Item = (String, String)>) {
        self.cwds = cwds.into_iter().collect();
    }

    /// Set the scope the header radio reports (`session-selector.ts:144-148`) and swap the row set
    /// under it — pi's `header.setScope(...)` + `sessionList.setSessions(...)` pair (`:1005-1010`,
    /// `:1021-1024`), which are never performed apart.
    pub fn set_scope(&mut self, scope: SessionScope) {
        self.scope = scope;
        self.resync_rows();
    }

    /// The scope the header radio reports (test/inspection).
    pub fn scope(&self) -> SessionScope {
        self.scope
    }

    /// `toggleScope` (`session-selector.ts:1003-1026`): `current` ⇄ `all`, re-pointing the list at
    /// the other cached set. Upstream flips `this.scope` FIRST (`:1005`) and only then decides
    /// whether the set is cached (`:1008`) or has to be loaded (`:1015-1017`) — cyrup has no async
    /// load, so the only reachable "nothing to switch to" case is a host that supplied no all-set,
    /// and that is upstream's un-wired `onToggleScope` (`:552`), where the scope does not move
    /// either because `toggleScope` is never entered.
    fn toggle_scope(&mut self) {
        if self.all_rows.is_none() {
            return;
        }
        self.scope = match self.scope {
            SessionScope::Current => SessionScope::All,
            SessionScope::All => SessionScope::Current,
        };
        self.resync_rows();
    }

    /// Re-point the display list at the set the live scope names — pi's `setSessions(sessions,
    /// showCwd)` (`:361-365`), whose `filterSessions` tail clamps the highlight into the new,
    /// possibly shorter list (`:386`).
    fn resync_rows(&mut self) {
        self.rows = match self.scope {
            SessionScope::Current => self.current_rows.clone(),
            SessionScope::All => self.all_rows.clone().unwrap_or_default(),
        };
        self.clamp_selection();
    }

    /// The filtered + sorted display list for the current query/sort/name-filter (clones for borrow
    /// ease; the list is small).
    ///
    /// `filterSessions` (`session-selector.ts:367-387`) name-filters first, then branches: in
    /// **threaded** mode with an empty query it builds and flattens the session tree
    /// (`:372-375`); every other mode — and any mode once a query is typed — is a flat list of
    /// depth-0 nodes (`:377-385`).
    fn filtered(&self) -> Vec<FlatSessionNode> {
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
        let idxs =
            filter_and_sort(&search_rows, self.input.value(), self.sort, self.name_filter);
        let rows: Vec<SessionRow> =
            idxs.into_iter().filter_map(|i| self.rows.get(i).cloned()).collect();
        if self.sort == SortMode::Threaded && self.input.value().trim().is_empty() {
            flatten_session_tree(&rows, &self.parents)
        } else {
            rows.into_iter().map(FlatSessionNode::flat).collect()
        }
    }

    /// The highlighted row of the *filtered* list, if any (test/inspection).
    pub fn current(&self) -> Option<SessionRow> {
        self.filtered().into_iter().nth(self.selected).map(|n| n.row)
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

    /// Drop a deleted session from **both** cached sets, then re-derive the display list — pi's
    /// `onDeleteSession` filters `currentSessions` AND `allSessions` before re-setting the list
    /// (`session-selector.ts:835-845` — `:836-841` filters, `:845` re-sets). Filtering only the visible set would resurrect the row on
    /// the next `Tab`.
    fn remove_row(&mut self, path: &str) {
        self.current_rows.retain(|r| r.path != path);
        if let Some(all) = self.all_rows.as_mut() {
            all.retain(|r| r.path != path);
        }
        self.resync_rows();
    }

    /// Apply a rename to **both** cached sets, for the same reason [`Self::remove_row`] does.
    /// (pi reloads the scope instead — `refreshSessionsAfterMutation`, `:999-1001`; cyrup's picker
    /// has no loader to re-run, so it patches the caches it was handed.)
    fn apply_rename(&mut self, path: &str, name: &str) {
        let patch = |rows: &mut Vec<SessionRow>| {
            if let Some(row) = rows.iter_mut().find(|r| r.path == path) {
                row.name = Some(name.to_string());
                if !name.is_empty() {
                    row.label = name.to_string();
                }
            }
        };
        patch(&mut self.current_rows);
        if let Some(all) = self.all_rows.as_mut() {
            patch(all);
        }
        self.resync_rows();
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

    /// `keyText("app.session.toggleNamedFilter")` (`session-selector.ts:424`) — the key the empty
    /// state tells the user to press. Empty when the action is unbound, exactly as `keyText`'s
    /// `keys.length === 0` arm returns `""` (`keybinding-hints.ts:30`).
    fn named_filter_key(&self) -> String {
        self.session_keymap.keys_label(SessionAction::ToggleNamedFilter).unwrap_or_default()
    }

    /// Build the body display lines for the windowed filtered list at `width` columns.
    ///
    /// `width` is needed for the `selectedBg` fill (S2): `session-selector.ts:505-509` builds the
    /// row as `leftPart + " ".repeat(spacing) + styledRight` where
    /// `spacing = max(1, width - leftWidth - rightWidth)`, so the string the fill wraps spans the
    /// whole row — the selection reads as a full-width bar, not a ragged one ending at the label.
    fn body_lines(
        &self,
        theme: &UiTheme,
        filtered: &[FlatSessionNode],
        width: u16,
    ) -> Vec<Line<'static>> {
        if filtered.is_empty() {
            // `:421-438` — **four** branches, not one. Three of them tell the user how to widen
            // the search, and the one cyrup always rendered ("  No sessions found") is the only
            // one that does not: it is the `showCwd` (= scope "all") + unfiltered-names case, i.e.
            // the one where there genuinely is nothing left to widen to. `showCwd` is
            // `scope === "all"` (`:844`, `:923`).
            let all_scope = self.scope == SessionScope::All;
            let message = match (self.name_filter, all_scope) {
                // `:424` — `keyText("app.session.toggleNamedFilter")`, resolved from the live
                // table. `Tab` at `:428`/`:435` IS a literal upstream, so it stays one here.
                (NameFilter::Named, true) => format!(
                    "  No named sessions found. Press {} to show all.",
                    self.named_filter_key()
                ),
                (NameFilter::Named, false) => format!(
                    "  No named sessions in current folder. Press {} to show all, or Tab to view all.",
                    self.named_filter_key()
                ),
                (NameFilter::All, true) => "  No sessions found".to_string(),
                (NameFilter::All, false) => {
                    "  No sessions in current folder. Press Tab to view all.".to_string()
                }
            };
            // `:437` `theme.fg("muted", truncateToWidth(emptyMessage, width, "…"))`.
            return vec![Line::from(Span::styled(
                truncate_to_width(&message, usize::from(width), "…"),
                theme.muted_style(),
            ))];
        }
        let width = usize::from(width);
        let len = filtered.len();
        let (start, end) = centered_window(self.selected, len, self.max_visible);
        let mut lines = Vec::new();
        for (i, node) in filtered.iter().enumerate().take(end).skip(start) {
            let row = &node.row;
            let is_sel = i == self.selected;
            let is_confirming = self.confirming_delete.as_deref() == Some(row.path.as_str());
            let is_current = self.current_path.as_deref() == Some(row.path.as_str());

            // S8 (`:457`, `:522-530`): the tree connectors, drawn `theme.fg("dim", prefix)` (`:500`).
            let prefix = node.tree_prefix();

            // S14 (`:464-473`): the right-hand column is `"<msgCount> <age>"`, and the `Ctrl+P` path
            // toggle **prepends the path to that same column** — pi never grows a second line per
            // session, so toggling the path does not halve how many sessions fit.
            // `:464-473` builds the column back-to-front — `"<msgCount> <age>"`, then the cwd is
            // PREPENDED when `showCwd` (`:468-470`), then the path when `showPath` (`:471-473`) —
            // so left-to-right the order is path, cwd, counts. `showCwd` is `scope === "all"`
            // (`:844`, `:923`): in the merged listing this column is the only thing that says which
            // project a row belongs to.
            let mut right = String::new();
            if self.show_path {
                right.push_str(&shorten_path(&row.path));
            }
            if self.scope == SessionScope::All
                && let Some(cwd) = self.cwds.get(&row.path)
                // `if (this.showCwd && session.cwd)` (`:468`) — an empty cwd draws no column.
                && !cwd.is_empty()
            {
                if !right.is_empty() {
                    right.push(' ');
                }
                right.push_str(&shorten_path(cwd));
            }
            if let Some(desc) = &row.desc {
                if !right.is_empty() {
                    right.push(' ');
                }
                right.push_str(desc);
            }

            // S10: the cursor glyph is U+203A `› ` (`session-selector.ts:476`
            // `isSelected ? theme.fg("accent", "› ") : "  "`), not U+2192 `→ `. `→ ` is correct in
            // `SelectList` (`select-list.ts:146`) — only this selector diverged.
            let cursor = if is_sel { "› " } else { "  " };
            let cursor_style = if is_sel { theme.accent_style() } else { theme.base_style() };

            // `:462` normalizes control characters out of the label before measuring it.
            let normalized = normalize_message(&row.label);
            // `:479-483`: the message gets whatever the cursor, prefix and right column leave, but
            // never less than 10 columns.
            let prefix_width = str_width(&prefix);
            let right_width = str_width(&right).saturating_add(2); // +2 for spacing (`:480`)
            let available = width.saturating_sub(2).saturating_sub(prefix_width).saturating_sub(right_width);
            let message = truncate_to_width(&normalized, available.max(10), "…");

            // S11 (`:486-497`): the row's state picks the message colour — `error` while its delete
            // is queued, `accent` for the session you are in, `warning` for a named session — and
            // selection then applies `theme.bold` ON TOP, rather than replacing the colour.
            let mut message_style = if is_confirming {
                theme.error_style()
            } else if is_current {
                theme.accent_style()
            } else if row.name.is_some() {
                theme.warning_style()
            } else {
                theme.base_style()
            };
            if is_sel {
                message_style = message_style.add_modifier(Modifier::BOLD);
            }

            // S9 (`:499-505`): `leftPart + " ".repeat(max(1, width - leftWidth - rightWidth))
            // + theme.fg(isConfirmingDelete ? "error" : "dim", rightPart)` — the metadata column is
            // flush right and `dim`, not a fixed two-space gap in `muted`.
            let mut spans = vec![
                Span::styled(cursor.to_string(), cursor_style),
                Span::styled(prefix, theme.dim_style()),
                Span::styled(message, message_style),
            ];
            let left_width = spans_width(&spans);
            let spacing =
                width.saturating_sub(left_width).saturating_sub(str_width(&right)).max(1);
            spans.push(Span::styled(" ".repeat(spacing), theme.base_style()));
            spans.push(Span::styled(
                right,
                if is_confirming { theme.error_style() } else { theme.dim_style() },
            ));

            // `:509` `truncateToWidth(line, width)` — the default `"..."` ellipsis.
            let mut spans = truncate_spans_to_width(spans, width, "...");
            // S2/SYS-4: `:506-508` `if (isSelected) line = theme.bg("selectedBg", line);` over the
            // WHOLE row. Upstream's row is width-wide by construction (the flush-right metadata
            // column above), but pad defensively so a short row still reads as a full-width bar.
            if is_sel {
                let pad = width.saturating_sub(spans_width(&spans));
                if pad > 0 {
                    spans.push(Span::styled(" ".repeat(pad), theme.base_style()));
                }
                for span in &mut spans {
                    span.style = theme.selected_bg_over(span.style);
                }
            }
            lines.push(Line::from(spans));
        }
        // S15 (`:512-517`): a `muted` `  (i/N)` row whenever the window does not cover the list.
        if start > 0 || end < len {
            let scroll = format!("  ({}/{})", self.selected.saturating_add(1), len);
            lines.push(Line::from(Span::styled(
                truncate_to_width(&scroll, width, ""),
                theme.muted_style(),
            )));
        }
        lines
    }

    /// S12 — the header's first line (`SessionSelectorHeader.render`, `session-selector.ts:130-153`
    /// and `:185`).
    ///
    /// Upstream is a two-part, width-justified row, and every part of it differs from what cyrup
    /// drew:
    ///
    /// * the title (`:131-132`) NAMES THE SCOPE — `"Resume Session (Current Folder)"` /
    ///   `"Resume Session (All)"` — and is `theme.bold(title)` with **no `fg`**, i.e. plain bold,
    ///   not accent teal;
    /// * the right group (`:150`) is `scopeText  nameText  sortText`, flush right, where the scope
    ///   is a radio pair (`:144-148`, `◉`/`○`, the live side `accent` and the other `muted`) and
    ///   `Name:`/`Sort:` are a `muted` label with an `accent` value (`:135-138`);
    /// * the gap is `max(0, width - w(left) - w(right))` after the title has itself been truncated
    ///   to `max(0, width - w(right) - 1)` (`:151-153`), so the right group wins the width fight.
    ///
    /// cyrup drew an accent-teal `" Resume Session"` followed by one flat `muted`
    /// `"   Name: …   Sort: …"` left-packed against it: no scope anywhere, no accent feedback on
    /// the two toggles, and the metadata never reached the right edge.
    fn header_line(&self, theme: &UiTheme, width: u16) -> Line<'static> {
        let width = usize::from(width);
        let title = match self.scope {
            SessionScope::Current => "Resume Session (Current Folder)",
            SessionScope::All => "Resume Session (All)",
        };
        // `:144-148` — the scope radio. (pi's third branch, `:141-143`, reports an in-flight
        // `Loading n/m`; cyrup's chrome hands the selector an already-loaded row list, so there is
        // no loading state to report.)
        let (scope_spans, name_value, sort_value) = {
            let scope = match self.scope {
                SessionScope::Current => vec![
                    Span::styled("◉ Current Folder", theme.accent_style()),
                    Span::styled(" | ○ All", theme.muted_style()),
                ],
                SessionScope::All => vec![
                    Span::styled("○ Current Folder | ", theme.muted_style()),
                    Span::styled("◉ All", theme.accent_style()),
                ],
            };
            (scope, name_label(self.name_filter), self.sort_label())
        };
        let mut right: Vec<Span<'static>> = scope_spans;
        right.push(Span::styled("  Name: ", theme.muted_style()));
        right.push(Span::styled(name_value, theme.accent_style()));
        right.push(Span::styled("  Sort: ", theme.muted_style()));
        right.push(Span::styled(sort_value, theme.accent_style()));
        // `:150` `truncateToWidth(…, width, "")` — an empty ellipsis, so it clips rather than eliding.
        let right = truncate_spans_to_width(right, width, "");
        let right_width = spans_width(&right);

        // `:151-152`.
        let available_left = width.saturating_sub(right_width).saturating_sub(1);
        let left = truncate_to_width(title, available_left, "");
        // `:153`.
        let spacing = width.saturating_sub(str_width(&left)).saturating_sub(right_width);

        // `theme.bold(title)` — bold with no `fg` escape, so the title keeps the default text
        // colour rather than becoming accent teal.
        let mut spans =
            vec![Span::styled(left, theme.base_style().add_modifier(Modifier::BOLD))];
        if spacing > 0 {
            spans.push(Span::styled(" ".repeat(spacing), theme.base_style()));
        }
        spans.extend(right);
        Line::from(spans)
    }

    /// The header's **hint rows** — `SessionSelectorHeader.render`'s `hintLine1`/`hintLine2`
    /// (`session-selector.ts:155-183`), which `:185` returns as lines 2 and 3 of the SAME header
    /// child. They are part of the header, not a footer: see [`Selector::render`] for the position.
    ///
    /// Three things upstream does that cyrup's flat `dim`/`muted` strings did not:
    ///
    /// * **Two-tone per pair.** `keyHint(kb, desc)` is `theme.fg("dim", keyText(kb)) +
    ///   theme.fg("muted", " " + desc)` (`keybinding-hints.ts:42-44`) — a dim KEY and a muted
    ///   DESCRIPTION, not one colour across the row.
    /// * **Keys are resolved, never spelled.** `keyText` joins every key bound to the id with `/`
    ///   (`:29-36`); a user who rebinds `app.session.delete` sees their own key here.
    /// * **`showRenameHint` gates the last pair** (`:177-179`).
    ///
    /// Colour detail on the separator: `sep` is `theme.fg("muted", " · ")` (`:168`) in the normal
    /// branch, but the delete-confirmation string at `:159` interpolates two `keyHint`s into a
    /// plain template and only THEN wraps the whole thing in `theme.fg("error", …)` (`:160`).
    /// `theme.fg` closes with `\x1b[39m` — a foreground reset, not a restore (`theme.ts:372-376`) —
    /// so everything after the first inner `keyHint` loses the error colour: `Delete session? ` is
    /// `error`, each key is `dim`, each description `muted`, and the literal ` · ` between them
    /// falls back to the default foreground.
    ///
    /// Always **two** rows in the list/confirm states, because `:161`/`:165` set `hintLine2 = ""`
    /// rather than dropping the row — the blank is what keeps every row below at a fixed offset.
    fn hint_lines(&self, theme: &UiTheme, width: u16) -> Vec<Line<'static>> {
        let width = usize::from(width);
        // `:181-182` / `:160` / `:164` — every branch truncates to `width` with a `…` ellipsis
        // (`:150`'s empty-ellipsis clip is the TITLE row's, not these).
        let clip = |spans: Vec<Span<'static>>| {
            Line::from(truncate_spans_to_width(spans, width, "…"))
        };
        let pair = |key: Option<String>, desc: &str| -> Vec<Span<'static>> {
            match key {
                // `keys.length === 0 ⇒ ""` (`keybinding-hints.ts:30`): an unbound id still renders
                // its description, behind an empty key run.
                Some(key) => crate::chrome::key_hint_spans(&key, desc, theme),
                None => vec![Span::styled(format!(" {desc}"), theme.muted_style())],
            }
        };
        let sep = || Span::styled(" · ", theme.muted_style());

        // `:158-161`.
        if self.confirming_delete.is_some() {
            let mut spans = vec![Span::styled("Delete session? ", theme.error_style())];
            spans.extend(pair(self.select_keymap.keys_label(SelectAction::Confirm), "confirm"));
            // Uncoloured: the preceding `keyHint` already emitted `\x1b[39m`.
            spans.push(Span::styled(" · ", theme.base_style()));
            spans.extend(pair(self.select_keymap.keys_label(SelectAction::Cancel), "cancel"));
            return vec![clip(spans), Line::from("")];
        }
        // cyrup-only state: the rename `Input` replaces the search box. Upstream reaches the same
        // affordance through `buildBaseLayout(panel, { showHeader: false })` (`:885`), whose panel
        // carries its own `${keyText(confirm)} to save · ${keyText(cancel)} to cancel` row (`:879`);
        // this keeps that text on the header's own hint row instead of duplicating the panel.
        if self.renaming.is_some() {
            let confirm = self.select_keymap.keys_label(SelectAction::Confirm).unwrap_or_default();
            let cancel = self.select_keymap.keys_label(SelectAction::Cancel).unwrap_or_default();
            return vec![
                clip(vec![Span::styled(
                    format!("{confirm} to save · {cancel} to cancel"),
                    theme.muted_style(),
                )]),
                Line::from(""),
            ];
        }

        // `:167-182`.
        let path_state = if self.show_path { "(on)" } else { "(off)" };
        // `keyHint("tui.input.tab", "scope")` (`:170`) — every key bound to the id, joined `/`, or
        // no key run at all when the user unbound it (`keyText`'s `keys.length === 0` arm).
        let tab_label = (!self.tab_keys.is_empty())
            .then(|| self.tab_keys.iter().map(|k| k.label()).collect::<Vec<_>>().join("/"));
        let mut hint1 = pair(tab_label, "scope");
        hint1.push(sep());
        hint1.push(Span::styled(
            "re:<pattern> regex · \"phrase\" exact",
            theme.muted_style(),
        ));

        let k = |a: SessionAction| self.session_keymap.keys_label(a);
        let mut hint2 = pair(k(SessionAction::ToggleSort), "sort");
        hint2.push(sep());
        hint2.extend(pair(k(SessionAction::ToggleNamedFilter), "named"));
        hint2.push(sep());
        hint2.extend(pair(k(SessionAction::Delete), "delete"));
        hint2.push(sep());
        hint2.extend(pair(k(SessionAction::TogglePath), &format!("path {path_state}")));
        if self.show_rename_hint {
            hint2.push(sep());
            hint2.extend(pair(k(SessionAction::Rename), "rename"));
        }
        vec![clip(hint1), clip(hint2)]
    }
}

impl Selector for SessionSelector {
    fn desired_height(&self, width: u16) -> u16 {
        let filtered = self.filtered();
        let body = self.body_lines(UiTheme::default_ref(), &filtered, width).len() as u16;
        // blank + top rule + blank + header (title + BOTH hint rows) + blank + search input + blank
        // + body + blank + bottom rule (L4/SYS-3 — see `render`) = body + hints + 9.
        let hints = self.hint_lines(UiTheme::default_ref(), width).len() as u16;
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
        // The header child (:741) is `SessionSelectorHeader`, and `render` returns **THREE** lines —
        // `return [\`${left}${" ".repeat(spacing)}${rightText}\`, hintLine1, hintLine2]` (`:185`).
        // The `content` child (:744) is `SessionList`, whose own first two lines are the search
        // `Input` and a blank (`:418-419` — `lines.push("")`, "Blank line after search").
        //
        // So the row order is fixed: title, hint1, hint2, `Spacer`(:742), search box, `:419`'s
        // blank, rows. cyrup emitted the `:742` blank and the search box where hint1/hint2 belong
        // and hung the hints off the bottom of the body, which shifted every row below the header
        // by two and put the keyboard help two blocks away from the keys it describes.
        //
        // Blanks are unconditional (upstream's `Spacer` children are), and a `Paragraph` draws
        // `lines[0..area.height]` — it drops the TRAILING rows, so a short slot shows a strict
        // PREFIX of this vector, which is what pi's layout engine does to an over-tall `Container`
        // (see `crate::selector::stack_rows`' doc). A short slot therefore leads with `:737`'s
        // blank, exactly as upstream does.
        //
        // S13: both of `/resume`'s rules are constructed
        // `new DynamicBorder((s) => theme.fg("accent", s))` (`session-selector.ts:738` and
        // `:746`) — this is the one selector upstream deliberately frames in **accent**, where
        // the rest pass the `border` token. cyrup framed it with `theme.border_style()`. Hence
        // `rule_line(.., accent)` below rather than the shared `border_rule_line` every other
        // envelope uses.
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(""),                               // `Spacer`(:737)
            rule_line(area.width, theme.accent_style()),  // `DynamicBorder`(:738), accent (S13)
            Line::from(""),                               // `Spacer`(:739)
            self.header_line(theme, area.width),          // header line 1 of 3 (:185)
        ];
        // Header lines 2 and 3 (`hintLine1`/`hintLine2`, `:156-183`).
        lines.extend(self.hint_lines(theme, area.width));
        // `Spacer`(:742) — `buildBaseLayout` puts one between the header child and the content
        // child, and the content child is the `SessionList` whose first line is the search input.
        lines.push(Line::from(""));
        // Search / rename input (`SessionList.render`, `session-selector.ts:418`).
        if let Some((_, edit)) = &self.renaming {
            // The accent ` rename ` label, then the `Input`'s own value + caret; the label eats
            // eight columns, so the value gets the rest.
            let mut spans = vec![Span::styled(" rename ", theme.accent_style())];
            spans.extend(crate::selector::search_input_spans(
                edit.value(),
                edit.cursor(),
                usize::from(area.width).saturating_sub(8),
                theme,
            ));
            lines.push(Line::from(spans));
        } else {
            // Search box with a visible block cursor (feature #9 "selector IME cursor").
            //
            // S31: `SessionList.render` splices the `Input`'s own lines in unmodified —
            // `lines.push(...this.searchInput.render(width))` (`session-selector.ts:418`) — so the
            // row is `Input.render`'s shared, unstyled `"> "` at column 0 (`input.ts:380`). cyrup
            // drew an accent `" > "`: one column in, and coloured.
            lines.push(Line::from(crate::selector::input_line_spans(
                self.input.value(),
                self.input.cursor(),
                area.width,
                theme,
            )));
        }
        // The blank `SessionList.render` itself pushes after the search input (`:419`).
        lines.push(Line::from(""));
        lines.extend(self.body_lines(theme, &filtered, area.width));
        // `Spacer`(:745).
        lines.push(Line::from(""));
        // `DynamicBorder`(:746) — accent, like `:738` above (S13).
        lines.push(rule_line(area.width, theme.accent_style()));
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Keep the delete-confirmation hint honest even for a selector built without
        // `with_keymaps`: adopt whatever `tui.select.*` table actually routed this key (the same
        // discipline `ListSelector::handle` uses).
        self.select_keymap = keymap.clone();
        // 1) Delete-confirmation state intercepts everything (`session-selector.ts:536-548`).
        if let Some(path) = self.confirming_delete.clone() {
            match keymap.action_for(key) {
                Some(SelectAction::Confirm) => {
                    self.confirming_delete = None;
                    self.remove_row(&path);
                    return SelectorOutcome::Apply(SessionSelectorOutcome::delete_payload(&path));
                }
                Some(SelectAction::Cancel) => {
                    self.confirming_delete = None;
                    return SelectorOutcome::Redraw;
                }
                _ => return SelectorOutcome::Redraw,
            }
        }

        // 2) Rename-input state. Enter/Esc stay here (upstream's rename panel owns them,
        // `session-selector.ts:879`); everything else is the embedded `Input`'s editing surface.
        if self.renaming.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let Some((path, edit)) = self.renaming.take() else {
                        return SelectorOutcome::Redraw;
                    };
                    let name = edit.value().trim().to_string();
                    self.apply_rename(&path, &name);
                    return SelectorOutcome::Apply(SessionSelectorOutcome::rename_payload(&path, &name));
                }
                KeyCode::Esc => {
                    self.renaming = None;
                    return SelectorOutcome::Redraw;
                }
                _ => {
                    if let Some((_, edit)) = self.renaming.as_mut() {
                        edit.handle_key(key);
                    }
                    return SelectorOutcome::Redraw;
                }
            }
        }

        // 3) `tui.input.tab` → the scope toggle (`session-selector.ts:551-556`). Upstream asks this
        // ONE editor-tier id here, ahead of every `app.session.*` chord and after the
        // delete-confirmation intercept, and `return`s whether or not a toggle is wired — so `Tab`
        // never falls through to the search input. Order matters: a user who rebinds an
        // `app.session.*` action onto tab would, in pi, still get the scope toggle.
        if self.tab_keys.iter().any(|k| k.matches(key)) {
            self.toggle_scope();
            return SelectorOutcome::Redraw;
        }

        // 4) The `app.session.*` chords, resolved through the live table rather than matched as
        // literal `ctrl+…` chars (R-10-018) — this is the same table the hint rows name, so the two
        // cannot drift apart.
        if let Some(action) = self.session_keymap.action_for(key) {
            match action {
                SessionAction::ToggleSort => self.cycle_sort(),
                SessionAction::ToggleNamedFilter => self.toggle_name_filter(),
                SessionAction::TogglePath => self.show_path = !self.show_path,
                SessionAction::Delete => {
                    if let Some(row) = self.current() {
                        self.confirming_delete = Some(row.path);
                    }
                }
                SessionAction::Rename => {
                    if let Some(row) = self.current() {
                        self.renaming =
                            Some((row.path, Input::with_value(row.name.clone().unwrap_or_default())));
                    }
                }
            }
            return SelectorOutcome::Redraw;
        }

        // 5) Navigation / confirm / cancel.
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
            // 6) Everything else → the search `Input` (`session-selector.ts:565-567`).
            None => match self.input.handle_key(key) {
                InputOutcome::Edited => {
                    self.selected = 0;
                    SelectorOutcome::Redraw
                }
                InputOutcome::Moved => SelectorOutcome::Redraw,
                InputOutcome::Ignored => SelectorOutcome::Ignored,
            },
        }
    }

    /// Feed the live editor table to BOTH embedded inputs (`session-selector.ts:332` and `:718`).
    fn set_editor_keymap(&mut self, keymap: &EditorKeymap) {
        self.input.set_editor_keymap(keymap);
        if let Some((_, edit)) = self.renaming.as_mut() {
            edit.set_editor_keymap(keymap);
        }
    }

    /// Route a bracketed paste to whichever input is focused — the rename field when it is open,
    /// otherwise the search box.
    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        if let Some((_, edit)) = self.renaming.as_mut() {
            edit.paste(text);
            return SelectorOutcome::Redraw;
        }
        self.input.paste(text);
        self.selected = 0;
        SelectorOutcome::Redraw
    }
}

/// The header name-filter label (`All`/`Named`, `session-selector.ts:137`).
fn name_label(filter: NameFilter) -> &'static str {
    match filter {
        NameFilter::All => "All",
        NameFilter::Named => "Named",
    }
}

/// `shortenPath` (`session-selector.ts:26-33`): rewrite a `$HOME`-rooted path to `~/…`. (pi keeps
/// its own copy of this in the session selector rather than importing `render-utils`'.)
fn shorten_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// `displayText.replace(/[\x00-\x1f\x7f]/g, " ").trim()` (`session-selector.ts:462`).
fn normalize_message(s: &str) -> String {
    let replaced: String =
        s.chars().map(|c| if c.is_control() || c == '\u{7f}' { ' ' } else { c }).collect();
    replaced.trim().to_string()
}

/// S8 — `buildSessionTree` (`session-selector.ts:209-254`) followed by `flattenSessionTree`
/// (`:259-278`), fused into one pass over `rows`.
///
/// `parents` is the child-path → parent-path edge set. A session whose parent is missing from the
/// list is a root (`:224-228`). Each subtree's `latestActivity` is the max `recency` over it
/// (`:231-242`) and roots and children alike are ordered by it, descending (`:245-251`). The walk
/// (`:262-271`) is pre-order, and an ancestor "continues" only when it is a non-root with siblings
/// left after it — the `depth > 0 ? !isLast : false` at `:268`.
///
/// **[CYRUP-DELTA]** upstream's `walk` is recursive and a `parentSessionPath` cycle would blow the
/// stack; this rejects any edge whose parent already descends from the child (leaving the child a
/// root), so the structure is a forest by construction and the walk is an explicit-stack DFS.
fn flatten_session_tree(
    rows: &[SessionRow],
    parents: &HashMap<String, String>,
) -> Vec<FlatSessionNode> {
    let n = rows.len();
    let mut by_path: HashMap<&str, usize> = HashMap::with_capacity(n);
    for (i, row) in rows.iter().enumerate() {
        by_path.entry(row.path.as_str()).or_insert(i);
    }

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut parent_of: Vec<Option<usize>> = vec![None; n];
    let mut roots: Vec<usize> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let parent = parents
            .get(&row.path)
            .and_then(|p| by_path.get(p.as_str()).copied())
            .filter(|p| *p != i && !descends_from(&parent_of, *p, i));
        match parent {
            Some(p) => {
                if let Some(kids) = children.get_mut(p) {
                    kids.push(i);
                }
                if let Some(slot) = parent_of.get_mut(i) {
                    *slot = Some(p);
                }
            }
            None => roots.push(i),
        }
    }

    // `updateLatestActivity` (`:231-242`), bottom-up over a parents-before-children ordering.
    let mut latest: Vec<u128> = rows.iter().map(|r| r.recency).collect();
    let mut order: Vec<usize> = roots.clone();
    let mut cursor = 0usize;
    while let Some(&idx) = order.get(cursor) {
        cursor += 1;
        if let Some(kids) = children.get(idx) {
            order.extend(kids.iter().copied());
        }
    }
    for &idx in order.iter().rev() {
        let (Some(p), Some(&child_latest)) = (parent_of.get(idx).copied().flatten(), latest.get(idx))
        else {
            continue;
        };
        if let Some(slot) = latest.get_mut(p) {
            *slot = (*slot).max(child_latest);
        }
    }

    // `sortNodes` (`:245-251`): latest activity descending, at every level.
    let by_latest = |a: &usize, b: &usize| {
        latest.get(*b).copied().unwrap_or(0).cmp(&latest.get(*a).copied().unwrap_or(0))
    };
    roots.sort_by(by_latest);
    for kids in &mut children {
        kids.sort_by(by_latest);
    }

    // `walk` (`:262-275`), pre-order with an explicit stack.
    struct Frame {
        idx: usize,
        depth: usize,
        is_last: bool,
        ancestor_continues: Vec<bool>,
    }
    let root_count = roots.len();
    let mut stack: Vec<Frame> = roots
        .iter()
        .enumerate()
        .rev()
        .map(|(i, &idx)| Frame {
            idx,
            depth: 0,
            is_last: i + 1 == root_count,
            ancestor_continues: Vec::new(),
        })
        .collect();
    let mut out: Vec<FlatSessionNode> = Vec::with_capacity(n);
    while let Some(frame) = stack.pop() {
        let Some(row) = rows.get(frame.idx) else { continue };
        out.push(FlatSessionNode {
            row: row.clone(),
            depth: frame.depth,
            is_last: frame.is_last,
            ancestor_continues: frame.ancestor_continues.clone(),
        });
        let Some(kids) = children.get(frame.idx) else { continue };
        // `:268` — a root's own branch never draws a continuation line under it.
        let continues = if frame.depth > 0 { !frame.is_last } else { false };
        let mut ancestors = frame.ancestor_continues;
        ancestors.push(continues);
        let kid_count = kids.len();
        for (i, &kid) in kids.iter().enumerate().rev() {
            stack.push(Frame {
                idx: kid,
                depth: frame.depth.saturating_add(1),
                is_last: i + 1 == kid_count,
                ancestor_continues: ancestors.clone(),
            });
        }
    }
    out
}

/// Whether `node` is `ancestor`'s descendant along the edges recorded so far — the cycle guard
/// [`flatten_session_tree`] uses. Bounded by `parent_of.len()` hops.
fn descends_from(parent_of: &[Option<usize>], node: usize, ancestor: usize) -> bool {
    let mut cur = Some(node);
    for _ in 0..parent_of.len() {
        match cur {
            Some(i) if i == ancestor => return true,
            Some(i) => cur = parent_of.get(i).copied().flatten(),
            None => return false,
        }
    }
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::keymap::Key;

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
        // `session-selector.ts:293`/`:706` — the selector OPENS in `threaded`, and `:986` cycles
        // threaded → recent → relevance → threaded.
        assert_eq!(sel.sort_mode(), SortMode::Threaded);
        sel.handle(&ctrl('s'), &km);
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
        // 100 columns: at 60 the header's right group (`◉ Current Folder | ○ All  Name: All
        // Sort: Threaded`, 51 cols) leaves `availableLeft = 8` and pi CLIPS the title to
        // `"Resume S"` (`session-selector.ts:151-152`), which is upstream behaviour, not a bug.
        let backend = TestBackend::new(100, 16);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Resume Session (Current Folder)"));
        assert!(text.contains("Build pipeline"));
        assert!(text.contains("Sort: Threaded"));
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

    // ---------------------------------------------------------------------------------------
    // S8/S9/S11/S12/S13/S14/S15 — per-row and per-header completeness against
    // `session-selector.ts` @ v0.84.1.
    // ---------------------------------------------------------------------------------------

    fn draw(sel: &mut SessionSelector, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let theme = UiTheme::default();
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
        term.backend().buffer().clone()
    }

    fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn find_row(buf: &ratatui::buffer::Buffer, needle: &str) -> u16 {
        (0..buf.area.height)
            .find(|y| row_text(buf, *y).contains(needle))
            .unwrap_or_else(|| panic!("no row contains {needle:?}"))
    }

    /// The **column** `needle` starts at on row `y`. `str::find` returns a BYTE offset, which stops
    /// agreeing with the column the moment a multi-byte glyph (`◉`, `├`) precedes the match — this
    /// walks cells instead.
    fn col_of(buf: &ratatui::buffer::Buffer, y: u16, needle: &str) -> u16 {
        let cells: Vec<String> =
            (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        for x in 0..cells.len() {
            let tail: String = cells[x..].concat();
            if tail.starts_with(needle) {
                return x as u16;
            }
        }
        panic!("row {y} does not contain {needle:?}: {:?}", row_text(buf, y));
    }

    /// A four-session fixture whose parent edges make a two-level tree: `b` and `c` are children of
    /// `a`, `d` is a child of `b`. Recencies are chosen so the `latestActivity` ordering
    /// (`session-selector.ts:245-251`) puts `b` before `c`.
    fn tree_selector() -> SessionSelector {
        let mut sel = SessionSelector::new(vec![
            row("/s/a.jsonl", Some("Root run"), "a root", 4),
            row("/s/b.jsonl", Some("Branch one"), "b one", 3),
            row("/s/c.jsonl", Some("Branch two"), "c two", 2),
            row("/s/d.jsonl", Some("Leaf"), "d leaf", 1),
        ]);
        sel.set_parent_paths([
            ("/s/b.jsonl".to_string(), "/s/a.jsonl".to_string()),
            ("/s/c.jsonl".to_string(), "/s/a.jsonl".to_string()),
            ("/s/d.jsonl".to_string(), "/s/b.jsonl".to_string()),
        ]);
        sel
    }

    /// **S8.** `buildTreePrefix` (`session-selector.ts:522-530`) —
    /// `ancestorContinues.map(c => c ? "│  " : "   ").join("") + (isLast ? "└─ " : "├─ ")`, drawn
    /// `theme.fg("dim", prefix)` at `:500`, over the pre-order walk of `flattenSessionTree`
    /// (`:259-278`).
    ///
    /// FAILS before the fix: `grep '├\|└\|│' session_selector.rs` found nothing — `/resume` was a
    /// flat list in every mode.
    #[test]
    fn threaded_mode_draws_pis_tree_connectors() {
        let mut sel = tree_selector();
        let buf = draw(&mut sel, 60, 20);
        let a = find_row(&buf, "Root run");
        // Pre-order: a, b, d (b's child), c — `:262-271`.
        assert_eq!(find_row(&buf, "Branch one"), a + 1);
        assert_eq!(find_row(&buf, "Leaf"), a + 2);
        assert_eq!(find_row(&buf, "Branch two"), a + 3);

        assert!(!row_text(&buf, a).contains('├'), "a root carries no connector: {:?}", row_text(&buf, a));
        // `b` is a non-last child of the root: `ancestorContinues = [false]` → `"   "`, then `"├─ "`.
        assert!(row_text(&buf, a + 1).starts_with("     ├─ Branch one"), "{:?}", row_text(&buf, a + 1));
        // `d` hangs under `b`, which still has `c` after it, so the depth-1 gutter CONTINUES: `"│  "`.
        assert!(row_text(&buf, a + 2).starts_with("     │  └─ Leaf"), "{:?}", row_text(&buf, a + 2));
        // `c` is the last child: `"└─ "`.
        assert!(row_text(&buf, a + 3).starts_with("     └─ Branch two"), "{:?}", row_text(&buf, a + 3));
    }

    /// **S8.** The connectors are `theme.fg("dim", prefix)` (`:500`) — the `dim` token, and a
    /// different colour from the message they precede.
    #[test]
    fn tree_connectors_are_drawn_in_the_dim_token() {
        let mut sel = tree_selector();
        let buf = draw(&mut sel, 60, 20);
        let theme = UiTheme::default();
        let y = find_row(&buf, "Branch one");
        let x = col_of(&buf, y, "├");
        assert_eq!(buf[(x, y)].fg, theme.dim_style().fg.unwrap(), "connector is not `dim`");
    }

    /// **S8.** A `parentSessionPath` cycle must not hang or blow the stack — cyrup's documented
    /// delta over upstream's unguarded recursion (`:262-271`).
    #[test]
    fn a_parent_cycle_degrades_to_a_forest_instead_of_recursing() {
        let mut sel = SessionSelector::new(vec![
            row("/s/a.jsonl", Some("A"), "a", 2),
            row("/s/b.jsonl", Some("B"), "b", 1),
        ]);
        sel.set_parent_paths([
            ("/s/a.jsonl".to_string(), "/s/b.jsonl".to_string()),
            ("/s/b.jsonl".to_string(), "/s/a.jsonl".to_string()),
        ]);
        assert_eq!(sel.visible_len(), 2, "every session must still be reachable");
    }

    /// **S9.** `session-selector.ts:502-505`:
    /// `spacing = max(1, width - leftWidth - visibleWidth(rightPart))` then
    /// `theme.fg(isConfirmingDelete ? "error" : "dim", rightPart)` — the metadata column is FLUSH
    /// RIGHT and `dim`.
    ///
    /// FAILS before the fix: cyrup pushed `Span::styled(format!("  {desc}"), muted_style())` right
    /// after the label, so the column was ragged and one shade too bright.
    #[test]
    fn metadata_column_is_flush_right_and_dim() {
        let mut sel = SessionSelector::new(rows());
        let theme = UiTheme::default();
        let buf = draw(&mut sel, 60, 20);
        let y = find_row(&buf, "Build pipeline");
        let text = row_text(&buf, y);
        assert!(text.ends_with("3 msgs"), "metadata must reach the right edge: {text:?}");
        // The gap between the label and the column is more than the old fixed two spaces.
        assert!(text.contains("Build pipeline    "), "column is not right-aligned: {text:?}");
        let last = buf.area.width - 1;
        assert_eq!(buf[(last, y)].fg, theme.dim_style().fg.unwrap(), "column is not `dim`");
        assert_ne!(
            theme.dim_style().fg.unwrap(),
            theme.muted_style().fg.unwrap(),
            "`dim` and `muted` are distinct tokens — this test would be vacuous otherwise"
        );
    }

    /// **S11.** `session-selector.ts:486-497`: `error` while the row's delete is queued, `accent`
    /// for the session you are in, `warning` for a named session — and `theme.bold(styledMsg)` is
    /// applied ON TOP of that when selected, not instead of it.
    ///
    /// FAILS before the fix: `body_lines` had exactly two styles, `accent+BOLD` when selected and
    /// `base` otherwise.
    #[test]
    fn row_state_picks_the_message_colour_and_selection_only_adds_bold() {
        let theme = UiTheme::default();
        let mut sel = SessionSelector::new(rows());
        sel.set_current_session_path(Some("/s/c.jsonl".to_string()));
        let buf = draw(&mut sel, 60, 20);

        // Row 0 ("Build pipeline") is named AND selected: `warning` + BOLD.
        let y = find_row(&buf, "Build pipeline");
        let x = col_of(&buf, y, "Build");
        assert_eq!(buf[(x, y)].fg, theme.warning_style().fg.unwrap(), "named row is not `warning`");
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD), "selected row is not bold");
        assert_ne!(
            buf[(x, y)].fg,
            theme.accent_style().fg.unwrap(),
            "selection must not overwrite the state colour with accent"
        );

        // `/s/b.jsonl` is unnamed and not current: plain `base`.
        let y = find_row(&buf, "/s/b.jsonl");
        let x = col_of(&buf, y, "/s/b");
        assert_eq!(buf[(x, y)].fg, theme.base_style().fg.unwrap(), "plain row is not `base`");

        // `/s/c.jsonl` is the current session: `accent` beats its name's `warning`.
        let y = find_row(&buf, "Docs");
        let x = col_of(&buf, y, "Docs");
        assert_eq!(buf[(x, y)].fg, theme.accent_style().fg.unwrap(), "current row is not `accent`");
    }

    /// **S11.** `:487-488` — the row queued for deletion is `error`, message AND metadata column
    /// (`:503`).
    #[test]
    fn the_row_queued_for_deletion_is_red_on_both_sides() {
        let theme = UiTheme::default();
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        sel.handle(&ctrl('d'), &km);
        let buf = draw(&mut sel, 60, 20);
        let y = find_row(&buf, "Build pipeline");
        let x = col_of(&buf, y, "Build");
        assert_eq!(buf[(x, y)].fg, theme.error_style().fg.unwrap(), "message is not `error`");
        let last = buf.area.width - 1;
        assert_eq!(buf[(last, y)].fg, theme.error_style().fg.unwrap(), "column is not `error`");
    }

    /// **S12.** `session-selector.ts:131-153`: the title NAMES THE SCOPE and is `theme.bold(title)`
    /// with no `fg`; the right group is `scopeText  nameText  sortText`, flush right, with the live
    /// radio side `accent` and `Name:`/`Sort:` a `muted` label + `accent` value.
    ///
    /// FAILS before the fix: an accent-teal `" Resume Session"` with one flat `muted`
    /// `"   Name: All   Sort: …"` packed against it and no scope anywhere on screen.
    #[test]
    fn header_names_the_scope_and_right_aligns_the_toggles() {
        let theme = UiTheme::default();
        let mut sel = SessionSelector::new(rows());
        let buf = draw(&mut sel, 100, 20);
        let y = find_row(&buf, "Resume Session");
        let text = row_text(&buf, y);

        assert!(text.starts_with("Resume Session (Current Folder)"), "{text:?}");
        assert!(text.ends_with("Sort: Threaded"), "right group is not flush right: {text:?}");
        assert!(text.contains("◉ Current Folder | ○ All"), "no scope radio: {text:?}");

        // `theme.bold(title)`: bold, and NOT accent.
        assert!(buf[(0, y)].modifier.contains(Modifier::BOLD), "title is not bold");
        assert_eq!(buf[(0, y)].fg, theme.base_style().fg.unwrap(), "title must carry no fg escape");
        assert_ne!(buf[(0, y)].fg, theme.accent_style().fg.unwrap(), "title is still accent teal");

        // The live radio side is `accent`; the other side is `muted`.
        let radio = col_of(&buf, y, "◉");
        assert_eq!(buf[(radio, y)].fg, theme.accent_style().fg.unwrap(), "live scope is not accent");
        let other = col_of(&buf, y, "○");
        assert_eq!(buf[(other, y)].fg, theme.muted_style().fg.unwrap(), "dormant scope is not muted");

        // `Name: ` is `muted`, its value `accent` (`:135-138`).
        let label = col_of(&buf, y, "Name: ");
        assert_eq!(buf[(label, y)].fg, theme.muted_style().fg.unwrap(), "`Name:` is not muted");
        let value = label + 6;
        assert_eq!(buf[(value, y)].fg, theme.accent_style().fg.unwrap(), "`All` is not accent");
    }

    /// **S12.** `:131` — toggling scope rewrites both the title and which radio is lit.
    #[test]
    fn header_reports_the_all_scope() {
        let mut sel = SessionSelector::new(rows());
        // SEAM-061: the `All` scope now DISPLAYS the all-projects set, so a selector that was never
        // handed one shows an empty list. Wire it, or this test asserts the header of a blank list.
        sel.set_all_rows(rows());
        sel.set_scope(SessionScope::All);
        let buf = draw(&mut sel, 100, 20);
        let text = row_text(&buf, find_row(&buf, "Resume Session"));
        assert!(text.starts_with("Resume Session (All)"), "{text:?}");
        assert!(text.contains("○ Current Folder | ◉ All"), "{text:?}");
    }

    /// **S13.** `session-selector.ts:738` and `:746` both build
    /// `new DynamicBorder((s) => theme.fg("accent", s))`.
    ///
    /// FAILS before the fix: `border_rule_line` used `theme.border_style()` — the `border` token.
    #[test]
    fn both_rules_are_accent_not_the_border_token() {
        let theme = UiTheme::default();
        let mut sel = SessionSelector::new(rows());
        let buf = draw(&mut sel, 60, 20);
        let rules: Vec<u16> =
            (0..buf.area.height).filter(|y| row_text(&buf, *y).starts_with("──")).collect();
        assert_eq!(rules.len(), 2, "expected a top and a bottom rule: {rules:?}");
        assert_ne!(
            theme.accent_style().fg.unwrap(),
            theme.border_style().fg.unwrap(),
            "`accent` and `border` are distinct tokens — this test would be vacuous otherwise"
        );
        for y in rules {
            assert_eq!(buf[(0, y)].fg, theme.accent_style().fg.unwrap(), "rule at {y} is not accent");
        }
    }

    /// **S14.** `session-selector.ts:471-473` folds the path into the SAME row's right column
    /// (`rightPart = shortenPath(session.path) + " " + rightPart`) — still one line per session.
    ///
    /// FAILS before the fix: `Ctrl+P` pushed a second `Line` per row, doubling the list height and
    /// halving how many sessions fit.
    #[test]
    fn the_path_toggle_folds_the_path_into_the_row_instead_of_adding_a_line() {
        let mut sel = SessionSelector::new(rows());
        let km = SelectKeymap::default();
        let before = sel.desired_height(80);
        sel.handle(&ctrl('p'), &km);
        assert_eq!(
            sel.desired_height(80),
            before,
            "toggling the path must not change the list height"
        );
        let buf = draw(&mut sel, 80, 20);
        let text = row_text(&buf, find_row(&buf, "Build pipeline"));
        assert!(text.ends_with("/s/a.jsonl 3 msgs"), "path is not in the right column: {text:?}");
    }

    /// **S27.** The row's message column is cut by the crate's one grapheme-atomic truncator
    /// (`crate::text_width::truncate_to_width`, `:479-483` → `tui/src/utils.ts:1053`), so a ZWJ
    /// sequence is either kept whole or dropped whole.
    ///
    /// FAILS before the fix: this module carried its own `for ch in s.chars()` copy, so a family
    /// emoji straddling the cut was emitted as `👨` + a bare U+200D joiner — an orphaned component
    /// and a dangling joiner in a row built from a session's own message text (`:462`, `:603`).
    #[test]
    fn a_zwj_family_emoji_is_never_cut_in_half_by_the_row_truncator() {
        const FAMILY: &str = "👨\u{200d}👩\u{200d}👧";
        let theme = UiTheme::default();
        // 24 `A`s put the cluster exactly astride the 29-column message budget at width 40: its
        // first two components fit, the third does not.
        let label = format!("{}{}{}", "A".repeat(24), FAMILY, " trailing text");
        let sel = SessionSelector::new(vec![row("/s/z.jsonl", Some(label.as_str()), "z", 1)]);
        let filtered = sel.filtered();
        let lines = sel.body_lines(&theme, &filtered, 40);
        let text: String =
            lines.iter().flat_map(|l| l.spans.iter()).map(|s| &*s.content).collect();

        assert!(text.contains('…'), "the row was not truncated at all: {text:?}");
        let whole = text.contains(FAMILY);
        let partial = text.contains('\u{200d}')
            || text.contains('👨')
            || text.contains('👩')
            || text.contains('👧');
        assert!(whole || !partial, "the family cluster was split into components: {text:?}");
    }

    /// **S15.** `session-selector.ts:512-517`: when the `maxVisible` window does not cover the list,
    /// a `theme.fg("muted", …)` `  (i/N)` row follows the rows.
    ///
    /// FAILS before the fix: `body_lines` returned straight after the row loop.
    #[test]
    fn a_short_window_over_a_long_list_reports_its_position() {
        let theme = UiTheme::default();
        let long: Vec<SessionRow> = (0..25)
            .map(|i| row(&format!("/s/{i}.jsonl"), None, &format!("s{i}"), 100 - i as u128))
            .collect();
        let mut sel = SessionSelector::new(long);
        let km = SelectKeymap::default();
        let buf = draw(&mut sel, 60, 24);
        let y = find_row(&buf, "(1/25)");
        assert!(row_text(&buf, y).starts_with("  (1/25)"), "{:?}", row_text(&buf, y));
        assert_eq!(buf[(2, y)].fg, theme.muted_style().fg.unwrap(), "scroll row is not `muted`");

        // It tracks the highlight.
        for _ in 0..6 {
            sel.handle(&key(KeyCode::Down), &km);
        }
        let buf = draw(&mut sel, 60, 24);
        assert!(row_text(&buf, find_row(&buf, "(7/25)")).starts_with("  (7/25)"));
    }

    /// **S15.** A list the window fully covers gets NO scroll row (`:513`'s guard).
    #[test]
    fn a_fully_visible_list_has_no_scroll_row() {
        let mut sel = SessionSelector::new(rows());
        let buf = draw(&mut sel, 60, 20);
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(!text.contains("(1/3)"), "unscrollable list must not report a position");
    }

    // ---- the header's hint rows (`session-selector.ts:155-185`) --------------------------------

    fn fg_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Option<ratatui::style::Color> {
        buf.cell((x, y)).map(|c| c.style().fg).unwrap_or(None)
    }

    /// `keyHint(kb, description)` is `theme.fg("dim", keyText(kb)) + theme.fg("muted", ` `
    /// ${description}`)` (`keybinding-hints.ts:42-44`) — **two** runs per pair. cyrup drew hint row
    /// 1 as one flat `muted` span and hint row 2 as one flat `dim` span, so the key and its
    /// description were indistinguishable in both.
    #[test]
    fn hint_pairs_are_a_dim_key_plus_a_muted_description() {
        let mut sel = SessionSelector::new(rows());
        let theme = UiTheme::dark();
        let backend = TestBackend::new(90, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        // `dim` and `muted` are different tokens (`#666666` vs `#808080`), so this cannot pass by
        // accident.
        assert_ne!(theme.dim_style().fg, theme.muted_style().fg);
        let y1 = find_row(&buf, "tab scope");
        assert_eq!(fg_at(&buf, col_of(&buf, y1, "tab"), y1), theme.dim_style().fg, "the key is dim");
        assert_eq!(
            fg_at(&buf, col_of(&buf, y1, "scope"), y1),
            theme.muted_style().fg,
            "its description is muted"
        );
        let y2 = find_row(&buf, "ctrl+s sort");
        assert_eq!(fg_at(&buf, col_of(&buf, y2, "ctrl+s"), y2), theme.dim_style().fg);
        assert_eq!(fg_at(&buf, col_of(&buf, y2, "sort"), y2), theme.muted_style().fg);
    }

    /// `keyHint("app.session.delete", "delete")` resolves through `getKeybindings().getKeys(...)`
    /// (`keybinding-hints.ts:29-36`), so a rebind reaches the HINT, not only the handler. cyrup
    /// spelled `ctrl+d` into the string.
    #[test]
    fn rebinding_a_session_action_renames_it_in_the_hint_row_and_moves_the_key() {
        let mut km = SessionKeymap::default();
        km.set_action(SessionAction::Delete, vec![Key::ctrl('k')]);
        let mut sel = SessionSelector::new(rows()).with_keymaps(&km, &EditorKeymap::default());
        let buf = draw(&mut sel, 90, 20);
        let text: String = (0..buf.area.height).map(|y| row_text(&buf, y)).collect();
        assert!(text.contains("ctrl+k delete"), "the hint names the NEW key: {text}");
        assert!(!text.contains("ctrl+d delete"), "and not the old one: {text}");
        // The handler moved with it.
        let mut sel = SessionSelector::new(rows()).with_keymaps(&km, &EditorKeymap::default());
        sel.handle(&ctrl('d'), &SelectKeymap::default());
        assert!(!sel.is_confirming_delete(), "ctrl+d is no longer bound");
        sel.handle(&ctrl('k'), &SelectKeymap::default());
        assert!(sel.is_confirming_delete(), "ctrl+k is");
    }

    /// `if (this.showRenameHint) hint2Parts.push(keyHint("app.session.rename", "rename"))`
    /// (`session-selector.ts:177-179`) — the gate cyrup ignored, so the pair was unconditional.
    #[test]
    fn the_rename_pair_is_gated_on_show_rename_hint() {
        let mut sel = SessionSelector::new(rows());
        let buf = draw(&mut sel, 100, 20);
        let with: String = (0..buf.area.height).map(|y| row_text(&buf, y)).collect();
        assert!(with.contains("rename"), "shown by default (`:772` = canRename): {with}");

        let mut sel = SessionSelector::new(rows());
        sel.set_show_rename_hint(false);
        let buf = draw(&mut sel, 100, 20);
        let without: String = (0..buf.area.height).map(|y| row_text(&buf, y)).collect();
        assert!(!without.contains("rename"), "gated off: {without}");
        assert!(without.contains("path (off)"), "the rest of the row survives: {without}");
    }

    // ---- the empty state (`session-selector.ts:421-438`) ---------------------------------------

    /// `SessionList.render`'s empty branch picks between **four** messages on `nameFilter` and
    /// `showCwd` (`session-selector.ts:422-436`), three of which tell the user how to widen the
    /// search. cyrup always drew `"  No sessions found"` — the one branch that offers no way out,
    /// and the branch that is only correct in the "All" scope with names unfiltered.
    #[test]
    fn the_empty_state_has_all_four_of_pis_branches() {
        // A query that matches nothing, so the list is empty in every configuration.
        let empty = |scope: SessionScope, named: bool| -> String {
            let mut sel = SessionSelector::new(rows());
            // Both scopes must be NON-empty to start with, so the empty state under test is
            // produced by the query below and not by an unwired `all` set (SEAM-061).
            sel.set_all_rows(rows());
            sel.set_scope(scope);
            assert_eq!(sel.visible_len(), 3, "the fixture must be non-empty before the query");
            if named {
                sel.handle(&ctrl('n'), &SelectKeymap::default());
            }
            for c in "zzzqqq".chars() {
                sel.handle(&key(KeyCode::Char(c)), &SelectKeymap::default());
            }
            let buf = draw(&mut sel, 100, 20);
            (0..buf.area.height)
                .map(|y| row_text(&buf, y))
                .find(|r| r.contains("No "))
                .unwrap_or_else(|| panic!("no empty-state row"))
                .trim_end()
                .to_string()
        };
        // `:430-432` — the only branch cyrup had, and the only one with no escape hatch.
        assert_eq!(empty(SessionScope::All, false), "  No sessions found");
        // `:433-435`.
        assert_eq!(
            empty(SessionScope::Current, false),
            "  No sessions in current folder. Press Tab to view all."
        );
        // `:425-426` — `keyText("app.session.toggleNamedFilter")`, resolved from the live table.
        assert_eq!(
            empty(SessionScope::All, true),
            "  No named sessions found. Press ctrl+n to show all."
        );
        // `:427-428`.
        assert_eq!(
            empty(SessionScope::Current, true),
            "  No named sessions in current folder. Press ctrl+n to show all, or Tab to view all."
        );
    }

    /// The `keyText` half of the two named branches (`:424`): rebinding
    /// `app.session.toggleNamedFilter` changes the key the message names.
    #[test]
    fn the_named_empty_state_names_the_live_toggle_key() {
        let mut km = SessionKeymap::default();
        km.set_action(SessionAction::ToggleNamedFilter, vec![Key::ctrl('j')]);
        let mut sel = SessionSelector::new(rows()).with_keymaps(&km, &EditorKeymap::default());
        sel.set_all_rows(rows());
        sel.set_scope(SessionScope::All);
        sel.handle(&ctrl('j'), &SelectKeymap::default());
        for c in "zzzqqq".chars() {
            sel.handle(&key(KeyCode::Char(c)), &SelectKeymap::default());
        }
        let buf = draw(&mut sel, 100, 20);
        let text: String = (0..buf.area.height).map(|y| row_text(&buf, y)).collect();
        assert!(text.contains("Press ctrl+j to show all."), "{text}");
    }

    // ---- SEAM-061: the `Tab` scope toggle (`session-selector.ts:551-556`, `:1003-1026`) ---------

    /// A second project's session, absent from the current-folder set and present in the
    /// all-projects one — pi's two loaders (`cli/session-picker.ts:15-19`).
    fn foreign_row() -> SessionRow {
        row("/other/z.jsonl", Some("Other project"), "z other project work", 9)
    }

    fn scoped_selector() -> SessionSelector {
        let mut all = rows();
        all.push(foreign_row());
        let mut sel = SessionSelector::new(rows());
        sel.set_all_rows(all);
        sel.set_session_cwds([
            ("/s/a.jsonl".to_string(), "/home/dev/here".to_string()),
            ("/s/b.jsonl".to_string(), "/home/dev/here".to_string()),
            ("/s/c.jsonl".to_string(), "/home/dev/here".to_string()),
            ("/other/z.jsonl".to_string(), "/home/dev/elsewhere".to_string()),
        ]);
        sel
    }

    /// **SEAM-061.** `kb.matches(keyData, "tui.input.tab")` → `onToggleScope()`
    /// (`session-selector.ts:551-556`) → `toggleScope` (`:1003-1026`), which re-points the list at
    /// the other cached set and flips the header.
    ///
    /// FAILS before the fix: `Tab` resolved to no action at all — it fell out of `handle` as
    /// `Ignored` — while the header advertised `tab scope` and the ONE merged list showed another
    /// project's sessions under the title "Resume Session (Current Folder)".
    #[test]
    fn tab_swaps_the_list_between_pis_two_loaders() {
        let mut sel = scoped_selector();
        let km = SelectKeymap::default();

        // Presence before absence: the foreign row exists in the fixture's `all` set.
        assert_eq!(sel.scope(), SessionScope::Current);
        assert_eq!(sel.visible_len(), 3);
        let current_paths: Vec<String> =
            sel.filtered().into_iter().map(|n| n.row.path).collect();
        assert!(
            !current_paths.contains(&"/other/z.jsonl".to_string()),
            "the current-folder scope must not list another project: {current_paths:?}"
        );

        let out = sel.handle(&key(KeyCode::Tab), &km);
        assert_eq!(out, SelectorOutcome::Redraw, "`Tab` is handled, not ignored");
        assert_eq!(sel.scope(), SessionScope::All);
        assert_eq!(sel.visible_len(), 4, "the `all` set is on screen");
        let all_paths: Vec<String> = sel
            .filtered()
            .into_iter()
            .map(|n| n.row.path)
            .collect();
        assert!(
            all_paths.contains(&"/other/z.jsonl".to_string()),
            "the other project's session is reachable: {all_paths:?}"
        );

        // `:1021-1025` — the second press goes back, it does not cycle onward.
        sel.handle(&key(KeyCode::Tab), &km);
        assert_eq!(sel.scope(), SessionScope::Current);
        assert_eq!(sel.visible_len(), 3);
    }

    /// **SEAM-061.** `const showCwd = scope === "all"` (`:844`, `:923`) and
    /// `rightPart = ${shortenPath(session.cwd)} ${rightPart}` (`:468-470`): the cwd column exists
    /// ONLY in the `all` scope, and it is what tells a merged row which project it belongs to.
    #[test]
    fn the_cwd_column_appears_only_in_the_all_scope() {
        let mut sel = scoped_selector();
        let buf = draw(&mut sel, 100, 24);
        let text: String = (0..buf.area.height).map(|y| row_text(&buf, y)).collect();
        assert!(text.contains("Build pipeline"), "the current set is on screen: {text}");
        assert!(!text.contains("/home/dev/here"), "no cwd column in `current`: {text}");

        sel.handle(&key(KeyCode::Tab), &SelectKeymap::default());
        let buf = draw(&mut sel, 100, 24);
        let y = find_row(&buf, "Other project");
        let line = row_text(&buf, y);
        assert!(line.contains("/home/dev/elsewhere"), "foreign row names its cwd: {line:?}");
        let here = row_text(&buf, find_row(&buf, "Build pipeline"));
        assert!(here.contains("/home/dev/here"), "local row names its cwd too: {here:?}");
        // `:464-473` prepends cwd to the counts column, it does not add a second line per session.
        assert!(
            here.find("/home/dev/here").unwrap() < here.find("3 msgs").unwrap(),
            "cwd precedes the counts in the same right column: {here:?}"
        );
    }

    /// **SEAM-061.** `if (this.onToggleScope) { this.onToggleScope(); } return;` (`:551-556`):
    /// upstream RETURNS whether or not a toggle is wired, so `Tab` is swallowed by the picker and
    /// never reaches the search input. cyrup's un-wired case is `set_all_rows` never called.
    #[test]
    fn tab_is_swallowed_when_the_host_wired_no_all_set() {
        let mut sel = SessionSelector::new(rows());
        let out = sel.handle(&key(KeyCode::Tab), &SelectKeymap::default());
        assert_eq!(out, SelectorOutcome::Redraw, "consumed, like upstream's bare `return`");
        assert_eq!(sel.scope(), SessionScope::Current, "nowhere to switch to");
        assert_eq!(sel.visible_len(), 3, "and the list is untouched");
    }

    /// **SEAM-061.** `onDeleteSession` filters `currentSessions` AND `allSessions` before re-setting
    /// the list (`session-selector.ts:835-845`).
    ///
    /// FAILS if only the visible set is filtered: the deleted row reappears on the next `Tab`.
    #[test]
    fn a_delete_removes_the_row_from_both_scopes() {
        let mut sel = scoped_selector();
        let km = SelectKeymap::default();
        sel.handle(&ctrl('d'), &km);
        assert!(sel.is_confirming_delete());
        sel.handle(&key(KeyCode::Enter), &km);
        assert_eq!(sel.visible_len(), 2, "gone from the current scope");
        sel.handle(&key(KeyCode::Tab), &km);
        let all_paths: Vec<String> = sel.filtered().into_iter().map(|n| n.row.path).collect();
        assert_eq!(all_paths.len(), 3, "gone from the all scope too: {all_paths:?}");
        assert!(
            !all_paths.contains(&"/s/a.jsonl".to_string()),
            "the deleted session must not come back on `Tab`: {all_paths:?}"
        );
    }

    /// **SEAM-061.** A rename applied in one scope must survive the toggle, for the same reason a
    /// delete does — pi reloads BOTH sets after a mutation (`refreshSessionsAfterMutation`,
    /// `:999-1001` → `loadScope`).
    #[test]
    fn a_rename_survives_the_scope_toggle() {
        let mut sel = scoped_selector();
        let km = SelectKeymap::default();
        sel.handle(&ctrl('r'), &km);
        for c in "Zed".chars() {
            sel.handle(&key(KeyCode::Char(c)), &km);
        }
        sel.handle(&key(KeyCode::Enter), &km);
        sel.handle(&key(KeyCode::Tab), &km);
        let renamed = sel
            .filtered()
            .into_iter()
            .find(|n| n.row.path == "/s/a.jsonl")
            .expect("the row is still in the all scope");
        assert_eq!(renamed.row.name.as_deref(), Some("Build pipelineZed"));
        assert_eq!(renamed.row.label, "Build pipelineZed");
    }

    /// **SEAM-061.** The toggle and its hint are ONE binding — pi asks `tui.input.tab` at `:551`
    /// and labels the hint with `keyHint("tui.input.tab", "scope")` at `:170`. Rebinding the editor
    /// id must move both, and must leave the stock key dead.
    #[test]
    fn rebinding_tui_input_tab_moves_both_the_scope_toggle_and_its_hint() {
        let mut editor = EditorKeymap::default();
        editor.set_action(EditorAction::Tab, vec![Key::ctrl('t')]);
        let mut all = rows();
        all.push(foreign_row());
        let mut sel = SessionSelector::new(rows())
            .with_keymaps(&SessionKeymap::default(), &editor);
        sel.set_all_rows(all);

        let buf = draw(&mut sel, 100, 24);
        let text: String = (0..buf.area.height).map(|y| row_text(&buf, y)).collect();
        assert!(text.contains("ctrl+t scope"), "the hint names the NEW key: {text}");
        assert!(!text.contains("tab scope"), "and not the stock one: {text}");

        // The stock key is dead...
        let km = SelectKeymap::default();
        sel.handle(&key(KeyCode::Tab), &km);
        assert_eq!(sel.scope(), SessionScope::Current, "`tab` is no longer bound");
        // ...and the rebound one drives the toggle.
        sel.handle(&ctrl('t'), &km);
        assert_eq!(sel.scope(), SessionScope::All);
        assert_eq!(sel.visible_len(), 4);
    }

    /// **SEAM-061.** `tui.input.tab` is asked BEFORE every `app.session.*` id (`:551` vs `:558`),
    /// so a user who rebinds a session action onto tab still gets the scope toggle. Pinning the
    /// precedence keeps cyrup's agreement with upstream from being an accident of statement order.
    #[test]
    fn tab_beats_a_session_action_rebound_onto_the_same_chord() {
        let mut km = SessionKeymap::default();
        km.set_action(SessionAction::Delete, vec![Key::parse("tab").unwrap()]);
        let mut all = rows();
        all.push(foreign_row());
        let mut sel =
            SessionSelector::new(rows()).with_keymaps(&km, &EditorKeymap::default());
        sel.set_all_rows(all);
        sel.handle(&key(KeyCode::Tab), &SelectKeymap::default());
        assert_eq!(sel.scope(), SessionScope::All, "the scope toggle wins");
        assert!(!sel.is_confirming_delete(), "the rebound delete does not fire");
    }

    /// `:158-161` — the confirmation replaces hint row 1 and sets `hintLine2 = ""`, so the row is
    /// still emitted (blank) and nothing below the header shifts. The keys come from
    /// `tui.select.confirm`/`cancel` (`:159`), not from a literal `enter`/`esc`.
    #[test]
    fn the_delete_confirmation_replaces_hint_one_and_blanks_hint_two() {
        let mut sel = SessionSelector::new(rows());
        let before = sel.desired_height(90);
        sel.handle(&ctrl('d'), &SelectKeymap::default());
        assert_eq!(sel.desired_height(90), before, "the header stays three rows tall");
        let buf = draw(&mut sel, 90, 20);
        let y = find_row(&buf, "Delete session?");
        assert!(
            row_text(&buf, y).starts_with("Delete session? enter confirm · escape/ctrl+c cancel"),
            "{:?}",
            row_text(&buf, y)
        );
        assert_eq!(row_text(&buf, y + 1).trim_end(), "", "hintLine2 = \"\" (:161)");
        assert!(row_text(&buf, y + 2).trim_end().is_empty(), "then Spacer(:742)");
        assert!(row_text(&buf, y + 3).starts_with('>'), "then the search Input (:418)");
    }
}
