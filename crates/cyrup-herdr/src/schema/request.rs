//! Mirrors the top level of `tmp/herdr/src/api/schema.rs` — the request envelope and the method
//! enum. (herdr keeps these in the parent file rather than a `schema/` submodule.)

use serde::Serialize;

use super::agents::{
    AgentPromptParams, AgentStartParams, AgentViewClearParams, AgentViewSetParams,
};
use super::common::{AgentTarget, EmptyParams, PaneTarget, TabTarget};
use super::events::{EventsSubscribeParams, PaneWaitForOutputParams};
use super::panes::{
    PaneClearAgentAuthorityParams, PaneCurrentParams, PaneListParams, PaneProcessInfoParams,
    PaneReadParams, PaneReleaseAgentParams, PaneReportAgentParams, PaneReportAgentSessionParams,
    PaneReportMetadataParams, PaneSendInputParams, PaneSplitParams,
};
use super::server::PingParams;
use super::tabs::{TabCreateParams, TabRenameParams};
use super::workspaces::WorkspaceCreateParams;

/// `Request` (`tmp/herdr/src/api/schema.rs:35-39`).
///
/// On the wire: `{"id":"req_1","method":"ping","params":{}}`. The `#[serde(flatten)]` is what
/// hoists `Method`'s adjacent `method`/`params` tagging onto the same object as `id`.
///
/// Serialize-only. This client sends requests; it never parses one, so there is no `Deserialize`
/// here to keep honest.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Request {
    /// The correlation id. herdr echoes it on the response, and a response carrying a different one
    /// is refused ([`crate::HerdrError::IdMismatch`]).
    pub id: String,
    /// The method and its params.
    #[serde(flatten)]
    pub method: Method,
}

/// `Method` (`tmp/herdr/src/api/schema.rs:40-47`), adjacently tagged as
/// `#[serde(tag = "method", content = "params")]`.
///
/// herdr declares 105 renamed variants plus 4 `#[serde(skip)]` internal graphics-frame ones (109
/// total, all covered by `api_method_name`, `tmp/herdr/src/api/server.rs:398-510`); the published
/// JSON Schema lists 104 because `pane.graphics.stream` is `#[schemars(skip)]`
/// (`schema.rs:209`) while still being a live wire method.
///
/// **This enum carries only the variants cyrup has a caller for**, in herdr's own declaration
/// order so a diff against a future pin stays line-for-line. Each further batch adds the ones it
/// drives, and the envelope is generic, so each is one variant. Nothing lands here without a
/// caller.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "method", content = "params")]
#[non_exhaustive]
// herdr carries the same allow on the same enum, for the same reason
// (`tmp/herdr/src/api/schema.rs:44-46`): a request enum is a short-lived wire value, and boxing a
// variant to even the sizes out would put a `Box` in every caller's construction site in exchange
// for nothing measurable.
#[allow(clippy::large_enum_variant)]
pub enum Method {
    /// `ping` (`tmp/herdr/src/api/schema.rs:48-49`), answered by the API server itself without
    /// touching the app (`tmp/herdr/src/api/server.rs:355-368`) — so it is the one method that
    /// proves the socket is live even while the UI is busy.
    #[serde(rename = "ping")]
    Ping(PingParams),
    /// `session.snapshot` (`schema.rs:74-75`) — the whole session in one reply.
    #[serde(rename = "session.snapshot")]
    SessionSnapshot(EmptyParams),
    /// `workspace.create` (`schema.rs:76-77`) — answers the new workspace, tab and root pane.
    #[serde(rename = "workspace.create")]
    WorkspaceCreate(WorkspaceCreateParams),
    /// `tab.create` (`schema.rs:102-103`) — answers the new tab and its root pane.
    #[serde(rename = "tab.create")]
    TabCreate(TabCreateParams),
    /// `tab.get` (`schema.rs:106-107`).
    #[serde(rename = "tab.get")]
    TabGet(TabTarget),
    /// `tab.rename` (`schema.rs:110-111`) — answers the renamed [`super::tabs::TabInfo`], not an
    /// acknowledgement.
    #[serde(rename = "tab.rename")]
    TabRename(TabRenameParams),
    /// `agent.list` (`schema.rs:116-117`) — the cross-pane roster.
    #[serde(rename = "agent.list")]
    AgentList(EmptyParams),
    /// `agent.get` (`schema.rs:118-119`).
    #[serde(rename = "agent.get")]
    AgentGet(AgentTarget),
    /// `agent.view.set` (`schema.rs:128-129`) — install the ONE server-wide sidebar projection.
    ///
    /// Answers [`super::response::ResponseResult::AgentView`], not an acknowledgement: herdr
    /// reports back what is now active, which is how a caller learns its set was accepted rather
    /// than replaced in the same breath.
    #[serde(rename = "agent.view.set")]
    AgentViewSet(AgentViewSetParams),
    /// `agent.view.clear` (`schema.rs:130-131`) — remove it, optionally only if this source still
    /// owns it.
    #[serde(rename = "agent.view.clear")]
    AgentViewClear(AgentViewClearParams),
    /// `agent.start` (`schema.rs:134-135`) — answers `agent_started` with the argv herdr typed.
    #[serde(rename = "agent.start")]
    AgentStart(AgentStartParams),
    /// `agent.prompt` (`schema.rs:136-137`) — answers `agent_prompted`; with a `wait` it is one of
    /// herdr's in-band waits and holds the connection until the agent settles.
    #[serde(rename = "agent.prompt")]
    AgentPrompt(AgentPromptParams),
    /// `pane.split` (`schema.rs:140-141`) — answers the **new** pane's
    /// [`super::panes::PaneInfo`] (`tmp/herdr/src/app/api/panes.rs:132`).
    #[serde(rename = "pane.split")]
    PaneSplit(PaneSplitParams),
    /// `pane.process_info` (`schema.rs:150-151`).
    #[serde(rename = "pane.process_info")]
    PaneProcessInfo(PaneProcessInfoParams),
    /// `pane.list` (`schema.rs:178-179`).
    #[serde(rename = "pane.list")]
    PaneList(PaneListParams),
    /// `pane.current` (`schema.rs:180-181`).
    #[serde(rename = "pane.current")]
    PaneCurrent(PaneCurrentParams),
    /// `pane.get` (`schema.rs:182-183`).
    #[serde(rename = "pane.get")]
    PaneGet(PaneTarget),
    /// `pane.focus` (`schema.rs:184-185`) — answers the focused pane's
    /// [`super::panes::PaneInfo`].
    #[serde(rename = "pane.focus")]
    PaneFocus(PaneTarget),
    /// `pane.send_input` (`schema.rs:198-199`) — **this is `herdr pane run`**
    /// (`tmp/herdr/src/cli/pane.rs:1039-1052`).
    #[serde(rename = "pane.send_input")]
    PaneSendInput(PaneSendInputParams),
    /// `pane.read` (`schema.rs:200-201`).
    #[serde(rename = "pane.read")]
    PaneRead(PaneReadParams),
    /// `pane.report_agent` (`schema.rs:223-224`) — the status bridge's main verb.
    #[serde(rename = "pane.report_agent")]
    PaneReportAgent(PaneReportAgentParams),
    /// `pane.report_agent_session` (`schema.rs:225-226`).
    #[serde(rename = "pane.report_agent_session")]
    PaneReportAgentSession(PaneReportAgentSessionParams),
    /// `pane.report_metadata` (`schema.rs:227-228`) — the only verb pi's own status bridge sends
    /// (`src/integrations/herdr-status.ts:206,221` @v0.68.0).
    #[serde(rename = "pane.report_metadata")]
    PaneReportMetadata(PaneReportMetadataParams),
    /// `pane.clear_agent_authority` (`schema.rs:229-230`).
    #[serde(rename = "pane.clear_agent_authority")]
    PaneClearAgentAuthority(PaneClearAgentAuthorityParams),
    /// `pane.release_agent` (`schema.rs:231-232`) — the `SessionShutdown` counterpart of
    /// `pane.report_agent`.
    #[serde(rename = "pane.release_agent")]
    PaneReleaseAgent(PaneReleaseAgentParams),
    /// `pane.close` (`schema.rs:233-234`).
    #[serde(rename = "pane.close")]
    PaneClose(PaneTarget),
    /// `pane.wait_for_output` (`schema.rs:241-242`) — one of herdr's four in-band waits: it holds
    /// the connection open, polls, and then writes exactly one line
    /// (`tmp/herdr/src/api/server.rs:251-288`, `tmp/herdr/src/api/wait.rs:22-128`).
    #[serde(rename = "pane.wait_for_output")]
    PaneWaitForOutput(PaneWaitForOutputParams),
    /// `events.subscribe` (`schema.rs:237-238`) — **the one method that keeps its connection**.
    ///
    /// herdr answers `{"type":"subscription_started"}` and then pushes events on the same
    /// connection until either side closes (`tmp/herdr/src/api/server.rs:229-250` →
    /// `:715-779`). It therefore does not go through [`crate::transport::request`], which reads
    /// exactly one line; [`crate::HerdrClient::subscribe`] and
    /// [`crate::HerdrClient::bootstrap`] own it.
    #[serde(rename = "events.subscribe")]
    EventsSubscribe(EventsSubscribeParams),
}

