//! Mirrors `tmp/herdr/src/api/schema/agents.rs` — the cross-pane agent roster.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::common::AgentStatus;

/// `AgentInfo` (`tmp/herdr/src/api/schema/agents.rs:186-226`) — one occupant of one pane, as
/// herdr sees it.
///
/// Answered by `agent.get`, listed by `agent.list`, and carried in bulk by `session.snapshot`.
///
/// **Keyed on `terminal_id`, not `pane_id`.** `terminal_id` is the first field and the identity
/// herdr's own target resolver answers with (`tmp/herdr/src/app/agents.rs:298-321` names
/// `terminal_id` first in an ambiguity message); `pane_id` is where that terminal is *currently*
/// displayed. A pane can be closed and its terminal survive, so a fleet roster that keys on
/// `pane_id` loses the agent the moment the user rearranges their panes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentInfo {
    /// The terminal this agent runs in — the stable identity.
    pub terminal_id: String,
    /// The user's own name for this agent (`agent.rename`).
    #[serde(default)]
    pub name: Option<String>,
    /// The agent label herdr detected or was told.
    #[serde(default)]
    pub agent: Option<String>,
    /// The title reported through `pane.report_metadata`.
    #[serde(default)]
    pub title: Option<String>,
    /// The title the program set through the terminal's own escape sequence.
    #[serde(default)]
    pub terminal_title: Option<String>,
    /// `terminal_title` with herdr's decoration removed.
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    /// The agent name to display.
    #[serde(default)]
    pub display_agent: Option<String>,
    /// The observed status — five values. See [`AgentStatus`].
    pub agent_status: AgentStatus,
    /// `true` when an authority's report is standing in for screen detection, which herdr
    /// therefore skipped.
    #[serde(default)]
    pub screen_detection_skipped: bool,
    /// Per-state label overrides.
    #[serde(default)]
    pub state_labels: BTreeMap<String, String>,
    /// Reported tokens, already resolved.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    /// The resumable session bound to this agent.
    #[serde(default)]
    pub agent_session: Option<AgentSessionInfo>,
    /// The workspace the agent's pane is in.
    pub workspace_id: String,
    /// The tab the agent's pane is in.
    pub tab_id: String,
    /// The pane the agent is currently displayed in.
    pub pane_id: String,
    /// Whether that pane has focus.
    pub focused: bool,
    /// `true` between `agent.start` and the agent actually coming up.
    #[serde(default)]
    pub launch_pending: bool,
    /// `true` once herdr has seen the agent accept input.
    #[serde(default)]
    pub interactive_ready: bool,
    /// Bumps on every *status* change. Distinct from `revision`, which bumps on every change at
    /// all — this one is what an attention-ordered view sorts on
    /// (`AgentViewBuiltinSortField::StateChangeSeq`, `agents.rs:153`).
    #[serde(default)]
    pub state_change_seq: u64,
    /// The pane's launch directory.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The foreground process's directory, when herdr can see it.
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    /// Bumps on every change — the SAME live terminal counter as
    /// [`super::panes::PaneInfo::revision`], not a second one: herdr copies it off the pane record
    /// this agent record is built from (`revision: pane.revision`,
    /// `tmp/herdr/src/app/agents.rs:400`). Safe to key a cache on, for the same reason and with
    /// the same caveats.
    pub revision: u64,
}

/// `AgentSessionInfo` (`tmp/herdr/src/api/schema/agents.rs:228-234`) — a resumable agent session,
/// as reported by `pane.report_agent_session` and echoed back on the records.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentSessionInfo {
    /// The authority that reported it.
    pub source: String,
    /// The agent label.
    pub agent: String,
    /// Whether `value` is an id or a path.
    pub kind: AgentSessionRefKind,
    /// The id or the path, per `kind`.
    pub value: String,
}

/// `AgentSessionRefKind` (`tmp/herdr/src/agent_resume.rs:14-19`), `rename_all = "snake_case"` —
/// on the wire `"id"` or `"path"`.
///
/// It lives outside `api/schema/` in herdr, in the module that owns resume plans, and is pulled
/// into [`AgentSessionInfo`] by a fully qualified path (`agents.rs:232`). Mirrored here rather
/// than in a file named after `agent_resume.rs`, because this is the only member of that module
/// that reaches the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionRefKind {
    /// `value` is a session id.
    Id,
    /// `value` is a session file path.
    Path,
}

// =================================================================================================
// `agent.view.set` / `agent.view.clear` — the sidebar projection
// =================================================================================================

