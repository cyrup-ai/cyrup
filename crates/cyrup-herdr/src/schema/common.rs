//! Mirrors `tmp/herdr/src/api/schema/common.rs`.

use serde::{Deserialize, Serialize};

/// `EmptyParams` (`tmp/herdr/src/api/schema/common.rs:25-26`) — serialises to `{}`.
///
/// It exists because `params` is **mandatory on every method**: `Method` is adjacently tagged
/// (`#[serde(tag = "method", content = "params")]`, `tmp/herdr/src/api/schema.rs:41`) with a
/// newtype variant per method, so `{"id":"x","method":"server.stop"}` with no `params` key is
/// rejected. The published schema pins it — every request variant's `"required"` is
/// `["method","params"]`.
///
/// Serialize-only: this client sends requests and never parses one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EmptyParams {}

/// `PaneTarget` (`tmp/herdr/src/api/schema/common.rs:33-36`) — the params of `pane.get`,
/// `pane.focus` and `pane.close` (`tmp/herdr/src/api/schema.rs:169,171,183,185,207,234`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneTarget {
    /// The public pane id, e.g. `"w1:p1"`.
    pub pane_id: String,
}

impl PaneTarget {
    /// A target naming `pane_id`.
    pub fn new(pane_id: impl Into<String>) -> Self {
        Self {
            pane_id: pane_id.into(),
        }
    }
}

/// `TabTarget` (`tmp/herdr/src/api/schema/common.rs:49-52`) — the params of `tab.get`
/// (`tmp/herdr/src/api/schema.rs:106-107`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TabTarget {
    /// The public tab id, e.g. `"w1:t1"`.
    pub tab_id: String,
}

impl TabTarget {
    /// A target naming `tab_id`.
    pub fn new(tab_id: impl Into<String>) -> Self {
        Self {
            tab_id: tab_id.into(),
        }
    }
}

/// `AgentTarget` (`tmp/herdr/src/api/schema/common.rs:54-57`) — the params of `agent.get`
/// (`tmp/herdr/src/api/schema.rs:118-119`).
///
/// The field is `target`, **not** `agent_id`: herdr resolves a terminal id, a pane id, or a name,
/// and answers `agent_target_ambiguous` when more than one matches
/// (`tmp/herdr/src/app/agents.rs:293-323`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentTarget {
    /// The agent target: a terminal id, a pane id, or an agent name.
    pub target: String,
}

impl AgentTarget {
    /// A target naming `target`.
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
        }
    }
}

/// `SplitDirection` (`tmp/herdr/src/api/schema/common.rs:70-75`), `rename_all = "snake_case"`.
///
/// herdr maps `Right` onto `Direction::Horizontal` and `Down` onto `Direction::Vertical`
/// (`tmp/herdr/src/app/api/panes.rs:68-71`) — the wire word names where the *new* pane lands, not
/// the axis of the divider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    /// The new pane opens to the right of the target.
    Right,
    /// The new pane opens below the target.
    Down,
}

/// `ReadSource` (`tmp/herdr/src/api/schema/common.rs:77-84`), `rename_all = "snake_case"`.
///
/// **Closed, and safely so** — unlike [`AgentStatus`], which had to be opened. A `ReadSource`
/// only ever comes *back* as the echo of one this client sent: herdr's `pane.read` handler writes
/// `source: params.source` straight out of the request
/// (`tmp/herdr/src/app/api/panes.rs:1533-1543`), and the one place herdr substitutes a different
/// value — `output_match_read_source`, `tmp/herdr/src/api/subscriptions.rs:11-18` — maps
/// `Recent` to `RecentUnwrapped` and is the identity on everything else. A sixth variant in a
/// newer herdr therefore cannot reach [`super::panes::PaneReadResult::source`] unless this client
/// asks for it, and it cannot ask for a variant it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadSource {
    /// The pane's current viewport.
    Visible,
    /// Recent scrollback, wrapped as displayed.
    Recent,
    /// Recent scrollback with display wrapping undone.
    RecentUnwrapped,
    /// The region herdr's own agent-state detection reads.
    Detection,
}

/// `ReadFormat` (`tmp/herdr/src/api/schema/common.rs:93-101`), `rename_all = "snake_case"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadFormat {
    /// Plain text. herdr's own default (`#[default]`, `common.rs:98`).
    #[default]
    Text,
    /// Text with ANSI escapes retained.
    Ansi,
}

