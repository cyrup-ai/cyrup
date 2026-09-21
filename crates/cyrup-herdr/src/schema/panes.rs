//! Mirrors `tmp/herdr/src/api/schema/panes.rs` — the pane params and the pane records.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::agents::AgentSessionInfo;
use super::common::{
    AgentStatus, PaneAgentState, ReadFormat, ReadSource, SplitDirection, default_true,
};

/// `PaneRightClickTarget` (`tmp/herdr/src/api/schema/panes.rs:16-24`), `rename_all = "snake_case"`.
///
/// Whether a right click inside the new pane belongs to herdr's own menu or is passed through to
/// the program running in it (`tmp/herdr/src/app/api/panes.rs:107-111`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneRightClickTarget {
    /// herdr keeps the right click. herdr's own default (`#[default]`, `panes.rs:21`).
    #[default]
    Herdr,
    /// The right click is delivered to the pane's program.
    Pane,
}

/// `PaneSplitParams` (`tmp/herdr/src/api/schema/panes.rs:26-43`).
///
/// Only `direction` is required — the published schema's `"required"` is `["direction"]`. With no
/// `target_pane_id` and no `workspace_id`, herdr splits the **focused** pane of the active
/// workspace (`tmp/herdr/src/app/api/panes.rs:35-47`).
///
/// `env` and `ratio` are the two reasons a native client beats `herdr pane split` for cyrup's
/// purposes: `env` is how a spawned pane learns which cyrup run it belongs to, and `ratio` is
/// honoured only when present (`split_pane_with_ratio` vs `split_pane`,
/// `tmp/herdr/src/app/api/panes.rs:73-100`). herdr validates `env` itself and answers
/// `invalid_env` for an empty key, an `=` in a key, or a NUL in either half
/// (`tmp/herdr/src/app/api/env.rs:3-31`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaneSplitParams {
    /// Split the focused pane of this workspace. Ignored when `target_pane_id` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Split this pane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_pane_id: Option<String>,
    /// Where the new pane lands.
    pub direction: SplitDirection,
    /// The new pane's share of the split, `0.0..=1.0`. Absent ⇒ herdr's own even split.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f32>,
    /// The new pane's working directory. Absent ⇒ herdr's `follow` cwd policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Focus the new pane.
    pub focus: bool,
    /// Who owns a right click in the new pane.
    pub right_click: PaneRightClickTarget,
    /// Extra environment for the new pane's shell.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl PaneSplitParams {
    /// A split in `direction` of whichever pane herdr considers current.
    #[must_use]
    pub fn new(direction: SplitDirection) -> Self {
        Self {
            workspace_id: None,
            target_pane_id: None,
            direction,
            ratio: None,
            cwd: None,
            focus: false,
            right_click: PaneRightClickTarget::Herdr,
            env: BTreeMap::new(),
        }
    }
}

/// `PaneListParams` (`tmp/herdr/src/api/schema/panes.rs:314-319`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PaneListParams {
    /// List only this workspace's panes. Absent ⇒ every pane in the session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// `PaneCurrentParams` (`tmp/herdr/src/api/schema/panes.rs:320-326`).
///
/// The field is `caller_pane_id`, not `pane_id`: it lets a process **inside** a pane say which
/// pane it is, so `pane.current` answers *its* pane rather than whichever one the user happens to
/// have focused (`tmp/herdr/src/app/api/panes.rs:145-149`). `HERDR_PANE_ID` is exactly that value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PaneCurrentParams {
    /// The calling process's own pane. Absent ⇒ the focused pane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_pane_id: Option<String>,
}

/// `PaneProcessInfoParams` (`tmp/herdr/src/api/schema/panes.rs:141-146`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PaneProcessInfoParams {
    /// The pane to inspect. Absent ⇒ the focused pane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

/// `PaneSendInputParams` (`tmp/herdr/src/api/schema/panes.rs:345-354`).
///
/// **This is `herdr pane run`.** The CLI verb is not a separate method: it sends
/// `pane.send_input { text, keys: ["Enter"] }` (`tmp/herdr/src/cli/pane.rs:1039-1052`). `text` and
/// `keys` are both optional on the wire (`"required": ["pane_id"]`) and are encoded in that order
/// by `encode_api_input` (`tmp/herdr/src/app/api/panes.rs:1846-1853`), so text-then-Enter is one
/// call rather than two. An unknown key name is `invalid_key` and **nothing is written** —
/// herdr encodes before it sends (`panes.rs:1852`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneSendInputParams {
    /// The pane to write to.
    pub pane_id: String,
    /// Literal text, written first.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Key names, written after `text`, e.g. `["Enter"]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
}

