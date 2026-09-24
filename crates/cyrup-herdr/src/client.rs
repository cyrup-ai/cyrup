//! [`HerdrClient`] — one herdr socket, every one-shot method cyrup drives.
//!
//! ## It holds no connection, on purpose
//!
//! herdr's `handle_connection_with_stop` (`tmp/herdr/src/api/server.rs:156-317`) reads **one**
//! line, dispatches it, writes one line and returns. There is no read loop, so there is no
//! connection worth keeping: [`HerdrClient::new`] is a `PathBuf` and a [`Duration`] and nothing
//! else, exactly as herdr's own client is (`tmp/herdr/src/api/client.rs:33-44`). Constructing one
//! touches no file and opens no socket, which is what lets a consumer build it eagerly and still
//! be inert when herdr is not there.
//!
//! ## Every call is bounded, because herdr's own dispatch is not
//!
//! `dispatch_to_app` waits on a `recv()` with a `None` timeout
//! (`tmp/herdr/src/api/server.rs:911-913`). A herdr whose UI thread is wedged therefore never
//! answers and never closes, and a client without a deadline of its own hangs forever. Every
//! method here runs under [`DEFAULT_TIMEOUT`] (15 s) unless the caller names another, and the one
//! method that is *designed* to take longer — [`HerdrClient::pane_wait_for_output`] — derives its
//! deadline from the wait it asked for rather than inheriting this one.
//!
//! ## The answer is correlated or it is refused
//!
//! A success whose `id` is not the id that was sent is [`HerdrError::IdMismatch`] and the
//! connection is dropped, never a payload accepted — see [`crate::transport::request`], which
//! holds that rule for every method on this type. An **error** envelope is fatal whatever its
//! `id`, including the empty one herdr writes when it could not recover a correlation id at all
//! (`tmp/herdr/src/api/server.rs:180-201`).
//!
//! ## The one method that is not a call: `events.subscribe`
//!
//! [`HerdrClient::subscribe`] and [`HerdrClient::bootstrap`] do not go through
//! [`crate::transport::request`], because `events.subscribe` holds its connection open past the
//! first line (`server.rs:229-250` → `:715-779`). See [`crate::stream`].
//!
//! **[`HerdrClient::bootstrap`] is the supported way to pair a snapshot with a stream, and it is
//! `[CYRUP-EXCEEDS-UPSTREAM]`.** *Premise, grepped twice:* herdr documents the ordering as
//! mandatory — *"To avoid a bootstrap gap, first open `events.subscribe` on another connection and
//! wait for its acknowledgement. Buffer that stream while calling `session.snapshot`, install the
//! snapshot, then apply the buffered events in order and continue streaming."*
//! (`socket-api.mdx:118-130`) — and its source is stricter than its prose: `stream_subscriptions`
//! takes `event_hub.current_sequence()` as its floor **before** any subscription exists
//! (`tmp/herdr/src/api/server.rs:723`), so *"Lifecycle subscriptions … do not replay events
//! retained before that point"* (`socket-api.mdx:817-819`) means the events in the window are
//! never sent at all. pi's client leaves the whole sequence to its caller — `subscribe()` is a
//! free-standing method beside `call()` (`src/runs/shared/herdr-connection.ts:93-106` @v0.68.0)
//! with nothing tying them together — so every pi consumer can silently lose the events that fire
//! while its snapshot is being built.
//!
//! `bootstrap` returns the pair from one call, so there is no order to get wrong, and it does the
//! buffering itself, so a caller cannot forget to. [`HerdrClient::session_snapshot`] remains
//! available on its own terms: a point-in-time reading, with no claim about what happens after it.

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::BufReader;