impl Method {
    /// The wire method name, mirroring `api_method_name`
    /// (`tmp/herdr/src/api/server.rs:398-510`).
    ///
    /// Used for the `method` field on every [`crate::HerdrError`], so a failure says which call
    /// failed without the caller threading a label through.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Ping(_) => "ping",
            Self::SessionSnapshot(_) => "session.snapshot",
            Self::WorkspaceCreate(_) => "workspace.create",
            Self::TabCreate(_) => "tab.create",
            Self::TabGet(_) => "tab.get",
            Self::TabRename(_) => "tab.rename",
            Self::AgentList(_) => "agent.list",
            Self::AgentGet(_) => "agent.get",
            Self::AgentViewSet(_) => "agent.view.set",
            Self::AgentViewClear(_) => "agent.view.clear",
            Self::AgentStart(_) => "agent.start",
            Self::AgentPrompt(_) => "agent.prompt",
            Self::PaneSplit(_) => "pane.split",
            Self::PaneProcessInfo(_) => "pane.process_info",
            Self::PaneList(_) => "pane.list",
            Self::PaneCurrent(_) => "pane.current",
            Self::PaneGet(_) => "pane.get",
            Self::PaneFocus(_) => "pane.focus",
            Self::PaneSendInput(_) => "pane.send_input",
            Self::PaneRead(_) => "pane.read",
            Self::PaneReportAgent(_) => "pane.report_agent",
            Self::PaneReportAgentSession(_) => "pane.report_agent_session",
            Self::PaneReportMetadata(_) => "pane.report_metadata",
            Self::PaneClearAgentAuthority(_) => "pane.clear_agent_authority",
            Self::PaneReleaseAgent(_) => "pane.release_agent",
            Self::PaneClose(_) => "pane.close",
            Self::PaneWaitForOutput(_) => "pane.wait_for_output",
            Self::EventsSubscribe(_) => "events.subscribe",
        }
    }
}