impl PaneSendInputParams {
    /// `herdr pane run`'s own shape: the text, then `Enter`
    /// (`tmp/herdr/src/cli/pane.rs:1047-1051`).
    pub fn run(pane_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            pane_id: pane_id.into(),
            text: text.into(),
            keys: vec!["Enter".to_owned()],
        }
    }

    /// Text with no trailing key.
    pub fn text(pane_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            pane_id: pane_id.into(),
            text: text.into(),
            keys: Vec::new(),
        }
    }
}

/// `PaneReadParams` (`tmp/herdr/src/api/schema/panes.rs:355-369`).
///
/// herdr's own struct has a sixth field, `intent: ReadIntent`, carrying `#[serde(skip)]` and
/// `#[schemars(skip)]` (`:66-68`): it is how herdr's in-process `pane.wait_for_output` loop marks
/// its own reads passive, and it never appears on the wire. There is nothing for a socket client
/// to mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneReadParams {
    /// The pane to read.
    pub pane_id: String,
    /// Which region of the pane.
    pub source: ReadSource,
    /// A line cap. Absent ⇒ herdr's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// Text or ANSI.
    pub format: ReadFormat,
    /// Strip ANSI escapes. herdr defaults this to `true` (`#[serde(default = "default_true")]`).
    pub strip_ansi: bool,
}

impl PaneReadParams {
    /// A read of `source` with herdr's own defaults: no line cap, text, ANSI stripped.
    pub fn new(pane_id: impl Into<String>, source: ReadSource) -> Self {
        Self {
            pane_id: pane_id.into(),
            source,
            lines: None,
            format: ReadFormat::Text,
            strip_ansi: default_true(),
        }
    }
}

/// `PaneReportAgentParams` (`tmp/herdr/src/api/schema/panes.rs:447-462`).
///
/// The status bridge's main verb. Required: `pane_id`, `source`, `agent`, `state`.
///
/// `source` is the authority name — herdr keys its per-source agent authority on it and
/// `pane.clear_agent_authority` releases it. `agent` is normalised by herdr
/// (`normalize_reported_agent_label`, `tmp/herdr/src/app/api/panes.rs:1556`) and an unusable label
/// is `invalid_agent`. `seq` is the monotonic ordering token that lets herdr drop a report that
/// arrives after a newer one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneReportAgentParams {
    /// The pane being reported on.
    pub pane_id: String,
    /// The reporting authority's name.
    pub source: String,
    /// The agent label, e.g. `"cyrup"`.
    pub agent: String,
    /// The reported state — four values, no `Done`. See [`PaneAgentState`].
    pub state: PaneAgentState,
    /// A one-line human message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Monotonic ordering token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The agent session this report belongs to, by id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    /// The agent session this report belongs to, by path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session_path: Option<String>,
}

impl PaneReportAgentParams {
    /// The four required fields.
    pub fn new(
        pane_id: impl Into<String>,
        source: impl Into<String>,
        agent: impl Into<String>,
        state: PaneAgentState,
    ) -> Self {
        Self {
            pane_id: pane_id.into(),
            source: source.into(),
            agent: agent.into(),
            state,
            message: None,
            seq: None,
            agent_session_id: None,
            agent_session_path: None,
        }
    }
}

/// `PaneReportAgentSessionParams` (`tmp/herdr/src/api/schema/panes.rs:464-477`).
///
/// Binds a pane to a resumable agent session without also asserting a state. herdr turns
/// `agent_session_id`/`agent_session_path` into an `AgentSessionRef`
/// (`session_ref_from_report`, `tmp/herdr/src/app/api/panes.rs:1586-1591`), which is what later
/// surfaces as [`super::agents::AgentSessionInfo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneReportAgentSessionParams {
    /// The pane being reported on.
    pub pane_id: String,
    /// The reporting authority's name.
    pub source: String,
    /// The agent label.
    pub agent: String,
    /// Monotonic ordering token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The session's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    /// The session's on-disk path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session_path: Option<String>,
    /// How the session was started, normalised by herdr
    /// (`normalize_session_start_source`, `tmp/herdr/src/app/api/panes.rs:1598-1600`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_start_source: Option<String>,
}

impl PaneReportAgentSessionParams {
    /// The three required fields.
    pub fn new(
        pane_id: impl Into<String>,
        source: impl Into<String>,
        agent: impl Into<String>,
    ) -> Self {
        Self {
            pane_id: pane_id.into(),
            source: source.into(),
            agent: agent.into(),
            seq: None,
            agent_session_id: None,
            agent_session_path: None,
            session_start_source: None,
        }
    }
}