use crate::env::HerdrPane;
use crate::error::{ApiError, ApiErrorCode, HerdrError, Result};
use crate::probe::{Pong, ping_for};
use crate::schema::agents::{
    AgentInfo, AgentPromptParams, AgentStartParams, AgentView, AgentViewClearParams,
    AgentViewSetParams,
};
use crate::schema::common::{AgentTarget, EmptyParams, PaneTarget, TabTarget};
use crate::schema::events::PaneWaitForOutputParams;
use crate::schema::events::{EventsSubscribeParams, Subscription};
use crate::schema::panes::{
    PaneClearAgentAuthorityParams, PaneCurrentParams, PaneInfo, PaneListParams, PaneProcessInfo,
    PaneProcessInfoParams, PaneReadParams, PaneReadResult, PaneReleaseAgentParams,
    PaneReportAgentParams, PaneReportAgentSessionParams, PaneReportMetadataParams,
    PaneSendInputParams, PaneSplitParams,
};
use crate::schema::response::{OutputMatched, WireResponse};
use crate::schema::session::SessionSnapshot;
use crate::schema::tabs::{TabCreateParams, TabInfo, TabRenameParams};
use crate::schema::workspaces::{WorkspaceCreateParams, WorkspaceInfo};
use crate::schema::{Method, PingParams, Request, ResponseResult};
use crate::stream::{self, HerdrEvents};
use crate::transport::{self, DEFAULT_TIMEOUT, LocalStream};

/// The slack added to a [`PaneWaitForOutputParams::timeout_ms`] to get this client's own deadline.
///
/// herdr's wait loop only notices its deadline between polls, on the
/// `CONNECTION_POLL_INTERVAL` = 100 ms tick (`tmp/herdr/src/api/server.rs:28`, consumed at
/// `tmp/herdr/src/api/wait.rs:126`), and each poll performs a real `pane.read` bounded by its own
/// 5 s `APP_RESPONSE_TIMEOUT` (`server.rs:29`). So a herdr that means to time out at `timeout_ms`
/// can legitimately answer a little after it, and a client deadline set exactly at `timeout_ms`
/// would report [`crate::HerdrError::Timeout`] for a wait that in fact completed. 5 s is that
/// worst-case poll, not a guess.
pub const WAIT_GRACE: Duration = Duration::from_secs(5);

/// A herdr API socket, and the typed calls cyrup makes on it.
///
/// Cheap: it holds a path and a deadline. See the module doc for why there is no connection in
/// here, and for what is deliberately absent.
#[derive(Debug, Clone)]
pub struct HerdrClient {
    socket: PathBuf,
    timeout: Duration,
}

impl HerdrClient {
    /// A client for the herdr API socket at `socket`.
    ///
    /// Nothing is opened, nothing is checked, and `socket` need not exist yet — a herdr that is
    /// not running is reported on the first call as [`crate::Unavailable::NoSocket`] carrying the
    /// path, which is the actionable half.
    #[must_use]
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// A client for the socket of the herdr pane this process runs in.
    ///
    /// Obtain the [`HerdrPane`] with [`HerdrPane::discover`] (ambient: `None` outside a pane, and
    /// then nothing runs) or [`HerdrPane::require`] (asked-for: an error naming the reason).
    #[must_use]
    pub fn for_pane(pane: &HerdrPane) -> Self {
        Self::new(pane.socket_path().to_path_buf())
    }

    /// Replace the default per-call deadline.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The socket this client talks to.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The default per-call deadline.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Send `method` and return its raw result, under the default deadline.
    ///
    /// The escape hatch for a method with no typed wrapper below. The typed wrappers are the
    /// supported surface: they name the `result.type` they expect, so a wrong answer is
    /// [`crate::HerdrError::UnexpectedResult`] rather than a caller's own `match` with a
    /// plausible-looking fallthrough.
    ///
    /// # Errors
    /// Every arm of [`crate::HerdrError`]. See [`crate::transport::request`].
    pub async fn call(&self, method: Method) -> Result<ResponseResult> {
        self.call_for(method, self.timeout).await
    }

