//! The `/tree` session-navigator layout engine (spec/tui/05 §5.1, §6.1; Pi `tree-selector.ts`, 47KB —
//! the largest first-party selector). This module ports the **bespoke layout**: branch connectors
//! (`├─ └─ │`), fold markers (`⊟`/`⊞`), per-entry glyphs (`● ◆ ◇ ⚙ ✓`), the has-label star (`☆`), a
//! right-aligned relative-time column, the selected-row accent, the filter-mode chips, and fold/filter
//! navigation. It is a full-width editor-swap [`Selector`] just like the other first-party pickers.
//!
//! The node list ([`TreeNode`]) is a flattened session DAG: each node carries its `depth`, `label`,
//! [`TreeKind`], fold state, and an optional time label. Building that list from the live session DAG
//! is the one L5 seam (`AgentSession` exposes `navigate_tree`/`branch` actions and `session_tree`
//! events but no flat-tree *getter* yet — tracked in the residual ledger); the **rendering + fold +
//! filter + connector** engine that is the bulk of the 47KB is built and tested here.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{SelectAction, SelectKeymap, TreeAction, TreeKeymap};
use crate::selector::{Selector, SelectorOutcome};
use crate::theme::UiTheme;

/// The entry-type glyph key (`tree-selector.ts` switch ~`:638`, render `:691-727`; spec/tui/05 §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKind {
    /// A user/assistant message — `●`.
    Message,
    /// A model change — `◆`.
    ModelChange,
    /// A thinking-level change — `◇`.
    ThinkingChange,
    /// A group of tool calls — `⚙`.
    ToolGroup,
    /// A compaction/branch-summary entry — `✓`.
    Compaction,
}

impl TreeKind {
    /// The single-cell glyph for this entry type.
    pub fn glyph(self) -> &'static str {
        match self {
            TreeKind::Message => "●",
            TreeKind::ModelChange => "◆",
            TreeKind::ThinkingChange => "◇",
            TreeKind::ToolGroup => "⚙",
            TreeKind::Compaction => "✓",
        }
    }
}

/// The per-role colouring of a `/tree` row's entry text — `getEntryDisplayText`
/// (`tree-selector.ts:768-852`).
///
/// **S24(b).** Upstream never draws the entry text as one span. It switches on `entry.type` and,
/// for a message, on `msg.role`, and emits a *coloured role prefix* followed by the content:
///
/// ```text
/// :781  result = theme.fg("accent",  "user: ")      + content;
/// :786  result = theme.fg("success", "assistant: ") + textContent;
/// :788  result = theme.fg("success", "assistant: ") + theme.fg("muted", "(aborted)");
/// :791  result = theme.fg("success", "assistant: ") + theme.fg("error",   errMsg);
/// :793  result = theme.fg("success", "assistant: ") + theme.fg("muted", "(no content)");
/// :799  result = theme.fg("muted",   this.formatToolCall(...));
/// :805  result = theme.fg("dim",     `[bash]: ${...}`);
/// :819  result = theme.fg("customMessageLabel", `[${entry.customType}]: `) + content;
/// :824  result = theme.fg("borderAccent", `[compaction: ${tokens}k tokens]`);
/// :828  result = theme.fg("warning", `[branch summary]: `) + summary;
/// :831/:834/:837/:840/:843  result = theme.fg("dim", …);
/// :851  return isSelected ? theme.bold(result) : result;
/// ```
///
/// Two consequences cyrup missed. First, a user row, an assistant row, a tool-result row and a
/// compaction row were **colour-identical** — one `base_style()` span each. Second, `:851` only
/// *bolds* the selected row; it does not recolour it, where cyrup replaced the whole label with
/// `accent`, so the selected row lost what little role information the row carried.
///
/// ## Why this is classified from the label text
///
/// cyrup splits `getEntryDisplayText` across two crates: the **text** half is
/// `cyrup_session_svc`'s `dag_display` (`session.rs:4935-4995`, whose own doc comment cites
/// `tree-selector.ts:762-830`), which composes exactly these prefixes — `"user: "`, `"assistant: "`,
/// `"[bash]: "`, `"branch summary: "`, `"model → "`, `"thinking → "`, `"title: "`, `"custom "`,
/// `"label "` — and the **style** half is here. `SessionDagNode` carries `kind`
/// ([`SessionDagKind`](cyrup_session_svc::SessionDagKind)) but no role, and `kind` collapses
/// user/assistant/bash/custom into one `Message` variant and compaction/branch-summary into one
/// `Compaction` variant, so the role is recoverable only from the text that `dag_display` wrote.
/// [`TreeEntryRole::classify`] therefore reads `kind` first and disambiguates on that exact prefix
/// set. A `role` field on `SessionDagNode` would let the classifier collapse to a `match`; that is
/// a cyrup-session-svc change, and the classifier is written so it is the only thing that would
/// need to move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeEntryRole {
    /// `theme.fg("accent", "user: ") + content` (`:781`).
    User,
    /// `theme.fg("success", "assistant: ") + textContent` (`:786`).
    Assistant,
    /// `theme.fg("success", "assistant: ") + theme.fg("muted", "(no content)")` (`:788`, `:793`) —
    /// the placeholder body is `muted`, not body text.
    AssistantPlaceholder,
    /// `theme.fg("muted", …)` over the whole row (`:799-801`).
    ToolResult,
    /// `theme.fg("dim", "[bash]: …")` over the whole row (`:805`).
    Bash,
    /// `theme.fg("customMessageLabel", "[type]: ") + content` (`:819`).
    CustomMessage,
    /// `theme.fg("borderAccent", "[compaction: Nk tokens]")` over the whole row (`:824`) — the one
    /// and only `borderAccent` render site in pi (T9).
    Compaction,
    /// `theme.fg("warning", "[branch summary]: ") + summary` (`:828`).
    BranchSummary,
    /// `theme.fg("dim", …)` over the whole row — `model_change` (`:831`),
    /// `thinking_level_change` (`:834`), `custom` (`:837`), `label` (`:840`), `session_info`
    /// (`:843-845`), and the `[role]` fallback for an unrecognised message role (`:807`).
    Dim,
    /// No upstream case matched: `default: result = ""` (`:847-848`) has no colour of its own, and a
    /// row whose label was built by something other than `dag_display` (a test fixture, an embedder)
    /// keeps body text.
    Plain,
}