/// `PaneReportMetadataParams` (`tmp/herdr/src/api/schema/panes.rs:479-507`).
///
/// The label/token verb — **the only one pi's status bridge ever sends**
/// (`src/integrations/herdr-status.ts:206,221` @v0.68.0, both shapes
/// `["pane","report-metadata",…]`).
///
/// Three things herdr validates and a client must not guess at:
/// - `tokens` is a **patch**: `Some(value)` sets, `None` clears. At most 16 keys, each matching
///   `^[A-Za-z0-9_-]{1,32}$` (`metadata_token_patch_schema`,
///   `tmp/herdr/src/api/schema/common.rs:3-12`); a violation is `invalid_metadata_token`.
/// - `ttl_ms` is `1..=86_400_000` (`#[schemars(range(…))]`, `:133`); out of range is
///   `invalid_metadata_ttl`.
/// - the three `clear_*` flags are how a field is removed, because `None` on `title` means
///   "leave it alone", not "clear it".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneReportMetadataParams {
    /// The pane being labelled.
    pub pane_id: String,
    /// The reporting authority's name.
    pub source: String,
    /// The agent label this metadata belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Apply to another source's record rather than this one's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applies_to_source: Option<String>,
    /// The pane's displayed title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The agent name herdr should display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_agent: Option<String>,
    /// Per-state label overrides.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub state_labels: BTreeMap<String, String>,
    /// The token patch: `Some` sets, `None` clears.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tokens: BTreeMap<String, Option<String>>,
    /// Clear `title`.
    pub clear_title: bool,
    /// Clear `display_agent`.
    pub clear_display_agent: bool,
    /// Clear `state_labels`.
    pub clear_state_labels: bool,
    /// Monotonic ordering token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// How long this metadata survives, in milliseconds, `1..=86_400_000`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

impl PaneReportMetadataParams {
    /// The two required fields; everything else left alone.
    pub fn new(pane_id: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            pane_id: pane_id.into(),
            source: source.into(),
            agent: None,
            applies_to_source: None,
            title: None,
            display_agent: None,
            state_labels: BTreeMap::new(),
            tokens: BTreeMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            seq: None,
            ttl_ms: None,
        }
    }
}

/// `PaneClearAgentAuthorityParams` (`tmp/herdr/src/api/schema/panes.rs:509-516`).
///
/// Drops the reporting authority a `source` holds over a pane, leaving the pane's own screen
/// detection in charge again (`AppEvent::HookAuthorityCleared`,
/// `tmp/herdr/src/app/api/panes.rs:1786-1790`). `source` is optional — absent clears whatever
/// authority the pane currently has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneClearAgentAuthorityParams {
    /// The pane.
    pub pane_id: String,
    /// The authority to drop. Absent ⇒ whichever is current.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Monotonic ordering token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

impl PaneClearAgentAuthorityParams {
    /// Clear whichever authority the pane holds.
    pub fn new(pane_id: impl Into<String>) -> Self {
        Self {
            pane_id: pane_id.into(),
            source: None,
            seq: None,
        }
    }
}

/// `PaneReleaseAgentParams` (`tmp/herdr/src/api/schema/panes.rs:518-525`).
///
/// The shutdown counterpart of [`PaneReportAgentParams`]: this named agent is no longer in this
/// pane (`AppEvent::HookAgentReleased`, `tmp/herdr/src/app/api/panes.rs:1804-1810`). Unlike
/// [`PaneClearAgentAuthorityParams`], `source` **and** `agent` are both required, so a release
/// cannot silently retire another reporter's agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneReleaseAgentParams {
    /// The pane.
    pub pane_id: String,
    /// The reporting authority's name.
    pub source: String,
    /// The agent label being released.
    pub agent: String,
    /// Monotonic ordering token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

impl PaneReleaseAgentParams {
    /// The three required fields.
    pub fn new(
        pane_id: impl Into<String>,
        source: impl Into<String>,
        agent: impl Into<String>,
    ) -> Self {
        Self {
            pane_id: pane_id.into(),
            source: source.into(),
            agent: agent.into(),
            seq: None,
        }
    }
}