    /// [`Self::call`] with an explicit deadline.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub async fn call_for(&self, method: Method, timeout: Duration) -> Result<ResponseResult> {
        let request = Request {
            id: request_id(method.name()),
            method,
        };
        transport::request(&self.socket, &request, timeout).await
    }

    /// `ping` — liveness, version, protocol and capabilities.
    ///
    /// Answered by the API server itself without touching the app
    /// (`tmp/herdr/src/api/server.rs:355-368`), so it stays answerable while the UI is busy.
    ///
    /// # Errors
    /// As [`crate::probe::ping`].
    pub async fn ping(&self) -> Result<Pong> {
        ping_for(&self.socket, self.timeout).await
    }

    // ---------------------------------------------------------------------------------------
    // The write half — what the status bridge sends.
    // ---------------------------------------------------------------------------------------

    /// `pane.report_agent` — this pane's agent is now in this state.
    ///
    /// `Ok(())` means herdr acknowledged with `{"type":"ok"}`. Any other success `type`, including
    /// one newer than this client, is [`crate::HerdrError::UnexpectedResult`] — a write that came
    /// back as something unreadable is not a write that landed.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::PaneNotFound`] for an unknown pane,
    /// [`crate::ApiErrorCode::InvalidAgent`] for a label herdr cannot normalise
    /// (`tmp/herdr/src/app/api/panes.rs:1556`), plus the transport arms.
    pub async fn report_agent(&self, params: PaneReportAgentParams) -> Result<()> {
        self.call(Method::PaneReportAgent(params))
            .await?
            .ok("pane.report_agent")
    }

    /// `pane.report_agent_session` — bind this pane to a resumable agent session.
    ///
    /// # Errors
    /// As [`Self::report_agent`].
    pub async fn report_agent_session(&self, params: PaneReportAgentSessionParams) -> Result<()> {
        self.call(Method::PaneReportAgentSession(params))
            .await?
            .ok("pane.report_agent_session")
    }

    /// `pane.report_metadata` — title, displayed agent name, per-state labels and the token patch.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::InvalidMetadataToken`] for a key outside
    /// `^[A-Za-z0-9_-]{1,32}$` or more than 16 of them,
    /// [`crate::ApiErrorCode::InvalidMetadataTtl`] for a `ttl_ms` outside `1..=86_400_000`,
    /// [`crate::ApiErrorCode::InvalidMetadataSource`] for a source herdr refuses, plus the
    /// transport arms.
    pub async fn report_metadata(&self, params: PaneReportMetadataParams) -> Result<()> {
        self.call(Method::PaneReportMetadata(params))
            .await?
            .ok("pane.report_metadata")
    }

    /// `pane.release_agent` — this named agent has left this pane.
    ///
    /// The `SessionShutdown` counterpart of [`Self::report_agent`]. Both `source` and `agent` are
    /// required, so one reporter cannot retire another's agent.
    ///
    /// # Errors
    /// As [`Self::report_agent`].
    pub async fn release_agent(&self, params: PaneReleaseAgentParams) -> Result<()> {
        self.call(Method::PaneReleaseAgent(params))
            .await?
            .ok("pane.release_agent")
    }

    /// `pane.clear_agent_authority` — give the pane back to herdr's own screen detection.
    ///
    /// # Errors
    /// As [`Self::report_agent`], minus `invalid_agent`: no agent label is sent.
    pub async fn clear_agent_authority(&self, params: PaneClearAgentAuthorityParams) -> Result<()> {
        self.call(Method::PaneClearAgentAuthority(params))
            .await?
            .ok("pane.clear_agent_authority")
    }

    /// `tab.get` — read a tab's record, including its current label.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::TabNotFound`] for an unknown tab, plus the transport arms.
    pub async fn tab_get(&self, tab_id: impl Into<String>) -> Result<TabInfo> {
        self.call(Method::TabGet(TabTarget::new(tab_id)))
            .await?
            .tab("tab.get")
    }

    /// `tab.rename` — set a tab's label, and get the tab back as it now reads.
    ///
    /// herdr answers `TabInfo`, not an acknowledgement
    /// (`tmp/herdr/src/app/api/tabs.rs:169-171`), so a caller that wants to know what the label
    /// became does not need a following [`Self::tab_get`].
    ///
    /// # Errors
    /// As [`Self::tab_get`].
    pub async fn tab_rename(&self, params: TabRenameParams) -> Result<TabInfo> {
        self.call(Method::TabRename(params))
            .await?
            .tab("tab.rename")
    }

    // ---------------------------------------------------------------------------------------
    // The read half.
    // ---------------------------------------------------------------------------------------

    /// `session.snapshot` — every workspace, tab, pane, layout and agent in one reply.
    ///
    /// A point-in-time reading. It says nothing about what happens next, and pairing it with an
    /// event stream is **not** a matter of calling both: herdr requires the subscription to be
    /// opened and acknowledged on a different connection first
    /// (`socket-api.mdx:118-130`). See the module doc.
    ///
    /// This is the one genuinely large reply on the socket; [`crate::MAX_RESPONSE_BYTES`] is
    /// sized for it.
    ///
    /// # Errors
    /// [`crate::HerdrError::TooLarge`] past 4 MiB, plus the transport arms.
    pub async fn session_snapshot(&self) -> Result<SessionSnapshot> {
        self.call(Method::SessionSnapshot(EmptyParams {}))
            .await?
            .session_snapshot("session.snapshot")
    }

    /// `pane.get` — one pane's record.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::PaneNotFound`], plus the transport arms.
    pub async fn pane_get(&self, pane_id: impl Into<String>) -> Result<PaneInfo> {
        self.call(Method::PaneGet(PaneTarget::new(pane_id)))
            .await?
            .pane("pane.get")
    }

    /// `pane.list` — every pane, or every pane of one workspace.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::WorkspaceNotFound`] for an unknown `workspace_id`, plus the
    /// transport arms.
    pub async fn pane_list(&self, params: PaneListParams) -> Result<Vec<PaneInfo>> {
        self.call(Method::PaneList(params))
            .await?
            .pane_list("pane.list")
    }

    /// `pane.current` — the pane herdr considers current.
    ///
    /// Set [`PaneCurrentParams::caller_pane_id`] — to `HERDR_PANE_ID`, i.e.
    /// [`HerdrPane::pane_id`] — to mean *the pane this process is in*. Left unset, herdr answers
    /// whichever pane the **user** has focused (`tmp/herdr/src/app/api/panes.rs:145-149`), which
    /// is a different question and usually the wrong one for a background task.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::PaneNotFound`] when nothing is focused, plus the transport arms.
    pub async fn pane_current(&self, params: PaneCurrentParams) -> Result<PaneInfo> {
        self.call(Method::PaneCurrent(params))
            .await?
            .pane_current("pane.current")
    }

    /// `agent.list` — every agent herdr can see, across every pane.
    ///
    /// # Errors
    /// The transport arms.
    pub async fn agent_list(&self) -> Result<Vec<AgentInfo>> {
        self.call(Method::AgentList(EmptyParams {}))
            .await?
            .agent_list("agent.list")
    }

    /// `agent.get` — one agent, by terminal id, pane id or name.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::AgentNotFound`] when nothing matches and
    /// [`crate::ApiErrorCode::AgentTargetAmbiguous`] when more than one does — herdr's message
    /// lists every candidate (`tmp/herdr/src/app/agents.rs:302-322`) and this client keeps it
    /// verbatim. Plus the transport arms.
    pub async fn agent_get(&self, target: impl Into<String>) -> Result<AgentInfo> {
        self.call(Method::AgentGet(AgentTarget::new(target)))
            .await?
            .agent("agent.get")
    }

    /// `agent.view.set` — install the ONE server-wide projection over herdr's Agents sidebar.
    ///
    /// Answers the state AFTER the call, so a caller can see that its view is the active one.
    /// There is a single slot server-wide and a set replaces whatever was there
    /// (`socket-api.mdx:481`), which is why the counterpart
    /// [`Self::clear_agent_view`] is normally called with a `source`.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::InvalidAgentView`] for a source, label, filter or sort outside the
    /// bounds [`AgentViewSetParams`] documents, plus the transport arms. herdr's `plugin_not_found`
    /// and `plugin_disabled` arms (`tmp/herdr/src/app/api/agent_view.rs:17-28`) are reachable only
    /// for a `plugin:`-prefixed source, which this client never sends; they decode as
    /// [`crate::ApiErrorCode::Other`] if one ever does.
    pub async fn set_agent_view(&self, params: AgentViewSetParams) -> Result<AgentView> {
        self.call(Method::AgentViewSet(params))
            .await?
            .agent_view("agent.view.set")
    }

    /// `agent.view.clear` — remove the projection.
    ///
    /// With [`AgentViewClearParams::source`] set, herdr clears it **only if that source still owns
    /// it**, and a mismatch leaves the active view alone (`socket-api.mdx:494`). The answer then
    /// comes back `active: true` naming the owner that kept it, which is not an error and is the
    /// reason this returns [`AgentView`] rather than `()`.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::InvalidAgentView`] for a malformed `source`, plus the transport
    /// arms.
    pub async fn clear_agent_view(&self, params: AgentViewClearParams) -> Result<AgentView> {
        self.call(Method::AgentViewClear(params))
            .await?
            .agent_view("agent.view.clear")
    }

    /// `pane.split` — open a new pane, and get **the new pane's** record back.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::PaneNotFound`] when the target cannot be resolved,
    /// [`crate::ApiErrorCode::PaneSplitFailed`] when the UI refuses,
    /// [`crate::ApiErrorCode::InvalidEnv`] for a malformed `env` entry, plus the transport arms.
    pub async fn pane_split(&self, params: PaneSplitParams) -> Result<PaneInfo> {
        self.call(Method::PaneSplit(params))
            .await?
            .pane("pane.split")
    }

    /// `workspace.create` — open a workspace, and get it, its first tab and that tab's root pane.
    ///
    /// # Errors
    /// herdr's refusals for the params (an unusable `cwd`, say), plus the transport arms.
    pub async fn workspace_create(
        &self,
        params: WorkspaceCreateParams,
    ) -> Result<(WorkspaceInfo, TabInfo, PaneInfo)> {
        self.call(Method::WorkspaceCreate(params))
            .await?
            .workspace_created("workspace.create")
    }

    /// `tab.create` — open a tab, and get it and its root pane.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::WorkspaceNotFound`]-class refusals for an unknown workspace, plus
    /// the transport arms.
    pub async fn tab_create(&self, params: TabCreateParams) -> Result<(TabInfo, PaneInfo)> {
        self.call(Method::TabCreate(params))
            .await?
            .tab_created("tab.create")
    }

    /// `agent.start` — launch a managed agent into an empty shell pane, under `timeout`.
    ///
    /// herdr waits up to the request's own `timeout_ms` for the agent to come up before answering,
    /// so the caller's deadline must exceed it; pi-subagents asks for 45 s and waits 60 s
    /// (`src/runs/shared/herdr-placed-run.ts:173` @v0.68.0).
    ///
    /// # Errors
    /// `agent_pane_busy` ("… is not an available shell") while the pane's shell is still coming up
    /// — upstream retries that one — plus herdr's other `agent.start` refusals and the transport
    /// arms.
    pub async fn agent_start(
        &self,
        params: AgentStartParams,
        timeout: Duration,
    ) -> Result<(AgentInfo, Vec<String>)> {
        self.call_for(Method::AgentStart(params), timeout)
            .await?
            .agent_started("agent.start")
    }

    /// `agent.prompt` — submit text to a managed agent, under `timeout`.
    ///
    /// With [`AgentPromptParams::wait`] set this is an in-band wait, so `timeout` must cover the
    /// wait herdr was asked for; pi-subagents adds 5 s (`herdr-external-adapters.ts:113`).
    ///
    /// # Errors
    /// herdr's `agent.prompt` refusals, plus the transport arms.
    pub async fn agent_prompt(
        &self,
        params: AgentPromptParams,
        timeout: Duration,
    ) -> Result<AgentInfo> {
        self.call_for(Method::AgentPrompt(params), timeout)
            .await?
            .agent_prompted("agent.prompt")
    }

    /// `pane.close` — close a pane.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::ConfirmationRequired`] when the close would take a whole worktree
    /// group with it — **the pane is still open** in that case
    /// (`tmp/herdr/src/app/api/panes.rs:1880-1886`). Plus
    /// [`crate::ApiErrorCode::PaneNotFound`] and the transport arms.
    pub async fn pane_close(&self, pane_id: impl Into<String>) -> Result<()> {
        self.call(Method::PaneClose(PaneTarget::new(pane_id)))
            .await?
            .ok("pane.close")
    }

    /// `pane.focus` — focus a pane, and get its record back.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::PaneNotFound`], plus the transport arms.
    pub async fn pane_focus(&self, pane_id: impl Into<String>) -> Result<PaneInfo> {
        self.call(Method::PaneFocus(PaneTarget::new(pane_id)))
            .await?
            .pane("pane.focus")
    }

    /// `pane.send_input` — write text and/or keys into a pane.
    ///
    /// [`PaneSendInputParams::run`] is `herdr pane run`'s own shape, text then `Enter`
    /// (`tmp/herdr/src/cli/pane.rs:1047-1051`).
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::InvalidKey`] for an unknown key name — **nothing is written**,
    /// herdr encodes the whole input first (`tmp/herdr/src/app/api/panes.rs:1846-1853`) —
    /// [`crate::ApiErrorCode::PaneSendFailed`] when the pane's writer is gone, plus the transport
    /// arms.
    pub async fn pane_send_input(&self, params: PaneSendInputParams) -> Result<()> {
        self.call(Method::PaneSendInput(params))
            .await?
            .ok("pane.send_input")
    }

    /// `pane.read` — a pane's text.
    ///
    /// The result's `revision` is **always `0`**, hard-coded by herdr
    /// (`tmp/herdr/src/app/api/panes.rs:1540`) — and it is `0` on every other payload that carries
    /// a `PaneReadResult` too, including [`OutputMatched`], because every one of them is built
    /// from this same dispatch. See [`PaneReadResult`]. Do not build a cache key or an event floor
    /// on any of those three; the real ordering token herdr does publish is
    /// [`PaneInfo::revision`], which [`Self::pane_get`], [`Self::pane_list`] and
    /// [`Self::session_snapshot`] carry.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::PaneNotFound`], plus the transport arms.
    pub async fn pane_read(&self, params: PaneReadParams) -> Result<PaneReadResult> {
        self.call(Method::PaneRead(params))
            .await?
            .pane_read("pane.read")
    }

    /// `pane.wait_for_output` — block until a pane's output matches, then answer once.
    ///
    /// **The deadline is derived, not inherited.** herdr waits `timeout_ms` if one is given and
    /// **forever** if one is not (`tmp/herdr/src/api/wait.rs:30-32`, `:113`), so:
    ///
    /// - `timeout_ms` set ⇒ this client waits `timeout_ms` + [`WAIT_GRACE`], which is long enough
    ///   for herdr's own 100 ms poll and its 5 s per-poll read to land, so a wait that herdr
    ///   completes is never reported as a client timeout.
    /// - `timeout_ms` unset ⇒ this client's own deadline ([`Self::timeout`], 15 s by default) is
    ///   the only bound there is. It is applied rather than waived, because the alternative is a
    ///   hang.
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::Timeout`] when **herdr** gives up (its own `timeout_ms` elapsed, a
    /// real answer), [`crate::HerdrError::Timeout`] when this client does (no answer at all),
    /// [`crate::ApiErrorCode::InvalidRegex`] for a pattern that does not compile — refused before
    /// the first read, so it fails at once — plus [`crate::ApiErrorCode::PaneNotFound`] and the
    /// transport arms.
    pub async fn pane_wait_for_output(
        &self,
        params: PaneWaitForOutputParams,
    ) -> Result<OutputMatched> {
        let deadline = params
            .timeout_ms
            .map_or(self.timeout, |ms| Duration::from_millis(ms) + WAIT_GRACE);
        self.call_for(Method::PaneWaitForOutput(params), deadline)
            .await?
            .output_matched("pane.wait_for_output")
    }

    /// `pane.process_info` — the pane's shell pid, tty and foreground process group.
    ///
    /// Every field but `pane_id` is best-effort: an empty `foreground_processes` means *herdr
    /// could not tell*, not *the pane is idle* (`tmp/herdr/src/app/api/panes.rs:536-538`).
    ///
    /// # Errors
    /// [`crate::ApiErrorCode::PaneNotFound`], plus the transport arms.
    pub async fn pane_process_info(
        &self,
        params: PaneProcessInfoParams,
    ) -> Result<PaneProcessInfo> {
        self.call(Method::PaneProcessInfo(params))
            .await?
            .pane_process_info("pane.process_info")
    }

    // ---------------------------------------------------------------------------------------
    // The stream half.
    // ---------------------------------------------------------------------------------------

    /// `session.snapshot` **and** `events.subscribe`, in the order herdr requires, as one call.
    ///
    /// This is the one place the correct order is written down, and it is written once so nothing
    /// has to re-derive it. It does **not** make the wrong order unreachable, and does not claim
    /// to: [`Self::session_snapshot`] and [`Self::subscribe`] are both `pub`, and
    /// `let snap = client.session_snapshot().await?; let ev = client.subscribe(subs).await?;` is
    /// exactly the losing order — [`Self::subscribe`]'s own doc says so. Anything holding a cache
    /// wants this method; the guarantee is that this method is right, not that the pair cannot be
    /// built any other way. The sequence it performs is `socket-api.mdx:118-130`'s, literally:
    ///
    /// 1. open `events.subscribe` on its own connection and **await the acknowledgement**;
    /// 2. **buffer that stream** while `session.snapshot` runs on a second connection;
    /// 3. hand back the snapshot together with the buffered-then-live stream, so the caller
    ///    installs the snapshot and then applies the buffered events in arrival order.
    ///
    /// Step 2 is a genuine race, not a sequence: the snapshot call and the stream read are polled
    /// together, because a client that stops reading a subscription does not fall behind, it loses
    /// the subscription — herdr's writes are blocking with a 5 s send timeout, after which
    /// `stream_subscriptions` returns (`tmp/herdr/src/api/server.rs:164-166`, `:762-778`). See
    /// [`crate::stream`].
    ///
    /// # Errors
    /// Every arm of [`HerdrError`]. A failure at any step leaves nothing behind: the subscription
    /// connection is dropped with the returned `Err`, so a failed bootstrap is not a half-open
    /// stream.
    pub async fn bootstrap(
        &self,
        subscriptions: Vec<Subscription>,
    ) -> Result<(SessionSnapshot, HerdrEvents)> {
        let mut events = self.subscribe(subscriptions).await?;
        let snapshot = {
            let call = self.session_snapshot();
            tokio::pin!(call);
            loop {
                // Re-read each turn: once the stream has ended there is nothing left to poll, and
                // a disabled branch is what keeps that from becoming a spin.
                let draining = !events.is_finished();
                tokio::select! {
                    biased;
                    answer = &mut call => break answer?,
                    () = events.buffer_one(), if draining => {}
                }
            }
        };
        Ok((snapshot, events))
    }

    /// `events.subscribe` — the stream on its own.
    ///
    /// For a consumer that keeps **no local cache**: a pane waiting for one status change has
    /// nothing to bootstrap, so it has no gap to avoid. Anything that holds a cache wants
    /// [`Self::bootstrap`] instead, because a snapshot taken around this call is exactly the
    /// losing order.
    ///
    /// The acknowledgement is checked, not assumed: the first line must be a success carrying this
    /// request's `id` and `{"type":"subscription_started"}`. A `pong`, an error envelope with any
    /// `id` including the empty one herdr writes when it could not recover a correlation id
    /// (`tmp/herdr/src/api/server.rs:180-201`), or a pushed event before the ack all fail here
    /// rather than yielding a stream whose state is unknown.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] when the first line is a success that is not the ack,
    /// [`HerdrError::Api`] when it is an error envelope, [`HerdrError::IdMismatch`] when a success
    /// carries another request's id, [`HerdrError::Timeout`] when no first line arrives inside
    /// [`Self::timeout`], plus the transport arms.
    pub async fn subscribe(&self, subscriptions: Vec<Subscription>) -> Result<HerdrEvents> {
        let request = Request {
            id: request_id(stream::METHOD),
            method: Method::EventsSubscribe(EventsSubscribeParams { subscriptions }),
        };
        let line = serde_json::to_string(&request).map_err(|err| HerdrError::Io(err.into()))?;

        let handshake = async {
            let socket = LocalStream::connect(&self.socket).await?;
            let mut reader = BufReader::new(socket);
            transport::write_line(&mut reader, stream::METHOD, &line).await?;
            let ack = transport::read_line(&mut reader, stream::METHOD).await?;
            Ok::<_, HerdrError>((reader, ack))
        };
        let (reader, ack) = match tokio::time::timeout(self.timeout, handshake).await {
            Ok(result) => result?,
            Err(_elapsed) => {
                return Err(HerdrError::Timeout {
                    method: stream::METHOD,
                    timeout: self.timeout,
                });
            }
        };

        // The acknowledgement's own bytes count against the stream's budget, because they came
        // off the same connection.
        let bytes = ack.len() as u64;
        match WireResponse::decode(ack.trim_end_matches(['\r', '\n'])) {
            Ok(WireResponse::Success(success)) => {
                if success.id != request.id {
                    return Err(HerdrError::IdMismatch {
                        method: stream::METHOD,
                        sent: request.id,
                        got: success.id,
                    });
                }
                success.result.subscription_started(stream::METHOD)?;
            }
            Ok(WireResponse::Error(failure)) => {
                return Err(HerdrError::Api {
                    method: stream::METHOD,
                    source: ApiError {
                        code: ApiErrorCode::from_wire(&failure.error.code),
                        message: failure.error.message,
                    },
                });
            }
            Err(source) => {
                return Err(HerdrError::Malformed {
                    method: stream::METHOD,
                    source,
                });
            }
        }
        Ok(HerdrEvents::new(reader, bytes))
    }
}

impl From<&HerdrPane> for HerdrClient {
    fn from(pane: &HerdrPane) -> Self {
        Self::for_pane(pane)
    }
}

/// A correlation id for one request.
///
/// herdr places no constraint on the shape — its own CLI sends the constant `"cli:request"`
/// (`tmp/herdr/src/cli.rs:757`) and its client sends `"api-client:status"`
/// (`tmp/herdr/src/api/client.rs:93`). A per-call counter is used here anyway, because
/// [`crate::HerdrError::IdMismatch`] can only mean something if two calls cannot share an id.
fn request_id(method: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("cyrup:{method}:{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// `ping`'s request, built the same way as every other — so [`crate::probe`] and this module
/// cannot drift apart on id shape.
pub(crate) fn ping_request() -> Request {
    Request {
        id: request_id("ping"),
        method: Method::Ping(PingParams {}),
    }
}