impl TreeEntryRole {
    /// Classify a row from its [`TreeKind`] and its already-composed label — see the type-level
    /// doc for why the label is load-bearing here.
    ///
    /// `kind` decides first wherever it can (`ToolGroup`/`Compaction`/`ModelChange`/
    /// `ThinkingChange` are unambiguous), and the prefix set disambiguates the rest, in the order
    /// `dag_display` can emit them: `"[bash]: "` is tested before the generic `[…]` custom-message
    /// shape because it also matches it.
    #[must_use]
    pub fn classify(kind: TreeKind, label: &str) -> TreeEntryRole {
        match kind {
            // `:795-801` — a tool result is muted end to end, with no coloured prefix.
            TreeKind::ToolGroup => TreeEntryRole::ToolResult,
            // `dag_display` maps BOTH `compaction` (`:822-824`) and `branch_summary` (`:827-828`)
            // onto `SessionDagKind::Compaction`, and upstream colours them differently — `:828` is
            // `warning`, not `borderAccent`.
            TreeKind::Compaction => {
                if label.starts_with("branch summary: ") {
                    TreeEntryRole::BranchSummary
                } else {
                    TreeEntryRole::Compaction
                }
            }
            // `:830-834` — both are a single `theme.fg("dim", …)`.
            TreeKind::ModelChange | TreeKind::ThinkingChange => TreeEntryRole::Dim,
            TreeKind::Message => {
                if label.starts_with("user: ") {
                    TreeEntryRole::User
                } else if let Some(body) = label.strip_prefix("assistant: ") {
                    // `:787-793`: an assistant row with no text renders its placeholder `muted`.
                    if body == "(no content)" || body == "(aborted)" {
                        TreeEntryRole::AssistantPlaceholder
                    } else {
                        TreeEntryRole::Assistant
                    }
                } else if label.starts_with("[bash]: ") {
                    TreeEntryRole::Bash
                } else if label.starts_with("title: ")
                    || label.starts_with("custom ")
                    || label.starts_with("label ")
                {
                    TreeEntryRole::Dim
                } else if label.starts_with('[') && label.ends_with(']') {
                    TreeEntryRole::CustomMessage
                } else {
                    TreeEntryRole::Plain
                }
            }
        }
    }

    /// The row's coloured spans: a `(prefix, prefix-style)` pair plus the remainder in body style,
    /// or a single whole-row span where upstream colours the whole row.
    ///
    /// `:851` `return isSelected ? theme.bold(result) : result;` — the selected row is **bolded**,
    /// keeping every per-role colour. It is not repainted in `accent`.
    fn spans(self, label: &str, is_sel: bool, theme: &UiTheme) -> Vec<Span<'static>> {
        let bold = |s: Style| if is_sel { s.add_modifier(Modifier::BOLD) } else { s };
        let split = |prefix_len: usize, prefix_style: Style, body_style: Style| {
            let (prefix, body) = label.split_at(prefix_len.min(label.len()));
            let mut out = vec![Span::styled(prefix.to_string(), bold(prefix_style))];
            if !body.is_empty() {
                out.push(Span::styled(body.to_string(), bold(body_style)));
            }
            out
        };
        let whole = |style: Style| vec![Span::styled(label.to_string(), bold(style))];
        match self {
            TreeEntryRole::User => {
                split("user: ".len(), theme.accent_style(), theme.base_style())
            }
            TreeEntryRole::Assistant => {
                split("assistant: ".len(), theme.success_style(), theme.base_style())
            }
            TreeEntryRole::AssistantPlaceholder => {
                split("assistant: ".len(), theme.success_style(), theme.muted_style())
            }
            TreeEntryRole::ToolResult => whole(theme.muted_style()),
            TreeEntryRole::Bash => whole(theme.dim_style()),
            // `:819` is a bare `theme.fg("customMessageLabel", …)`. The bold that
            // [`UiTheme::custom_message_label_style`] carries belongs to the TRANSCRIPT's label —
            // `custom-message.ts:92` wraps the text in `\x1b[1m…\x1b[22m` *inside* the colour —
            // and there is no such wrapper here, so it is removed rather than inherited. (`:851`
            // adds bold back on the selected row, which is a different thing and applies to every
            // role alike.)
            TreeEntryRole::CustomMessage => {
                whole(theme.custom_message_label_style().remove_modifier(Modifier::BOLD))
            }
            TreeEntryRole::Compaction => whole(theme.border_accent_style()),
            TreeEntryRole::BranchSummary => {
                split("branch summary: ".len(), theme.warning_style(), theme.base_style())
            }
            TreeEntryRole::Dim => whole(theme.dim_style()),
            TreeEntryRole::Plain => whole(theme.base_style()),
        }
    }
}

/// The `/tree` filter modes (`filterMode`, `tree-selector.ts`; cycled by `1-5`, spec/tui/05 §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// All entries except folded subtrees — `1 default`.
    Default,
    /// Hide tool groups — `2 no-tools`.
    NoTools,
    /// Only user/assistant messages — `3 user`.
    UserOnly,
    /// Only labeled entries — `4 labeled`.
    LabeledOnly,
    /// Everything, including tool calls — `5 all`.
    All,
}