/// `PaneInfo` (`tmp/herdr/src/api/schema/panes.rs:527-561`) — herdr's pane record.
///
/// Answered by `pane.get`, `pane.focus` and **`pane.split`** (all three encode
/// `ResponseResult::PaneInfo`, `tmp/herdr/src/app/api/panes.rs:132,159-168,484-500`), and carried
/// in bulk by `pane.list`, `pane.current` and `session.snapshot`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneInfo {
    /// The public pane id, e.g. `"w1:p1"` — the value herdr injects as `HERDR_PANE_ID`.
    pub pane_id: String,
    /// The terminal backing this pane. This, not `pane_id`, is what
    /// [`super::agents::AgentInfo`] is keyed on.
    pub terminal_id: String,
    /// The owning workspace.
    pub workspace_id: String,
    /// The owning tab.
    pub tab_id: String,
    /// Whether this pane has focus.
    pub focused: bool,
    /// The pane's launch directory.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The foreground process's directory, when herdr can see it.
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    /// The user's own label.
    #[serde(default)]
    pub label: Option<String>,
    /// The agent herdr believes occupies this pane.
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
    /// The agent name to display, from `pane.report_metadata`.
    #[serde(default)]
    pub display_agent: Option<String>,
    /// The observed status — five values, `Done` included. See [`AgentStatus`].
    pub agent_status: AgentStatus,
    /// Per-state label overrides.
    #[serde(default)]
    pub state_labels: BTreeMap<String, String>,
    /// Reported tokens, already resolved: `String`, not `Option<String>`, because the patch
    /// semantics of [`PaneReportMetadataParams::tokens`] belong to the request only.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    /// The resumable agent session bound to this pane.
    #[serde(default)]
    pub agent_session: Option<AgentSessionInfo>,
    /// Where the pane's viewport sits in its scrollback.
    #[serde(default)]
    pub scroll: Option<PaneScrollInfo>,
    /// Bumps on every change; the ordering token for a cached copy of a pane record.
    ///
    /// It is one of TWO live counters in this crate, not the only one:
    /// [`super::agents::AgentInfo::revision`] is the SAME value, copied straight off the pane
    /// record herdr assembles the agent record from (`revision: pane.revision`,
    /// `tmp/herdr/src/app/agents.rs:400`, where `pane` is that pane's own `pane_info`). Either may
    /// be keyed on; they cannot disagree.
    ///
    /// herdr fills it from the terminal's own counter (`revision: terminal.revision`,
    /// `tmp/herdr/src/app/creation.rs:354`), initialised to `0`
    /// (`tmp/herdr/src/terminal/state.rs:180`) and advanced at three production sites:
    /// `tmp/herdr/src/terminal/state.rs:235` (`wrapping_add(1)` on a stripped-title change),
    /// `tmp/herdr/src/app/actions.rs:428` and `tmp/herdr/src/app/api/panes.rs:1749` (both
    /// `saturating_add(1)`, on metadata-token expiry and patch). So a cache over `pane.get`,
    /// `pane.list`, `pane.current` or [`super::session::SessionSnapshot`] keys on **this** field.
    ///
    /// Not to be confused with [`PaneReadResult::revision`], which is a hard-coded `0` and is
    /// useless as a key — that type's doc says which payloads carry the dead one.
    pub revision: u64,
}

/// `PaneScrollInfo` (`tmp/herdr/src/api/schema/panes.rs:563-568`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct PaneScrollInfo {
    /// Rows scrolled back from the live edge; `0` is pinned to the bottom.
    pub offset_from_bottom: u64,
    /// The largest `offset_from_bottom` this pane's scrollback allows.
    pub max_offset_from_bottom: u64,
    /// The viewport's height in rows.
    pub viewport_rows: u64,
}

/// `PaneProcessInfo` (`tmp/herdr/src/api/schema/panes.rs:570-581`) — the answer to
/// `pane.process_info`.
///
/// Every field but `pane_id` is best-effort: herdr fills them from `detect::foreground_job`
/// (`tmp/herdr/src/app/api/panes.rs:536-538`), which is a platform probe that can legitimately
/// see nothing. An empty `foreground_processes` is *"herdr could not tell"*, not *"the pane is
/// idle"*.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneProcessInfo {
    /// The pane.
    pub pane_id: String,
    /// The pane's shell process.
    #[serde(default)]
    pub shell_pid: Option<u32>,
    /// The foreground process group on the pane's tty.
    #[serde(default)]
    pub foreground_process_group_id: Option<u32>,
    /// The pane's tty device.
    #[serde(default)]
    pub tty: Option<String>,
    /// The processes in the foreground group.
    #[serde(default)]
    pub foreground_processes: Vec<PaneProcessInfoProcess>,
}