/// `PaneAgentState` (`tmp/herdr/src/api/schema/common.rs:149-156`), `rename_all = "snake_case"`.
///
/// **Four states, not five.** This is what `pane.report_agent` carries, and it has no `Done`:
/// herdr derives a pane's *displayed* [`AgentStatus`] — which does have `Done` — from the reported
/// state plus its own screen detection. Sending `"done"` here would be `invalid_request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneAgentState {
    /// The agent is waiting for input.
    Idle,
    /// The agent is running.
    Working,
    /// The agent needs the user.
    Blocked,
    /// The reporter cannot tell.
    Unknown,
}

/// `AgentStatus` (`tmp/herdr/src/api/schema/common.rs:158-166`), `rename_all = "snake_case"`.
///
/// **Five states.** This is the *observed* status herdr publishes on [`super::panes::PaneInfo`],
/// [`super::tabs::TabInfo`], [`super::agents::AgentInfo`] and
/// [`super::workspaces::WorkspaceInfo`], and it carries `Done`, which
/// [`PaneAgentState`] does not.
///
/// **Open, like [`super::response::ResponseResult`] and for the same reason.** This field is
/// REQUIRED and un-defaulted on [`super::panes::PaneInfo`], [`super::tabs::TabInfo`],
/// [`super::agents::AgentInfo`] and [`super::workspaces::WorkspaceInfo`], and
/// [`super::session::SessionSnapshot`] carries a `Vec<PaneInfo>`. A closed enum here would mean
/// that **one pane in a status a newer herdr invented — a pane cyrup does not own and does not
/// care about — fails the whole `session.snapshot` line**, taking `pane_get`, `pane_list`,
/// `pane_current`, `pane_split`, `pane_focus`, `agent_get`, `agent_list`, `tab_get`, `tab_rename`
/// and the `pane_created`/`pane_updated` events with it. It would not degrade, either:
/// [`crate::HerdrClient::bootstrap`] fails, [`crate::ReconnectingEvents`] reads that as a restart,
/// and `rebootstrap` burns its whole attempt budget re-fetching the same undecodable snapshot
/// before ending the stream for ever. One enum value would take the fleet view dark for the rest
/// of the process's life. So [`Self::Unrecognised`] exists, and it round-trips the wire spelling.
///
/// The justification that used to stand here for closing it was false and has been deleted:
/// "`#[serde(other)]` is only available on an internally or adjacently tagged enum, so a bare
/// string enum has no forward-compatible arm to offer". The first clause is true **about
/// `#[serde(other)]`**; the conclusion does not follow. Variant-level `#[serde(untagged)]` has
/// given exactly this catch-all since serde 1.0.181, and this workspace is on 1.0.228
/// (`Cargo.lock:6963-6964`); a hand-written `Deserialize` over `String` would have given it in
/// any version.
///
/// What remains true, and is why [`Self::Unrecognised`] is *loud* rather than folded into
/// [`Self::Unknown`]: herdr's own type is closed, the published artifact pins exactly the five
/// named values (`success_response.$defs.AgentStatus.enum`,
/// `docs/next/api/herdr-api.schema.json`) and its CI fails on drift
/// (`src/api/schema/tests.rs:182-207`). A sixth status is therefore a real protocol change, and
/// the fix is still the documented one — bump the pin and re-read that file. Reporting it as
/// [`Self::Unknown`] would be a fabricated reading ("herdr cannot tell" is a status herdr
/// publishes); reporting it as [`Self::Unrecognised`] carrying the raw spelling is not.
///
/// `Serialize` as well as `Deserialize`, unlike [`PaneAgentState`]: herdr accepts an `AgentStatus`
/// on the way *in* too, as the optional narrowing on
/// [`super::events::Subscription::PaneAgentStatusChanged`] (`tmp/herdr/src/api/schema/events.rs:75-79`).
/// An [`Self::Unrecognised`] sent that way serialises as the string it carries, so it is herdr's
/// `invalid_request` to refuse — never a silently different request.
///
/// Not `Copy`, because [`Self::Unrecognised`] owns its spelling. That is the price of not going
/// dark, and nothing in this crate needed the `Copy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Waiting for input.
    Idle,
    /// Running.
    Working,
    /// Needs the user.
    Blocked,
    /// Finished its turn.
    Done,
    /// herdr cannot tell.
    Unknown,
    /// A status this build does not name, carrying herdr's own spelling.
    ///
    /// Never one of the five above — serde tries those first and only falls through to this
    /// untagged arm when none matched.
    #[serde(untagged)]
    Unrecognised(String),
}

/// herdr's `default_true` (`tmp/herdr/src/api/schema/common.rs:168-170`), used by
/// `#[serde(default = …)]` on every `strip_ansi` field.
pub(crate) const fn default_true() -> bool {
    true
}