impl FilterMode {
    /// The chip label shown in the header (`Filter: <name>`).
    pub fn label(self) -> &'static str {
        match self {
            FilterMode::Default => "default",
            FilterMode::NoTools => "no-tools",
            FilterMode::UserOnly => "user",
            FilterMode::LabeledOnly => "labeled",
            FilterMode::All => "all",
        }
    }

    /// Map the persisted `treeFilterMode` setting value to a mode.
    ///
    /// Pi seeds the `/tree` selector from `settingsManager.getTreeFilterMode()`
    /// (`interactive-mode.ts:4644` → `TreeSelectorComponent(..., initialFilterMode)` →
    /// `tree-selector.ts:137` `this.filterMode = initialFilterMode ?? "default"`), so a configured
    /// filter is the one `/tree` OPENS with. The accepted values are the `FilterMode` union
    /// (`settings-manager.ts:117`): `default` | `no-tools` | `user-only` | `labeled-only` | `all`;
    /// anything else falls back to `default`, matching
    /// [`cyrup_config::EffectiveSettings::tree_filter_mode`]'s own validation.
    ///
    /// Note the SETTING spellings (`user-only`/`labeled-only`) are not the header CHIP labels
    /// [`Self::label`] renders (`user`/`labeled`) — Pi's chip text is likewise shortened.
    #[must_use]
    pub fn from_setting(value: &str) -> FilterMode {
        match value {
            "no-tools" => FilterMode::NoTools,
            "user-only" => FilterMode::UserOnly,
            "labeled-only" => FilterMode::LabeledOnly,
            "all" => FilterMode::All,
            _ => FilterMode::Default,
        }
    }

    // TUI-027: `from_digit` is deleted. cyrup used to switch filter modes on the bare digits
    // `1`-`5`; upstream has no digit arm anywhere in `handleInput` — every printable character no
    // binding claims is appended to the tree's text SEARCH (`tree-selector.ts:1093-1100`), and the
    // filter modes are the seven `app.tree.filter.*` ctrl chords instead.

    /// Whether `node` survives this filter.
    fn keeps(self, node: &TreeNode) -> bool {
        match self {
            FilterMode::Default | FilterMode::All => true,
            FilterMode::NoTools => node.kind != TreeKind::ToolGroup,
            FilterMode::UserOnly => node.kind == TreeKind::Message,
            FilterMode::LabeledOnly => node.has_label,
        }
    }
}

/// One flattened session-DAG node (`FlatNode`, `tree-selector.ts:283`).
#[derive(Clone, Debug)]
pub struct TreeNode {
    /// The entry id confirmed via `onSelect(entryId)` (drives the branch/summarize flow).
    pub id: String,
    /// Tree depth (0 = a virtual-root child, which gets **no** connector, spec/tui/05 §5.1).
    pub depth: usize,
    /// The row text (message preview / "N tool calls" / "model → …").
    pub label: String,
    /// The entry glyph kind.
    pub kind: TreeKind,
    /// Whether this node has foldable descendants (renders `⊟`/`⊞`).
    pub foldable: bool,
    /// Whether this node is currently folded (its subtree hidden).
    pub folded: bool,
    /// Whether the entry carries a user label (renders the `☆` star).
    pub has_label: bool,
    /// When this entry's **label** was last set, pre-formatted for display — Pi's
    /// `FlatNode.node.labelTimestamp` (`session-manager.ts:165`, rendered at `tree-selector.ts:741-743`).
    /// `None` hides the column.
    ///
    /// It is the *label-change* time, not the entry's own timestamp, and it is only ever shown on a
    /// row that carries a label: Pi's render condition is
    /// `showLabelTimestamps && node.label && node.labelTimestamp`, which [`TreeSelector::rows`]
    /// reproduces as `show_time && has_label && time_label.is_some()`. A timestamp on an unlabeled
    /// node is therefore ignored rather than rendered.
    pub time_label: Option<String>,
}

impl TreeNode {
    /// Convenience constructor for a plain message node.
    pub fn message(id: impl Into<String>, depth: usize, label: impl Into<String>) -> Self {
        TreeNode {
            id: id.into(),
            depth,
            label: label.into(),
            kind: TreeKind::Message,
            foldable: false,
            folded: false,
            has_label: false,
            time_label: None,
        }
    }
}

/// The inline label-edit sub-state (`LabelInput`, `tree-selector.ts:1231-1283`): active only while the
/// user is renaming the selected entry (`e`, `app.tree.editLabel`). Mirrors Pi's `LabelInput` embedded
/// `Input` — a live text buffer + caret offset over the entry whose label is being set. When `Some`, the
/// tree list captures **all** keys into this buffer (Pi's `if (this.labelInput) …` route,
/// `tree-selector.ts:1373-1379`), so `e`/`z`/digits type literally instead of firing tree actions.
struct LabelEdit {
    /// The entry id being (re)labeled — carried into the persist payload on save.
    entry_id: String,
    /// The label text buffer (empty ⇒ "remove label", Pi `value || undefined`, `:1277`).
    query: String,
    /// Caret byte offset within `query`.
    cursor: usize,
}

/// The session-tree navigator selector (the bespoke `/tree` layout). Holds the full flattened node
/// list plus selection/filter/timestamp UI state; renders only the *visible* rows (filter + folds).
pub struct TreeSelector {
    nodes: Vec<TreeNode>,
    /// Index into the **visible** rows (filter + fold applied).
    selected: usize,
    filter: FilterMode,
    /// Whether the label-timestamp column is shown (`app.tree.toggleLabelTimestamp`). **Off** by
    /// default, as Pi's `private showLabelTimestamps = false` (`tree-selector.ts:116`) — the `t`
    /// toggle turns it ON; it does not turn a default-on column off.
    show_time: bool,
    keymap: TreeKeymap,
    /// The inline label editor, present only while renaming (`shift+l`); see [`LabelEdit`].
    label_edit: Option<LabelEdit>,
    /// The live text search (`tree-selector.ts:113` `private searchQuery = ""`).
    ///
    /// **TUI-027.** Every printable, non-control key that no `app.tree.*` / `tui.select.*` binding
    /// claims is appended here (`:1093-1100`), backspace pops one character (`:1079-1084`), and
    /// `tui.select.cancel` clears it *instead of* cancelling when it is non-empty (`:1032-1035`).
    /// It filters [`TreeSelector::visible_indices`] as whitespace-split lowercase tokens, all of
    /// which must be substrings of the row's searchable text (`:337`, `:391-393`).
    search_query: String,
}

