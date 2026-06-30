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

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
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

    /// Map a `1..=5` digit to a mode (the bespoke filter keys).
    pub fn from_digit(d: char) -> Option<FilterMode> {
        match d {
            '1' => Some(FilterMode::Default),
            '2' => Some(FilterMode::NoTools),
            '3' => Some(FilterMode::UserOnly),
            '4' => Some(FilterMode::LabeledOnly),
            '5' => Some(FilterMode::All),
            _ => None,
        }
    }

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
    /// The right-aligned relative-time column (e.g. `12:04`, `4h ago`); `None` hides it.
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

/// The session-tree navigator selector (the bespoke `/tree` layout). Holds the full flattened node
/// list plus selection/filter/timestamp UI state; renders only the *visible* rows (filter + folds).
pub struct TreeSelector {
    nodes: Vec<TreeNode>,
    /// Index into the **visible** rows (filter + fold applied).
    selected: usize,
    filter: FilterMode,
    /// Whether the time column is shown (`app.tree.toggleLabelTimestamp`).
    show_time: bool,
    keymap: TreeKeymap,
}

impl TreeSelector {
    /// Build from a flattened node list (already in DAG pre-order).
    pub fn new(nodes: Vec<TreeNode>) -> Self {
        TreeSelector { nodes, selected: 0, filter: FilterMode::Default, show_time: true, keymap: TreeKeymap::default() }
    }

    /// Override the tree bindings (JSON-configured `app.tree.*`).
    pub fn set_keymap(&mut self, keymap: TreeKeymap) {
        self.keymap = keymap;
    }

    /// The indices of currently-visible nodes (filter passes + not inside a folded subtree).
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
            if !self.filter.keeps(n) {
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

    /// Build the connector prefix for `node` at `nodes` index `idx` (depth-0 has none). Uses the
    /// "is this the last child of its parent" relation derived from the flat pre-order list.
    fn connector_prefix(&self, idx: usize) -> String {
        let Some(node) = self.nodes.get(idx) else { return String::new() };
        if node.depth == 0 {
            return String::new();
        }
        // For each ancestor level 1..depth, draw `│  ` if that ancestor has a following sibling,
        // else three spaces; then the node's own `├─ `/`└─ `.
        let mut prefix = String::new();
        for level in 1..node.depth {
            if self.ancestor_has_following_sibling(idx, level) {
                prefix.push_str("│  ");
            } else {
                prefix.push_str("   ");
            }
        }
        if self.is_last_child(idx) {
            prefix.push_str("└─ ");
        } else {
            prefix.push_str("├─ ");
        }
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
            // Connector prefix.
            let prefix = self.connector_prefix(idx);
            if !prefix.is_empty() {
                spans.push(Span::styled(prefix, theme.dim_style()));
            }
            // Fold marker.
            if node.foldable {
                let marker = if node.folded { "⊞ " } else { "⊟ " };
                spans.push(Span::styled(marker, theme.muted_style()));
            }
            // Glyph + label.
            let glyph_style = if is_sel { theme.accent_style() } else { theme.base_style() };
            spans.push(Span::styled(format!("{} ", node.kind.glyph()), glyph_style));
            let label_style = if is_sel {
                theme.accent_style().add_modifier(Modifier::BOLD)
            } else {
                theme.base_style()
            };
            spans.push(Span::styled(node.label.clone(), label_style));
            if node.has_label {
                spans.push(Span::styled("  ☆labeled".to_string(), theme.warning_style()));
            }
            // Right-aligned time column + selected marker.
            let left_len: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let mut right = String::new();
            if is_sel {
                right.push_str("◀ selected");
            } else if self.show_time
                && let Some(t) = &node.time_label
            {
                right.push_str(t);
            }
            if !right.is_empty() {
                let pad = (width as usize).saturating_sub(left_len + right.chars().count() + 1);
                spans.push(Span::raw(" ".repeat(pad + 1)));
                let style = if is_sel { theme.accent_style() } else { theme.dim_style() };
                spans.push(Span::styled(right, style));
            }
            lines.push(Line::from(spans));
        }
        lines
    }

    /// The header line: `Session Tree` + `Filter: <mode>   (visible/total)`.
    fn header(&self, theme: &UiTheme) -> Line<'static> {
        let visible = self.visible_indices().len();
        let total = self.nodes.len();
        Line::from(vec![
            Span::styled(
                " Session Tree".to_string(),
                theme.accent_style().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   Filter: {}   ({visible}/{total})", self.filter.label()),
                theme.dim_style(),
            ),
        ])
    }
}

impl Selector for TreeSelector {
    fn desired_height(&self, _width: u16) -> u16 {
        // top rule + header + body + two hint lines + bottom rule.
        let body = self.visible_indices().len().min(u16::MAX as usize) as u16;
        body.saturating_add(5)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let [top, header, body, hint, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(border_rule(top.width, theme), top);
        frame.render_widget(Paragraph::new(self.header(theme)), header);
        frame.render_widget(
            Paragraph::new(self.rows(body.width, theme)).style(theme.base_style()),
            body,
        );
        let hint_line = Line::from(Span::styled(
            " ↑/↓ move   ←/→ page   z/x branch   e label   t label time".to_string(),
            theme.dim_style(),
        ));
        frame.render_widget(Paragraph::new(hint_line), hint);
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // Bespoke tree bindings take precedence over the shared select map.
        if let Some(action) = self.keymap.action_for(key) {
            match action {
                TreeAction::FoldOrUp => self.fold_or_up(),
                TreeAction::UnfoldOrDown => self.unfold_or_down(),
                TreeAction::ToggleLabelTimestamp => self.toggle_time(),
                // Inline label edit needs the embedded `LabelInput` sub-state (residual); no-op here.
                TreeAction::EditLabel => {}
            }
            return SelectorOutcome::Redraw;
        }
        // Filter digits `1-5`.
        if let ratatui::crossterm::event::KeyCode::Char(c) = key.code
            && let Some(mode) = FilterMode::from_digit(c)
        {
            self.set_filter(mode);
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
            Some(SelectAction::Cancel) => SelectorOutcome::Cancel,
            None => SelectorOutcome::Ignored,
        }
    }
}

/// A full-width `─` rule styled `border` (Pi `DynamicBorder`; mirrors `selector::border_rule`).
fn border_rule(width: u16, theme: &UiTheme) -> Paragraph<'static> {
    let rule = "─".repeat(width.max(1) as usize);
    Paragraph::new(Line::from(Span::styled(rule, theme.border_style())))
}