/// `PaneProcessInfoProcess` (`tmp/herdr/src/api/schema/panes.rs:583-595`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneProcessInfoProcess {
    /// The process id.
    pub pid: u32,
    /// The process name as the platform reports it.
    pub name: String,
    /// `argv[0]`, when readable.
    #[serde(default)]
    pub argv0: Option<String>,
    /// The full argument vector, when readable.
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    /// The joined command line, when readable.
    #[serde(default)]
    pub cmdline: Option<String>,
    /// The process's working directory, when readable.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// `PaneLayoutSnapshot` (`tmp/herdr/src/api/schema/panes.rs:669-678`).
///
/// Part of [`super::session::SessionSnapshot`]'s record closure — one entry per tab, describing
/// where each pane sits on the terminal grid.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneLayoutSnapshot {
    /// The owning workspace.
    pub workspace_id: String,
    /// The tab this layout describes.
    pub tab_id: String,
    /// Whether one pane is zoomed over the others.
    pub zoomed: bool,
    /// The tab's whole area in cells.
    pub area: PaneLayoutRect,
    /// The focused pane in this tab.
    pub focused_pane_id: String,
    /// Each pane's rectangle.
    pub panes: Vec<PaneLayoutPane>,
    /// Each divider.
    pub splits: Vec<PaneLayoutSplit>,
}

/// `PaneLayoutRect` (`tmp/herdr/src/api/schema/panes.rs:680-686`) — terminal **cells**, not pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct PaneLayoutRect {
    /// Column of the left edge.
    pub x: u16,
    /// Row of the top edge.
    pub y: u16,
    /// Width in cells.
    pub width: u16,
    /// Height in cells.
    pub height: u16,
}

/// `PaneLayoutPane` (`tmp/herdr/src/api/schema/panes.rs:688-693`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneLayoutPane {
    /// The pane.
    pub pane_id: String,
    /// Whether it has focus.
    pub focused: bool,
    /// Its rectangle.
    pub rect: PaneLayoutRect,
}

/// `PaneLayoutSplit` (`tmp/herdr/src/api/schema/panes.rs:695-701`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneLayoutSplit {
    /// The divider's id.
    pub id: String,
    /// Which way it divides.
    pub direction: SplitDirection,
    /// The first child's share.
    pub ratio: f32,
    /// The divided area.
    pub rect: PaneLayoutRect,
}

/// `PaneReadResult` (`tmp/herdr/src/api/schema/panes.rs:755-765`) — the answer to `pane.read`,
/// and the payload inside an `output_matched` (`tmp/herdr/src/api/wait.rs:100-107`).
///
/// **This type's `revision` is always `0`** at this pin — on every payload that carries a
/// `PaneReadResult`, not only on the direct answer to `pane.read`. herdr hard-codes the literal in
/// the single `pane.read` dispatch (`tmp/herdr/src/app/api/panes.rs:1540`) and every other
/// producer routes through it: `pane.wait_for_output` copies the field it just read
/// (`let revision = read.revision;`, `tmp/herdr/src/api/wait.rs:98`), the `pane.output_matched`
/// subscription builds its event from the same helper
/// (`tmp/herdr/src/api/subscriptions.rs:295-320` → `:493-515`), and the headless server's
/// frozen-alt-screen override touches only `text` and `truncated`
/// (`tmp/herdr/src/server/headless.rs:3141-3147`).
///
/// So do not build a cache key, or an `EventMatch` floor, on any of the **three dead ones** —
/// [`Self::revision`], [`super::response::OutputMatched::revision`] and
/// [`super::events::PaneOutputMatchedEvent::read`]'s: the floor would be `0` and every stale event
/// would pass it.
///
/// **That `0` is a fact about the read, not about the protocol.** herdr does publish a real
/// ordering token, on the pane and agent records: [`PaneInfo::revision`], the terminal's own
/// counter (`tmp/herdr/src/app/creation.rs:354`, bumped at `tmp/herdr/src/terminal/state.rs:235`,
/// `tmp/herdr/src/app/actions.rs:428` and `tmp/herdr/src/app/api/panes.rs:1749`), and
/// [`super::agents::AgentInfo::revision`], which is that same value copied across
/// (`tmp/herdr/src/app/agents.rs:400`). A fleet cache keys on either.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneReadResult {
    /// The pane that was read.
    pub pane_id: String,
    /// Its workspace.
    pub workspace_id: String,
    /// Its tab.
    pub tab_id: String,
    /// The region that was read.
    pub source: ReadSource,
    /// The format the text is in.
    pub format: ReadFormat,
    /// The text.
    pub text: String,
    /// See the type doc: `0` from every producer at this pin.
    pub revision: u64,
    /// Whether herdr cut the text at its own ceiling.
    pub truncated: bool,
}