impl TreeSelector {
    /// Build from a flattened node list (already in DAG pre-order).
    pub fn new(nodes: Vec<TreeNode>) -> Self {
        TreeSelector {
            nodes,
            selected: 0,
            filter: FilterMode::Default,
            show_time: false,
            keymap: TreeKeymap::default(),
            label_edit: None,
            search_query: String::new(),
        }
    }

    /// Override the tree bindings (JSON-configured `app.tree.*`).
    pub fn set_keymap(&mut self, keymap: TreeKeymap) {
        self.keymap = keymap;
    }

    /// The current search query (inspection/tests) — `getSearchQuery()` (`tree-selector.ts:619-621`).
    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// Whether `node` survives the live search — `applyFilter`'s search arm
    /// (`tree-selector.ts:337`, `:391-393`): the query is lowercased and split on whitespace, empty
    /// tokens dropped, and **every** token must appear as a substring of the node's searchable text.
    ///
    /// **[CYRUP-DELTA]** Upstream's `getSearchableText` (`:560-614`) joins the user label, the
    /// message role and the extracted content. cyrup's flattened [`TreeNode`] carries one
    /// pre-rendered row text ([`TreeNode::label`]) which the DAG display already builds *from* those
    /// same parts — the role prefix and the content preview are both in it (see the `S24(b)` note on
    /// `cyrup_test_support::TreeRole`) — so it is the available equivalent, not a narrowing choice. The one part it
    /// cannot carry is a user label whose text is not on the row: `SessionDagNode` exposes only
    /// `has_label`, the same limitation [`TreeSelector::begin_label_edit`] already documents.
    fn matches_search(&self, node: &TreeNode) -> bool {
        if self.search_query.trim().is_empty() {
            return true;
        }
        let haystack = node.label.to_lowercase();
        self.search_query
            .to_lowercase()
            .split_whitespace()
            .all(|token| haystack.contains(token))
    }