/// `AgentViewSetParams` (`tmp/herdr/src/api/schema/agents.rs:52-61`).
///
/// One TRANSIENT declarative projection over herdr's built-in Agents view. herdr re-evaluates it
/// "whenever agent facts or current UI context change", and it controls "the expanded and
/// collapsed sidebar, mobile Agents list, mouse targets, indexed focus, and next/previous Agent
/// navigation" — and deliberately **not** `agent.list`, notifications, detection or the global
/// attention counts (`socket-api.mdx:418-425`).
///
/// There is exactly ONE view server-wide: `state.agent_view_override` is a single `Option`
/// (`tmp/herdr/src/app/api/agent_view.rs:88-89`) and *"a successful set atomically replaces the
/// previous view"* (`socket-api.mdx:481`). It lasts until it is cleared, replaced, its owning
/// plugin is disabled, or the server exits — herdr persists nothing.
///
/// # What herdr refuses
///
/// `validate_agent_view` (`tmp/herdr/src/app/agent_view.rs:19-39`) runs before anything is stored,
/// and every failure is [`crate::ApiErrorCode::InvalidAgentView`]:
///
/// * [`Self::source`] — trimmed, non-empty, at most 120 characters, and only
///   `[A-Za-z0-9:._-]` (`:178-191`). A `plugin:<id>` source is additionally checked against the
///   installed plugin list and answers `plugin_not_found` / `plugin_disabled`
///   (`api/agent_view.rs:15-28`); a non-`plugin:` source — which is what cyrup uses — skips that.
/// * [`Self::label`] — trimmed, control characters stripped, non-empty, at most 32 characters
///   (`:193-205`).
/// * [`Self::filter`] — at most 8 levels deep and 64 nodes; `all`/`any` must be non-empty; `in`
///   takes 1..=32 values (`:207-247`).
/// * [`Self::sort`] — at most 8 fields (`:29-37`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentViewSetParams {
    /// Who owns this view. Plugins use `plugin:<HERDR_PLUGIN_ID>`; every other caller uses its own
    /// non-`plugin:` name, which is what makes [`AgentViewClearParams::source`]'s ownership check
    /// meaningful.
    pub source: String,
    /// The short name herdr shows for the active view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Which agents the view keeps. `None` keeps all of them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<AgentViewFilter>,
    /// How they are ordered. **Empty means "leave `ui.agent_panel_sort` alone"**
    /// (`socket-api.mdx:475-477`), not "sort by nothing", which is why it is skipped when empty
    /// rather than sent as `[]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<AgentViewSort>,
}

impl AgentViewSetParams {
    /// A view owned by `source`, with no filter and no sort.
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            label: None,
            filter: None,
            sort: Vec::new(),
        }
    }
}

/// `AgentViewClearParams` (`tmp/herdr/src/api/schema/agents.rs:63-68`).
///
/// `source: None` clears whatever view is active. `Some(source)` clears it **only if that source
/// still owns it** — *"a source mismatch leaves the active view unchanged"*
/// (`socket-api.mdx:494`, implemented at `tmp/herdr/src/app/api/agent_view.rs:49-62`). That is the
/// shape a well-behaved caller uses on shutdown: it must not delete a view some other program
/// installed after its own was replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct AgentViewClearParams {
    /// Clear only if this source still owns the view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl AgentViewClearParams {
    /// Clear only the view `source` still owns.
    #[must_use]
    pub fn owned_by(source: impl Into<String>) -> Self {
        Self {
            source: Some(source.into()),
        }
    }
}

/// `AgentViewFilter` (`tmp/herdr/src/api/schema/agents.rs:70-91`), tagged `op`.
///
/// The six operators of `socket-api.mdx:459`: `all`, `any`, `not`, `eq`, `in`, `exists`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AgentViewFilter {
    /// Every branch matches. Must be non-empty.
    All {
        /// The branches.
        filters: Vec<AgentViewFilter>,
    },
    /// Any branch matches. Must be non-empty.
    Any {
        /// The branches.
        filters: Vec<AgentViewFilter>,
    },
    /// The branch does not match.
    Not {
        /// The branch.
        filter: Box<AgentViewFilter>,
    },
    /// The field equals the value.
    Eq {
        /// The field.
        field: AgentViewField,
        /// The value.
        value: AgentViewValue,
    },
    /// The field is one of 1..=32 values.
    In {
        /// The field.
        field: AgentViewField,
        /// The values.
        values: Vec<AgentViewValue>,
    },
    /// The field is present at all.
    Exists {
        /// The field.
        field: AgentViewField,
    },
}