    /// The indices of currently-visible nodes (filter passes + search matches + not inside a folded
    /// subtree).
    pub fn visible_indices(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut active_fold: Option<usize> = None;
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(fd) = active_fold
                && n.depth <= fd
            {
                active_fold = None;
            }
            if active_fold.is_some() {
                continue;
            }
            if !self.filter.keeps(n) || !self.matches_search(n) {
                // A filtered-out foldable branch still suppresses its (also-filtered) descendants.
                if n.foldable && n.folded {
                    active_fold = Some(n.depth);
                }
                continue;
            }
            out.push(i);
            if n.foldable && n.folded {
                active_fold = Some(n.depth);
            }
        }
        out
    }

    /// The entry ids of the currently-visible rows, in display order (inspection/tests).
    pub fn visible_ids(&self) -> Vec<String> {
        self.visible_indices()
            .into_iter()
            .filter_map(|i| self.nodes.get(i).map(|n| n.id.clone()))
            .collect()
    }

    /// The currently-highlighted node's entry id, if any.
    pub fn selected_id(&self) -> Option<String> {
        let visible = self.visible_indices();
        visible.get(self.selected).and_then(|&i| self.nodes.get(i)).map(|n| n.id.clone())
    }

    /// Move the highlight to `id` if it is currently visible — Pi's `showTreeSelector(entryId)`
    /// re-show with the same selection (`interactive-mode.ts:4763,4807`). A no-op when the id is
    /// filtered out, folded away, or gone (the selection stays wherever it was), never a panic.
    pub fn select_id(&mut self, id: &str) {
        if let Some(pos) = self
            .visible_indices()
            .into_iter()
            .position(|i| self.nodes.get(i).is_some_and(|n| n.id == id))
        {
            self.selected = pos;
        }
    }

    /// The current filter mode (inspection/tests).
    pub fn filter(&self) -> FilterMode {
        self.filter
    }

    /// Set the filter mode and clamp the selection.
    pub fn set_filter(&mut self, mode: FilterMode) {
        self.filter = mode;
        self.clamp_selection();
    }

    /// Move the highlight up/down within the visible rows.
    fn move_by(&mut self, delta: isize) {
        let len = self.visible_indices().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, len as isize - 1) as usize;
    }

    /// Fold the selected node if foldable+open, else move up (`app.tree.foldOrUp`).
    fn fold_or_up(&mut self) {
        if let Some(idx) = self.selected_node_index()
            && let Some(node) = self.nodes.get(idx)
            && node.foldable
            && !node.folded
        {
            if let Some(n) = self.nodes.get_mut(idx) {
                n.folded = true;
            }
            self.clamp_selection();
            return;
        }
        self.move_by(-1);
    }

    /// Unfold the selected node if foldable+folded, else move down (`app.tree.unfoldOrDown`).
    fn unfold_or_down(&mut self) {
        if let Some(idx) = self.selected_node_index()
            && let Some(node) = self.nodes.get(idx)
            && node.foldable
            && node.folded
        {
            if let Some(n) = self.nodes.get_mut(idx) {
                n.folded = false;
            }
            return;
        }
        self.move_by(1);
    }

    /// The `nodes` index of the highlighted visible row.
    fn selected_node_index(&self) -> Option<usize> {
        self.visible_indices().get(self.selected).copied()
    }

    /// Keep `selected` within the (possibly shrunken) visible range.
    fn clamp_selection(&mut self) {
        let len = self.visible_indices().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// Toggle the relative-time column.
    fn toggle_time(&mut self) {
        self.show_time = !self.show_time;
    }

    /// Unfold every node — pi's `this.foldedNodes.clear()`, which **every** filter/search mutation
    /// runs before `applyFilter()` (`tree-selector.ts:1033`, `:1040`, `:1045`, and each of the seven
    /// filter arms and the backspace/typing arms).
    fn clear_folds(&mut self) {
        for node in &mut self.nodes {
            node.folded = false;
        }
    }

    /// Apply a filter mode the way pi's direct-filter arms do: set it, clear the folds, re-filter.
    fn apply_filter_mode(&mut self, mode: FilterMode) {
        self.filter = mode;
        self.clear_folds();
        self.clamp_selection();
    }

    /// pi's toggle arms (`tree-selector.ts:1039-1063`): a filter key whose mode is already active
    /// returns to `default`; otherwise it selects that mode. `app.tree.filter.default` is the one
    /// arm that is not a toggle (`:1036-1038`).
    fn toggle_filter_mode(&mut self, mode: FilterMode) {
        let next = if self.filter == mode { FilterMode::Default } else { mode };
        self.apply_filter_mode(next);
    }

    /// pi's cycle arms (`tree-selector.ts:1064-1076`) over the ordered mode list
    /// `["default","no-tools","user-only","labeled-only","all"]`, wrapping in both directions.
    fn cycle_filter(&mut self, forward: bool) {
        const MODES: [FilterMode; 5] = [
            FilterMode::Default,
            FilterMode::NoTools,
            FilterMode::UserOnly,
            FilterMode::LabeledOnly,
            FilterMode::All,
        ];
        let cur = MODES.iter().position(|m| *m == self.filter).unwrap_or(0);
        let next = if forward { (cur + 1) % MODES.len() } else { (cur + MODES.len() - 1) % MODES.len() };
        // `.get()` rather than `MODES[next]`: the index is already in range by construction (`% len`),
        // but the workspace denies `clippy::indexing_slicing` and this line was tripping it.
        if let Some(mode) = MODES.get(next) {
            self.apply_filter_mode(*mode);
        }
    }

    /// Begin inline label editing on the highlighted entry (`app.tree.editLabel` → `onLabelEdit` →
    /// `showLabelInput`, `tree-selector.ts:1046-1049,1327,1351`). A no-op when no row is selected. The
    /// buffer starts empty: cyrup's flattened DAG (`SessionDagNode`) carries only `has_label`, not the
    /// label text, so unlike Pi (which seeds `selected.node.label`) there is no existing string to
    /// pre-fill — the user types the new label from scratch.
    fn begin_label_edit(&mut self) {
        if let Some(entry_id) = self.selected_id() {
            self.label_edit = Some(LabelEdit { entry_id, query: String::new(), cursor: 0 });
        }
    }

    /// Set/clear the `has_label` star on the node whose entry id is `entry_id`, mirroring Pi's
    /// `TreeList.updateNodeLabel` (`:626-633`) local refresh so the tree reflects the rename the instant
    /// the label input closes (the persist to the session DAG is the chrome's job). An empty label
    /// clears the star (Pi stores `undefined`, `apply_label` drops empty labels).
    fn update_node_label(&mut self, entry_id: &str, has_label: bool) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == entry_id) {
            node.has_label = has_label;
        }
    }

    /// Route one key into the active [`LabelEdit`] buffer (Pi `LabelInput.handleInput`, `:1272-1282`):
    /// `confirm` trims + submits (empty ⇒ remove), `cancel` discards, everything printable types into
    /// the buffer. On submit it updates the local star and emits an [`SelectorOutcome::Apply`] carrying
    /// `"{entry_id}\u{1f}{label}"` — the chrome persists it via the session's `set_label` path and the
    /// selector stays open (Pi hides the input and returns to the tree, `:1353-1372`).
    fn handle_label_edit(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        match keymap.action_for(key) {
            Some(SelectAction::Confirm) => {
                let Some(edit) = self.label_edit.take() else { return SelectorOutcome::Redraw };
                let label = edit.query.trim().to_string();
                self.update_node_label(&edit.entry_id, !label.is_empty());
                SelectorOutcome::Apply(format!("{}{}{}", edit.entry_id, crate::FIELD_SEP, label))
            }
            Some(SelectAction::Cancel) => {
                self.label_edit = None;
                SelectorOutcome::Redraw
            }
            _ => {
                let Some(edit) = self.label_edit.as_mut() else { return SelectorOutcome::Redraw };
                match key.code {
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        edit.query.insert(edit.cursor, c);
                        edit.cursor += c.len_utf8();
                        SelectorOutcome::Redraw
                    }
                    KeyCode::Backspace => {
                        if edit.cursor > 0
                            && let Some(ch) = edit.query[..edit.cursor].chars().next_back()
                        {
                            let start = edit.cursor - ch.len_utf8();
                            edit.query.replace_range(start..edit.cursor, "");
                            edit.cursor = start;
                        }
                        SelectorOutcome::Redraw
                    }
                    _ => SelectorOutcome::Ignored,
                }
            }
        }
    }

    /// The label-input body lines (`LabelInput.render`, `tree-selector.ts:1256-1270`): a muted prompt,
    /// the live buffer with a visible caret, and the save/cancel hint — shown in the tree body while a
    /// rename is in progress.
    fn label_edit_lines(&self, edit: &LabelEdit, theme: &UiTheme) -> Vec<Line<'static>> {
        // S31: `LabelInput.render` splices the `Input`'s own line in behind a literal two-space
        // `indent` — `lines.push(...this.input.render(availableWidth).map(line => `${indent}${line}`))`
        // (`tree-selector.ts:1299,1302`) — so the row reads `"  " + "> " + value`. cyrup drew the
        // indent but dropped `Input.render`'s shared `"> "` prompt (`input.ts:380`) entirely.
        let mut input = vec![Span::raw("  ")];
        input.extend(crate::selector::input_line_spans(&edit.query, edit.cursor, theme));
        vec![
            Line::from(Span::styled("  Label (empty to remove):", theme.muted_style())),
            Line::from(input),
            Line::from(Span::styled("  enter save   esc cancel", theme.dim_style())),
        ]
    }

    /// Build the connector prefix for `node` at `nodes` index `idx` (depth-0 has none). Uses the
    /// "is this the last child of its parent" relation derived from the flat pre-order list.
    ///
    /// **The fold state lives INSIDE the connector.** Upstream builds the prefix one character at a
    /// time over `totalChars = displayIndent * 3` (`tree-selector.ts:701-729`); at the node's own
    /// connector level the three cells are
    ///
    /// * `posInLevel === 0` → `flatNode.isLast ? "└" : "├"` (`:719`)
    /// * `posInLevel === 1` → `isFolded ? "⊞" : foldable ? "⊟" : "─"` (`:721-722`)
    /// * `posInLevel === 2` → `" "` (`:724`)
    ///
    /// so a foldable-and-expanded child renders `├⊟ `, a folded one `├⊞ `, and a leaf `├─ `. The
    /// whole prefix is then styled `theme.fg("dim", prefix)` (`:746`), which is why the fold cell is
    /// dim here rather than accent — only the connector-less fallback marker at `:734` is accent.
    fn connector_prefix(&self, idx: usize) -> String {
        let Some(node) = self.nodes.get(idx) else { return String::new() };
        if node.depth == 0 {
            return String::new();
        }
        // For each ancestor level 1..depth, draw `│  ` if that ancestor has a following sibling,
        // else three spaces; then the node's own `├`/`└` + fold cell + ` `.
        let mut prefix = String::new();
        for level in 1..node.depth {
            if self.ancestor_has_following_sibling(idx, level) {
                prefix.push_str("│  ");
            } else {
                prefix.push_str("   ");
            }
        }
        prefix.push(if self.is_last_child(idx) { '└' } else { '├' });
        // `:722` verbatim — `isFolded` is tested FIRST and independently of `foldable`.
        prefix.push(if node.folded {
            '⊞'
        } else if node.foldable {
            '⊟'
        } else {
            '─'
        });
        prefix.push(' ');
        prefix
    }

    /// Whether the node at `idx` is the last child of its parent (no later node at the same depth
    /// before a shallower node).
    fn is_last_child(&self, idx: usize) -> bool {
        let Some(d) = self.nodes.get(idx).map(|n| n.depth) else { return true };
        for n in self.nodes.iter().skip(idx + 1) {
            if n.depth < d {
                return true;
            }
            if n.depth == d {
                return false;
            }
        }
        true
    }

    /// Whether the ancestor of `idx` at `level` has a following sibling (so a vertical bar continues).
    fn ancestor_has_following_sibling(&self, idx: usize, level: usize) -> bool {
        // Find the ancestor node index at `level` (nearest preceding node of depth==level).
        let mut anc = None;
        for j in (0..idx).rev() {
            if let Some(n) = self.nodes.get(j)
                && n.depth == level
            {
                anc = Some(j);
                break;
            }
            if self.nodes.get(j).map(|n| n.depth) < Some(level) {
                break;
            }
        }
        match anc {
            Some(a) => !self.is_last_child(a),
            None => false,
        }
    }

    /// The visible rows as styled lines for `width` (used by [`Selector::render`] and tests).
    pub fn rows(&self, width: u16, theme: &UiTheme) -> Vec<Line<'static>> {
        let visible = self.visible_indices();
        let mut lines = Vec::with_capacity(visible.len());
        for (row, &idx) in visible.iter().enumerate() {
            let Some(node) = self.nodes.get(idx) else { continue };
            let is_sel = row == self.selected;
            let mut spans: Vec<Span<'static>> = Vec::new();
            // Cursor gutter — `tree-selector.ts:689`:
            //   `const cursor = isSelected ? theme.fg("accent", "› ") : "  ";`
            // Upstream keeps this as a fixed 2-column gutter (`TREE_GUTTER_WIDTH`, `:49`) that the
            // horizontal viewport never clips (`:84-89`).
            spans.push(if is_sel {
                Span::styled("› ".to_string(), theme.accent_style())
            } else {
                Span::raw("  ")
            });
            // Connector prefix — this is also where the FOLD STATE is drawn (`:721-722`); see
            // `connector_prefix`.
            let prefix = self.connector_prefix(idx);
            let shows_fold_in_connector = !prefix.is_empty();
            if shows_fold_in_connector {
                spans.push(Span::styled(prefix, theme.dim_style()));
            }
            // The connector-less FALLBACK fold marker — `tree-selector.ts:733-734`:
            //   const showsFoldInConnector = flatNode.showConnector && !flatNode.isVirtualRootChild;
            //   const foldMarker = isFolded && !showsFoldInConnector ? theme.fg("accent", "⊞ ") : "";
            //
            // `!showsFoldInConnector` means "the connector did NOT already show the fold state", so
            // this branch is reached only by a node that HAS NO CONNECTOR — depth 0 here, a root or
            // a virtual-root child upstream. It is a fallback, not the general case, and it only
            // ever emits the FOLDED glyph: an expanded connector-less node draws nothing, because
            // there is no cell to put `⊟` in.
            if node.folded && !shows_fold_in_connector {
                spans.push(Span::styled("⊞ ", theme.accent_style()));
            }
            // Glyph + label.
            let glyph_style = if is_sel { theme.accent_style() } else { theme.base_style() };
            spans.push(Span::styled(format!("{} ", node.kind.glyph()), glyph_style));
            // S24(b): the entry text is coloured PER ROLE, with a coloured role prefix, and the
            // selected row is only BOLDED (`:851`), never repainted `accent` — see
            // [`TreeEntryRole`].
            spans.extend(
                TreeEntryRole::classify(node.kind, &node.label).spans(&node.label, is_sel, theme),
            );
            if node.has_label {
                spans.push(Span::styled("  ☆labeled".to_string(), theme.warning_style()));
            }
            // Right-aligned label-timestamp column.
            //
            // S37: this used to ALSO render a `◀ selected` marker on the highlighted row, padded
            // flush right. There is no upstream analog — `git grep '◀' v0.84.1 -- packages/` finds
            // nothing anywhere in pi, and `renderHorizontalViewport` (`tree-selector.ts:85-91`)
            // emits `row.gutter + row.body` truncated to `width` with no right-hand padding at all,
            // so an upstream row is exactly as wide as its content. The marker was a cyrup
            // invention that (a) added text pi never draws and (b) padded the row out to `width`,
            // which is what made the `selectedBg` fill below look full-width. It is removed; the
            // selection is indicated the way upstream indicates it — the `› ` cursor at `:689` plus
            // the fill at `:750-753`.
            //
            // S24(a): the pad is computed from the row's **visible width**, not `chars().count()`.
            // Upstream measures the same quantity with `visibleWidth` (`tree-selector.ts:747`
            // `const anchorCol = visibleWidth(prefixPart);`, `:754` `bodyWidth: visibleWidth(body)`),
            // and everything ahead of this column is unicode: the connector `│├└─⊟⊞`, the glyphs
            // `●◆◇⚙✓`, the `☆labeled` star, and a message preview that is arbitrary user text. A CJK
            // preview measured one column per character, so the pad overshot by the number of wide
            // characters in the row and pushed the timestamp off the right edge. `Span::width` is
            // the crate's `visibleWidth`; this was the eighth char-count measurement found in it.
            let left_len: usize = spans.iter().map(Span::width).sum();
            let mut right = String::new();
            if self.show_time
                // Pi's render condition in full (`tree-selector.ts:741-743`): the column is a
                // *label* timestamp, so an entry with no label never shows one even when the toggle
                // is on and a timestamp happens to be attached.
                && node.has_label
                && let Some(t) = &node.time_label
            {
                right.push_str(t);
            }
            if !right.is_empty() {
                let pad =
                    (width as usize).saturating_sub(left_len + Span::raw(&right).width() + 1);
                spans.push(Span::raw(" ".repeat(pad + 1)));
                let style = if is_sel { theme.accent_style() } else { theme.dim_style() };
                spans.push(Span::styled(right, style));
            }
            // S2/SYS-4: the selected row carries the `selectedBg` fill. `tree-selector.ts:750-753`
            //     if (isSelected) { gutter = theme.bg("selectedBg", gutter); body = theme.bg("selectedBg", body); }
            // wraps the already-styled gutter and body — the fill is laid OVER the per-span
            // foregrounds, it does not replace them, and it stops at the end of the body: upstream
            // does NOT pad the row out to `width` (`:85-91`), so the bar is content-wide, not
            // full-width. This is one of only two places upstream fills a selection background;
            // cyrup previously drew it in `SelectList`, where upstream never does, and omitted it
            // here.
            if is_sel {
                for span in &mut spans {
                    span.style = theme.selected_bg_over(span.style);
                }
            }
            lines.push(Line::from(spans));
        }
        lines
    }

    /// The header line: `Session Tree` + `Filter: <mode>   (visible/total)`, plus Pi's
    /// `[+label time]` marker while the label-timestamp column is on (`getStatusLabels`,
    /// `tree-selector.ts:658-660`).
    ///
    /// That marker is what makes the `t` toggle discoverable at all now that the column is off by
    /// default and empty on every unlabeled row: without it, pressing `t` in a session with no
    /// labels changes nothing visible and reads as a dead key.
    fn header(&self, theme: &UiTheme) -> Line<'static> {
        let visible = self.visible_indices().len();
        let total = self.nodes.len();
        let label_time = if self.show_time { " [+label time]" } else { "" };
        Line::from(vec![
            Span::styled(
                " Session Tree".to_string(),
                theme.accent_style().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   Filter: {}   ({visible}/{total}){label_time}", self.filter.label()),
                theme.dim_style(),
            ),
        ])
    }

    /// pi's `SearchLine.render` (`tree-selector.ts:1165-1172`): the muted `Type to search:` prompt
    /// unconditionally, plus the live query in `accent` when there is one.
    fn search_line(&self, theme: &UiTheme) -> Line<'static> {
        let mut spans =
            vec![Span::styled("  Type to search:".to_string(), theme.dim_style())];
        if !self.search_query.is_empty() {
            spans.push(Span::styled(format!(" {}", self.search_query), theme.accent_style()));
        }
        Line::from(spans)
    }

    /// The help row, resolved from the **live** [`TreeKeymap`] — pi's `TreeHelp` over
    /// `TREE_HELP_ITEMS` (`tree-selector.ts:1215-1236`), joined with pi's ` · ` separator. An
    /// unbound action contributes nothing, as `formatHelpKeys` returns `""` for it.
    ///
    /// The `move` and `page` cells stay literal: those two rows read `tui.select.*` /
    /// `tui.editor.cursor*` upstream, and cyrup's [`Selector::render`] signature carries no
    /// [`SelectKeymap`], so there is nothing live to resolve them from here.
    fn help_text(&self) -> String {
        let mut items: Vec<String> = vec!["↑/↓ move".to_string(), "←/→ page".to_string()];
        let pair = |a: TreeAction, b: TreeAction| -> Option<String> {
            match (self.keymap.first_key_label(a), self.keymap.first_key_label(b)) {
                (Some(x), Some(y)) => Some(format!("{x}/{y}")),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            }
        };
        if let Some(k) = pair(TreeAction::FoldOrUp, TreeAction::UnfoldOrDown) {
            items.push(format!("{k} branch"));
        }
        if let Some(k) = self.keymap.first_key_label(TreeAction::EditLabel) {
            items.push(format!("{k} label"));
        }
        if let Some(k) = self.keymap.first_key_label(TreeAction::ToggleLabelTimestamp) {
            items.push(format!("{k} label time"));
        }
        let filters: Vec<String> = [
            TreeAction::FilterDefault,
            TreeAction::FilterNoTools,
            TreeAction::FilterUserOnly,
            TreeAction::FilterLabeledOnly,
            TreeAction::FilterAll,
        ]
        .into_iter()
        .filter_map(|a| self.keymap.first_key_label(a))
        .collect();
        if !filters.is_empty() {
            items.push(format!("filters {}", filters.join("/")));
        }
        if let Some(k) =
            pair(TreeAction::FilterCycleForward, TreeAction::FilterCycleBackward)
        {
            items.push(format!("cycle {k}"));
        }
        format!(" {}", items.join(" · "))
    }
}

impl Selector for TreeSelector {
    fn desired_height(&self, _width: u16) -> u16 {
        // top rule + header + search line + body + hint line + bottom rule (+ one slack row).
        let body = if self.label_edit.is_some() {
            // The label editor occupies the body (prompt + input + hint): at least 3 rows.
            3
        } else {
            self.visible_indices().len().min(u16::MAX as usize) as u16
        };
        body.saturating_add(6)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let [top, header, search, body, hint, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(border_rule(top.width, theme), top);
        frame.render_widget(Paragraph::new(self.header(theme)), header);
        // The standing `Type to search:` prompt plus the live query — pi's `SearchLine`
        // (`tree-selector.ts:1155-1173`), which renders the muted prompt whether or not a query is
        // present and appends the query in `accent`. Placed between the header and the tree body,
        // as pi's container order does (`:1375-1381`).
        frame.render_widget(Paragraph::new(self.search_line(theme)), search);
        // While renaming, the body shows the inline label editor (Pi swaps `treeContainer` for the
        // `labelInputContainer`, `tree-selector.ts:1363-1372`); otherwise the filtered tree rows.
        if let Some(edit) = &self.label_edit {
            frame.render_widget(
                Paragraph::new(self.label_edit_lines(edit, theme)).style(theme.base_style()),
                body,
            );
        } else {
            frame.render_widget(
                Paragraph::new(self.rows(body.width, theme)).style(theme.base_style()),
                body,
            );
        }
        let hint_line = if self.label_edit.is_some() {
            Line::from(Span::styled(
                " editing label — enter save   esc cancel".to_string(),
                theme.dim_style(),
            ))
        } else {
            Line::from(Span::styled(self.help_text(), theme.dim_style()))
        };
        frame.render_widget(Paragraph::new(hint_line), hint);
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // While the inline label editor is open it captures ALL keys (Pi `if (this.labelInput)`,
        // `tree-selector.ts:1373-1379`) so `e`/`z`/digits type literally instead of firing tree actions.
        if self.label_edit.is_some() {
            return self.handle_label_edit(key, keymap);
        }
        // Bespoke tree bindings take precedence over the shared select map.
        if let Some(action) = self.keymap.action_for(key) {
            match action {
                TreeAction::FoldOrUp => self.fold_or_up(),
                TreeAction::UnfoldOrDown => self.unfold_or_down(),
                TreeAction::ToggleLabelTimestamp => self.toggle_time(),
                // Open the inline label editor on the selected entry (`onLabelEdit` → `showLabelInput`).
                TreeAction::EditLabel => self.begin_label_edit(),
                TreeAction::FilterDefault => self.apply_filter_mode(FilterMode::Default),
                TreeAction::FilterNoTools => self.toggle_filter_mode(FilterMode::NoTools),
                TreeAction::FilterUserOnly => self.toggle_filter_mode(FilterMode::UserOnly),
                TreeAction::FilterLabeledOnly => self.toggle_filter_mode(FilterMode::LabeledOnly),
                TreeAction::FilterAll => self.toggle_filter_mode(FilterMode::All),
                TreeAction::FilterCycleForward => self.cycle_filter(true),
                TreeAction::FilterCycleBackward => self.cycle_filter(false),
            }
            return SelectorOutcome::Redraw;
        }
        match keymap.action_for(key) {
            Some(SelectAction::Up) | Some(SelectAction::PageUp) => {
                self.move_by(-1);
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Down) | Some(SelectAction::PageDown) => {
                self.move_by(1);
                SelectorOutcome::Redraw
            }
            Some(SelectAction::Confirm) => match self.selected_id() {
                Some(id) => SelectorOutcome::Confirm(id),
                None => SelectorOutcome::Redraw,
            },
            // `tui.select.cancel` clears a live search FIRST and only cancels the selector when the
            // query is already empty (`tree-selector.ts:1031-1037`).
            Some(SelectAction::Cancel) => {
                if self.search_query.is_empty() {
                    SelectorOutcome::Cancel
                } else {
                    self.search_query.clear();
                    self.clear_folds();
                    self.clamp_selection();
                    SelectorOutcome::Redraw
                }
            }
            None => {
                // Backspace pops one character off the query (`tree-selector.ts:1078-1084`).
                if key.code == KeyCode::Backspace {
                    if self.search_query.pop().is_some() {
                        self.clear_folds();
                        self.clamp_selection();
                        return SelectorOutcome::Redraw;
                    }
                    return SelectorOutcome::Ignored;
                }
                // Otherwise: the final `else` of `handleInput` (`:1093-1100`) — any printable key
                // data with no control characters is appended to the search query. cyrup's event
                // model gives one `KeyCode::Char` at a time, and pi's `hasControlChars` test rejects
                // `code < 32 || code === 0x7f || (0x80..=0x9f)`, which is exactly
                // `char::is_control` for the codepoints a terminal can deliver here. A key carrying
                // CONTROL/ALT/SUPER is a chord, not text, so it is not search input either.
                if let KeyCode::Char(c) = key.code
                    && !c.is_control()
                    && !key.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    )
                {
                    self.search_query.push(c);
                    self.clear_folds();
                    self.clamp_selection();
                    return SelectorOutcome::Redraw;
                }
                SelectorOutcome::Ignored
            }
        }
    }
}

/// A full-width `─` rule styled `border` (Pi `DynamicBorder`; mirrors `selector::border_rule`).
fn border_rule(width: u16, theme: &UiTheme) -> Paragraph<'static> {
    let rule = "─".repeat(width.max(1) as usize);
    Paragraph::new(Line::from(Span::styled(rule, theme.border_style())))
}