/// `AgentViewField` (`:93-97`) — a built-in field, or a plugin-reported metadata token.
///
/// `#[serde(untagged)]` is herdr's own: a built-in serialises as the bare string
/// (`"workspace_id"`) and a token as `{"token":"name"}` (`socket-api.mdx:461-462`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum AgentViewField {
    /// One of herdr's seven.
    Builtin(AgentViewBuiltinField),
    /// A `pane.report_metadata` token — the same keyspace
    /// [`super::panes::PaneReportMetadataParams::tokens`] writes.
    Token {
        /// The token key.
        token: String,
    },
}

/// `AgentViewBuiltinField` (`:99-109`) — the seven filterable fields of `socket-api.mdx:460-461`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentViewBuiltinField {
    /// The EFFECTIVE status: `idle`, `working`, `blocked`, `done` or `unknown`, where `done`
    /// means "idle and not yet seen" (`socket-api.mdx:469-470`). It is NOT
    /// [`super::common::PaneAgentState`], which is what a reporter may SEND — `done` and
    /// `unknown` are herdr's to derive.
    Status,
    /// The agent's workspace.
    WorkspaceId,
    /// The agent's tab.
    TabId,
    /// The agent's pane.
    PaneId,
    /// The agent label.
    Agent,
    /// Whether the user has looked at it since its last state change.
    Seen,
    /// herdr's monotonic state-transition counter.
    StateChangeSeq,
}

/// `AgentViewValue` (`:111-118`) — a string, a bool, a number, or a UI-context reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum AgentViewValue {
    /// A literal string.
    String(String),
    /// A literal boolean.
    Bool(bool),
    /// A literal unsigned number.
    Number(u64),
    /// "whatever the UI is currently showing" — see [`AgentViewContext`].
    Context {
        /// The context key.
        context: AgentViewContext,
    },
}

/// `AgentViewContext` (`:120-126`) — the two values herdr resolves at evaluation time.
///
/// Each *"may only be compared to the matching ID field"* (`socket-api.mdx:463-464`):
/// [`Self::CurrentWorkspaceId`] against [`AgentViewBuiltinField::WorkspaceId`] and
/// [`Self::CurrentTabId`] against [`AgentViewBuiltinField::TabId`]. Anything else is
/// [`crate::ApiErrorCode::InvalidAgentView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentViewContext {
    /// The workspace the user is looking at.
    CurrentWorkspaceId,
    /// The tab the user is looking at.
    CurrentTabId,
}

/// `AgentViewSort` (`:128-133`). Sorts are stable and evaluated in order; missing values sort
/// after present ones (`socket-api.mdx:473-475`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentViewSort {
    /// What to order by.
    pub field: AgentViewSortField,
    /// Which direction. `asc` is herdr's default and is serialised explicitly here because herdr
    /// declares the field `#[serde(default)]` rather than skipping it.
    pub order: AgentViewSortOrder,
}

impl AgentViewSort {
    /// Descending on a built-in field — the direction "most urgent first" wants.
    #[must_use]
    pub const fn desc(field: AgentViewBuiltinSortField) -> Self {
        Self {
            field: AgentViewSortField::Builtin(field),
            order: AgentViewSortOrder::Desc,
        }
    }
}

/// `AgentViewSortField` (`:135-139`) — a built-in, or a metadata token, untagged as herdr has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum AgentViewSortField {
    /// One of herdr's eight.
    Builtin(AgentViewBuiltinSortField),
    /// A `pane.report_metadata` token.
    Token {
        /// The token key.
        token: String,
    },
}

/// `AgentViewBuiltinSortField` (`:141-152`) — the eight of `socket-api.mdx:472-473`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentViewBuiltinSortField {
    /// Workspace order in the sidebar.
    WorkspaceOrder,
    /// Tab order within the workspace.
    TabOrder,
    /// Pane order within the tab.
    PaneOrder,
    /// herdr's own attention priority — `tab_attention_priority`
    /// (`tmp/herdr/src/app/api_helpers.rs`), the same rank its default "priority" panel sort uses.
    Attention,
    /// The effective status.
    Status,
    /// The agent label.
    Agent,
    /// Whether it has been seen.
    Seen,
    /// The state-transition counter — "most recently changed first" when descending.
    StateChangeSeq,
}

/// `AgentViewSortOrder` (`:154-162`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentViewSortOrder {
    /// Ascending — herdr's default.
    #[default]
    Asc,
    /// Descending.
    Desc,
}

/// What `agent.view.set` and `agent.view.clear` both answer with — herdr's
/// `ResponseResult::AgentView` (`tmp/herdr/src/api/schema/response.rs:110-116`), lifted out of the
/// inline variant so a caller can hold one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentView {
    /// Whether a projection is installed at all.
    pub active: bool,
    /// Who owns it.
    pub source: Option<String>,
    /// Its label.
    pub label: Option<String>,
}
