//! The [`IntercomExtension`] `NativeExtension` facade + the binary-wiring entry point
//! [`intercom_extension_for_env`] (a port of `pi-intercom/index.ts:430` `piIntercomExtension(pi)`).
//!
//! WIRING (all reachable in this phase, no dead primitives):
//! - `init` registers the `intercom` tool always, and `contact_supervisor` ONLY when child-
//!   orchestrator metadata is present (`index.ts:1162-1163`); it subscribes the lifecycle events.
//! - `on_event(SessionStart)` spawns the connect: `ensure_broker` (re-exec the detached broker) →
//!   `IntercomClient::connect` → stash the live client + start the inbound event loop (the outbound
//!   waiter match + `ReplyTracker` record, `index.ts:709-764`).
//! - `on_event(SessionShutdown)` disconnects; the agent/tool lifecycle events drive presence
//!   (`index.ts:562-621`).
//! - [`intercom_extension_for_env`] is called at the three `crates/cyrup/src/main.rs` session-build
//!   sites, child-mode gated (a subagent child with metadata always attaches so `contact_supervisor`
//!   registers; a plain session attaches only when opt-in-installed).
//!
//! CHANNEL HANDOFF (WIRED): the three seam channels ([`Self::clarify_channel`]/
//! [`Self::delivery_channel`]/[`Self::steer_channel`]) are handed into
//! `SubagentsExtension::with_channels` at the three `crates/cyrup/src/main.rs` session-build sites,
//! replacing subagents' `NoTransportChannel`/no-live-`AskLock`/`NoTransportSteerChannel` degrade
//! defaults with these broker-backed impls — see [`crate::seams`].
//!
//! LOCAL SURFACE (WIRED): a top-level orchestrator (no supervisor to relay to) surfaces a delivered
//! subagent result LOCALLY through the live `HostServices` — `append_entry` + an `inject_message`
//! trigger-turn (`deliverLocalSubagentRelayMessage`, `index.ts:889-910`), see the
//! `IntercomDeliveryChannel::send` no-supervisor branch in [`crate::seams`].

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::ExtensionId;
use cyrup_ext::registry::CommandDescriptor;
use cyrup_ext::{EventKind, ExtError, HostCtx, HostEvent, HookOutcome, HostServices, InitApi, NativeExtension};
use cyrup_ext_subagents::tui::intercom::{ClarifyChannel, DeliveryChannel, SteerChannel};

use crate::config::{IntercomConfig, ask_timeout_ms, config_path, load_config};
use crate::connect::{self, ConnectParams};
use crate::identity::{
    ChildOrchestratorMetadata, preferred_supervisor_target, read_child_orchestrator_metadata,
};
use crate::inbound::schedule_inbound_flush;
use crate::paths::{agent_dir_path, intercom_dir_path};
use crate::seams::{IntercomClarifyChannel, IntercomDeliveryChannel, IntercomSteerChannel};
use crate::session_state::SharedIntercomState;
use crate::tools::contact_supervisor::ContactSupervisorTool;
use crate::tools::intercom::IntercomTool;
use crate::transport::client::IntercomClient;
use crate::transport::protocol::{SessionInfo, now_ms};
use crate::ui::compose::COMPOSE_MAX_WIDTH;
use crate::ui::{ComposeOverlay, DefaultKeybindings, PlainTheme, SessionListOverlay, compose_send};

/// The `/intercom` overlay slash command (pi `pi.registerCommand("intercom", …)`, `index.ts:1877`).
pub const INTERCOM_COMMAND: &str = "intercom";
/// The width the `/intercom` session picker renders at (the session-list overlay's max width).
const INTERCOM_OVERLAY_WIDTH: usize = crate::ui::session_list::SESSION_LIST_MAX_WIDTH;

/// The extension's fixed id.
pub const EXTENSION_ID: &str = "cyrup-intercom";
/// The explicit opt-in flag: set truthy to attach intercom to a plain (non-child) session.
pub const INSTALL_ENV_VAR: &str = "CYRUP_INTERCOM";

/// The intercom native extension.
pub struct IntercomExtension {
    id: ExtensionId,
    state: Arc<SharedIntercomState>,
    agent_dir: PathBuf,
    metadata: Option<ChildOrchestratorMetadata>,
    clarify: Arc<IntercomClarifyChannel>,
    delivery: Arc<IntercomDeliveryChannel>,
    steer: Arc<IntercomSteerChannel>,
}

impl IntercomExtension {
    /// Build the extension over a resolved config + optional child metadata + this session's cwd.
    ///
    /// # Errors
    /// pi `getAskTimeoutMs` throws (uncaught) when `PI_INTERCOM_ASK_TIMEOUT_MS`/
    /// `CYRUP_INTERCOM_ASK_TIMEOUT_MS` is set but is not a positive integer number of milliseconds
    /// (`config.ts:14-16`), which crashes the whole `piIntercomExtension(pi)` construction
    /// (`index.ts:433`). This mirrors that: an invalid env value is a hard `Err`, never a silent
    /// default.
    pub fn new(
        agent_dir: PathBuf,
        cwd: PathBuf,
        config: IntercomConfig,
        metadata: Option<ChildOrchestratorMetadata>,
    ) -> Result<Self, String> {
        let ask_timeout = ask_timeout_ms()?;
        let state = Arc::new(SharedIntercomState::new(config, ask_timeout, cwd));
        let supervisor_target = metadata.as_ref().map(preferred_supervisor_target);
        let clarify = Arc::new(IntercomClarifyChannel::new(state.clone()));
        let delivery = Arc::new(IntercomDeliveryChannel::new(state.clone(), supervisor_target));
        let steer = Arc::new(IntercomSteerChannel::new(state.clone()));
        Ok(Self {
            id: ExtensionId::from(EXTENSION_ID),
            state,
            agent_dir,
            metadata,
            clarify,
            delivery,
            steer,
        })
    }

    /// The broker-backed [`ClarifyChannel`] this extension owns. HANDED (WIRED) into
    /// `SubagentsExtension::with_channels(.., clarify)` at the `main.rs` sites (the port doc §8.4
    /// item 1); the exec detach-trigger arm fires it on a child's blocking ask. See [`crate::seams`].
    #[must_use]
    pub fn clarify_channel(&self) -> Arc<dyn ClarifyChannel> {
        self.clarify.clone()
    }

    /// The broker-backed [`DeliveryChannel`] this extension owns. HANDED (WIRED) into
    /// `SubagentsExtension::with_channels(.., delivery)` at the `main.rs` sites (the port doc §8.4
    /// item 1); the run driver's `deliver_group_out_of_band` invokes it. See [`crate::seams`].
    #[must_use]
    pub fn delivery_channel(&self) -> Arc<dyn DeliveryChannel> {
        self.delivery.clone()
    }

    /// The broker-backed [`SteerChannel`] this extension owns. HANDED (WIRED) into
    /// `SubagentsExtension::with_channels(.., steer)` at the `main.rs` sites; the subagents
    /// `control_resume` `SteerRunning` arm fires it to DELIVER `action='resume'`'s follow-up to a
    /// still-running async child's registered bridge target over the broker (R-SA-086, pi
    /// `subagent-executor.ts:860-878`). Backed by the SAME `SharedIntercomState` broker client the
    /// delivery/clarify channels use. See [`crate::seams::IntercomSteerChannel`].
    #[must_use]
    pub fn steer_channel(&self) -> Arc<dyn SteerChannel> {
        self.steer.clone()
    }

    /// The shared session state (exposed for tests + a future P4/P5 consumer).
    #[must_use]
    pub fn state(&self) -> &Arc<SharedIntercomState> {
        &self.state
    }

    /// The connect params every attempt (the startup one and every reconnect rung) rebuilds its
    /// registration from — see [`crate::connect::build_registration`], which is where this
    /// extension's former `build_registration` moved so a reconnect produces an IDENTICAL
    /// registration instead of a stale snapshot captured once at `SessionStart`.
    fn connect_params(&self, model: Option<&str>) -> ConnectParams {
        ConnectParams {
            agent_dir: self.agent_dir.clone(),
            metadata: self.metadata.clone(),
            model: model.map(str::to_string),
        }
    }

    /// Derive the presence status suffix (`currentStatus`, `index.ts:562-583`) for a lifecycle
    /// transition, appending the optional configured status suffix.
    fn presence_status(&self, base: &str) -> String {
        match &self.state.config.status {
            Some(suffix) if !suffix.trim().is_empty() => format!("{base} · {suffix}"),
            _ => base.to_string(),
        }
    }

    fn sync_presence(&self, base: &str) {
        if let Some(client) = self.state.client() {
            client.update_presence(None, Some(self.presence_status(base)), None);
        }
    }

    /// The `/intercom` command body (pi `openIntercomOverlay`, `index.ts:1810-1874`, degraded to text
    /// per the port doc §4.3): list sessions over the live broker, then either render the session
    /// picker (no args) or resolve `<target>` + send `<message…>` via [`compose_send`].
    async fn run_intercom_command(&self, client: &Arc<IntercomClient>, args: &str) -> crate::Result<String> {
        let sessions = client.list_sessions().await?;
        let my_id = client.session_id();
        let Some(current) = my_id
            .as_deref()
            .and_then(|id| sessions.iter().find(|s| s.id == id).cloned())
        else {
            return Ok("Current session is missing from the intercom session list.".to_string());
        };
        let others: Vec<SessionInfo> = sessions
            .into_iter()
            .filter(|s| Some(s.id.as_str()) != my_id.as_deref())
            .collect();

        // No args → the session picker (session-list overlay rendered as text).
        let render_picker = |others: Vec<SessionInfo>| {
            SessionListOverlay::new(current.clone(), others)
                .render(&PlainTheme, &DefaultKeybindings, INTERCOM_OVERLAY_WIDTH)
                .join("\n")
        };
        if args.is_empty() {
            return Ok(render_picker(others));
        }

        // `<target> <message…>` → resolve + send.
        let (target, message) = match args.split_once(char::is_whitespace) {
            Some((t, m)) => (t.trim().to_string(), m.trim().to_string()),
            None => (args.to_string(), String::new()),
        };
        if message.is_empty() {
            // Target but no body → render the compose box for it (pi's ComposeOverlay), or the picker.
            if let Some(target_id) = self.state.resolve_target(client, &target).await?
                && let Some(session) = others.iter().find(|s| s.id == target_id).cloned()
            {
                let label = session.name.clone().unwrap_or_else(|| session.id.clone());
                let compose = ComposeOverlay::new(session, label)
                    .render(&PlainTheme, &DefaultKeybindings, COMPOSE_MAX_WIDTH)
                    .join("\n");
                return Ok(format!("{compose}\n\nType `/intercom {target} <message>` to send."));
            }
            return Ok(format!("Usage: /intercom <session> <message>\n\n{}", render_picker(others)));
        }
        let Some(target_id) = self.state.resolve_target(client, &target).await? else {
            return Ok(format!("No intercom session matches \"{target}\"."));
        };
        compose_send(client, &target_id, &message).await?;
        Ok(format!("Message sent to {target}."))
    }
}

#[async_trait]
impl NativeExtension for IntercomExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        // `intercom` is always registered; `contact_supervisor` only for a subagent child with
        // orchestrator metadata (index.ts:1162-1163,1425).
        api.register_tool(Arc::new(IntercomTool::new(self.state.clone())));
        if let Some(metadata) = &self.metadata {
            api.register_tool(Arc::new(ContactSupervisorTool::new(self.state.clone(), metadata.clone())));
        }
        // The `/intercom` overlay command (pi `registerCommand("intercom", …)`, index.ts:1877). cyrup
        // has no `register_shortcut`, so the `alt+m` binding degrades to this command (the port doc
        // §4.3); `execute_command` renders the session picker + drives the compose send.
        api.register_command(
            INTERCOM_COMMAND,
            CommandDescriptor { description: "Open the session intercom picker / send a message".to_string(), completions: Vec::new() },
        );
        // Lifecycle: connect/disconnect + presence sync (never blocks/mutates a tool call).
        api.subscribe(&[
            EventKind::SessionStart,
            EventKind::SessionShutdown,
            EventKind::AgentStart,
            EventKind::AgentEnd,
            EventKind::ToolExecStart,
            EventKind::ToolExecEnd,
            // `pi.on("turn_start"/"turn_end")` (index.ts:1112-1127,1074-1080) drives
            // `replyTracker.beginTurn()`/`endTurn()` so `resolveReplyTarget`'s `currentTurnContext`
            // priority branch (reply_tracker.rs) is ever reachable in production.
            EventKind::TurnStart,
            EventKind::TurnEnd,
        ]);
        Ok(())
    }

    /// Late-bind the live `HostServices` backend (P-1 Route B, the port doc §4.1). The builder calls
    /// this via `load_native_with_services` (facade.rs:181) BEFORE `init`; stash the shared `Arc` so
    /// the inbound surface ([`crate::inbound::surface_incoming_message`]) and the ClarifyChannel human
    /// answer ([`crate::seams::IntercomClarifyChannel::ask`]) reach `append_entry`/`input` from their
    /// background tasks OUTSIDE any `HostCtx`. Idempotent (a session rebuild rebinds the same Arc).
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        self.state.set_host_services(services);
    }

    /// Dispatch the `/intercom` command (command-tier). No args → render the session picker; `<target>
    /// <message…>` → resolve the target and send it over the broker (the port doc §4.3 degrade of pi's
    /// interactive overlay).
    async fn execute_command(&self, name: &str, args: &str, ctx: &HostCtx) -> Result<Option<String>, ExtError> {
        ctx.require_command_tier()?;
        if name != INTERCOM_COMMAND {
            return Err(ExtError::Component(format!("native extension has no handler for command `{name}`")));
        }
        // pi's overlay opens through `ensureConnected("overlay")` (index.ts:1827,1864) rather than a
        // bare `client` read: an overlay is a deliberate user action, so it is worth (re)spawning the
        // broker and reconnecting for. A failure still degrades to the same text, never a hard error.
        let Ok(client) = connect::ensure_connected(&self.state, connect::ConnectReason::Overlay).await else {
            return Ok(Some("Intercom is not connected in this session.".to_string()));
        };
        let output = self
            .run_intercom_command(&client, args.trim())
            .await
            .unwrap_or_else(|e| format!("intercom command failed: {e}"));
        Ok(Some(output))
    }

    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionStart { .. } => {
                // Capture this session's static `has_ui` (pi `hasUI`) ONCE, before the inbound loop
                // starts, so the inbound delivery policy (`inbound.rs`) can pick the interactive
                // trigger-turn branch vs. the non-interactive busy auto-reply (index.ts:739-758).
                self.state.set_has_ui(ctx.has_ui);
                // `startSessionRuntime` (index.ts:926-951): publish the params every connect attempt
                // rebuilds its registration from, clear the shutdown latch, bump the generation and
                // reset the backoff ladder.
                connect::begin_runtime(&self.state, self.connect_params(ctx.model()));
                // Connect off the event path: `ensure_broker` may spawn the broker + wait up to 5s;
                // blocking the SessionStart dispatch that long is unacceptable (the port doc §2 notes
                // intercom must not stall the session), so the connect runs on a background task and
                // stashes the live client into the shared state when ready. pi does the same via a
                // `setTimeout(…, 0)` (index.ts:952-965) — including the failure arm below, which is
                // the whole point of ICOM-003: a broker that is not up YET must not disable intercom
                // for the rest of the session, it must arm the reconnect ladder.
                let state = self.state.clone();
                tokio::spawn(async move {
                    if let Err(e) = connect::ensure_connected(&state, connect::ConnectReason::Startup).await {
                        tracing::warn!(error = %e, "intercom: startup connect failed; scheduling reconnect");
                        connect::schedule_reconnect(&state);
                    }
                });
                HookOutcome::Noop
            }
            HostEvent::SessionShutdown { .. } => {
                // `clearInboundFlushTimer()` (index.ts:1070): a pending debounce must not outlive the
                // session and fire against a torn-down host.
                self.state.set_flush_timer(None);
                // `shuttingDown = true; disposed = true; clearReconnectTimer()` (index.ts:1060-1064)
                // BEFORE the disconnect below, so the disconnect edge this triggers cannot arm a
                // reconnect: a deliberate shutdown never reconnects.
                connect::shutdown(&self.state);
                self.state.waiter.fail_pending("Session shutting down");
                if let Some(client) = self.state.client() {
                    client.disconnect();
                }
                self.state.set_client(None);
                self.state.tracker.lock().unwrap_or_else(|e| e.into_inner()).reset();
                HookOutcome::Noop
            }
            HostEvent::AgentStart => {
                self.sync_presence("thinking");
                HookOutcome::Noop
            }
            HostEvent::AgentEnd { .. } => {
                self.sync_presence("idle");
                // `pi.on("agent_end") -> scheduleInboundFlush(0)` (index.ts:1116-1117): the run that
                // was in flight has ended, so drain anything that arrived while this session was
                // busy (`InboundPolicy::Queue`) instead of leaving it parked until the next message.
                schedule_inbound_flush(&self.state, 0);
                HookOutcome::Noop
            }
            HostEvent::ToolExecStart { name, .. } => {
                self.sync_presence(&format!("tool:{name}"));
                HookOutcome::Noop
            }
            HostEvent::ToolExecEnd { .. } => {
                self.sync_presence("thinking");
                HookOutcome::Noop
            }
            HostEvent::TurnStart { .. } => {
                // `pi.on("turn_start") -> replyTracker.beginTurn()` (index.ts:1112-1127): prune expired
                // pending asks, then adopt the oldest queued turn context (queued by
                // `trigger_turn_over_inbound` right before this turn started) as `current_turn_context`.
                self.state.tracker.lock().unwrap_or_else(|e| e.into_inner()).begin_turn(now_ms());
                HookOutcome::Noop
            }
            HostEvent::TurnEnd { .. } => {
                // `pi.on("turn_end") -> replyTracker.endTurn()` + `scheduleInboundFlush(0)`
                // (index.ts:1080-1086).
                self.state.tracker.lock().unwrap_or_else(|e| e.into_inner()).end_turn();
                schedule_inbound_flush(&self.state, 0);
                HookOutcome::Noop
            }
            _ => HookOutcome::Noop,
        }
    }
}

// ================================================================================= binary wiring

fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref().map(str::trim),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

/// Whether intercom is "installed" for a plain (non-child) session: an explicit `CYRUP_INTERCOM`
/// opt-in, or a `<intercomDir>/config.json` present. A subagent child with orchestrator metadata is
/// always attached regardless (it needs `contact_supervisor`).
#[must_use]
pub fn is_installed(intercom_dir: &std::path::Path) -> bool {
    env_truthy(INSTALL_ENV_VAR) || config_path(intercom_dir).exists()
}

/// The binary-side entry point `crates/cyrup/src/main.rs` calls at each of its three session-build
/// sites (mirrors `subagent_extension_for_env`/`permission_extension_for_env`). Returns `None`
/// (attach nothing) when intercom is disabled, or when this is a plain session that has not opted in.
///
/// A subagent child (child-orchestrator metadata present) always attaches so `contact_supervisor` is
/// registered; a plain session attaches only when opted in (`is_installed`).
///
/// # Errors
/// See [`IntercomExtension::new`] — propagates a hard error when the ask-timeout env var is set but
/// invalid, matching pi's uncaught throw (`config.ts:14-16`).
pub fn intercom_extension_for_env(
    agent_dir: PathBuf,
    cwd: PathBuf,
) -> Result<Option<Arc<dyn NativeExtension>>, String> {
    Ok(intercom_extension_for_env_concrete(agent_dir, cwd)?.map(|ext| ext as Arc<dyn NativeExtension>))
}

/// As [`intercom_extension_for_env`], but returns the CONCRETE [`IntercomExtension`] so the caller
/// (`crates/cyrup/src/main.rs`) can extract its [`IntercomExtension::clarify_channel`]/
/// [`IntercomExtension::delivery_channel`] seam channels and hand them to
/// `SubagentsExtension::with_channels` (the port doc §8.4 item 1 / P5 handoff — CLOSING R-SA-037/
/// 119/120/123/124/125) BEFORE attaching this same extension via `.with_native_extension(..)`. The
/// two seam channels reference the one `SharedIntercomState` this extension owns, so handing them
/// out and then attaching the extension wires BOTH ends to the same live broker client.
///
/// # Errors
/// See [`IntercomExtension::new`].
pub fn intercom_extension_for_env_concrete(
    agent_dir: PathBuf,
    cwd: PathBuf,
) -> Result<Option<Arc<IntercomExtension>>, String> {
    let intercom_dir = intercom_dir_path(&agent_dir);
    let config = load_config(&intercom_dir);
    if !config.enabled {
        return Ok(None);
    }
    let metadata = read_child_orchestrator_metadata();
    if metadata.is_none() && !is_installed(&intercom_dir) {
        return Ok(None);
    }
    Ok(Some(Arc::new(IntercomExtension::new(agent_dir, cwd, config, metadata)?)))
}

/// The default agent dir (`~/.cyrup` or `$CYRUP_CODING_AGENT_DIR`) — a convenience for a caller that
/// has not already resolved one.
#[must_use]
pub fn default_agent_dir() -> PathBuf {
    agent_dir_path()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn disabled_config_attaches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let intercom_dir = intercom_dir_path(dir.path());
        std::fs::create_dir_all(&intercom_dir).unwrap();
        std::fs::write(config_path(&intercom_dir), r#"{"enabled":false}"#).unwrap();
        // enabled:false → None regardless of install/child state.
        assert!(
            intercom_extension_for_env(dir.path().to_path_buf(), dir.path().to_path_buf())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn plain_session_without_optin_attaches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // No config.json, no child metadata. `is_installed` ORs the `CYRUP_INTERCOM` env signal with
        // the config-file signal, so account for whatever this process's ambient env already is
        // (e.g. a developer/CI shell with `CYRUP_INTERCOM=1` set workspace-wide) rather than assuming
        // it is unset — this crate is `#![forbid(unsafe_code)]`, so a `src/` test cannot sandbox the
        // process env via `set_var`/`remove_var` to force the "no env" case.
        let env_opted_in = env_truthy(INSTALL_ENV_VAR);
        assert_eq!(
            is_installed(&intercom_dir_path(dir.path())),
            env_opted_in,
            "with no config.json, installed iff the ambient env already opted in"
        );
    }

    #[test]
    fn installed_when_config_file_present() {
        let dir = tempfile::tempdir().unwrap();
        let intercom_dir = intercom_dir_path(dir.path());
        std::fs::create_dir_all(&intercom_dir).unwrap();
        std::fs::write(config_path(&intercom_dir), "{}").unwrap();
        assert!(is_installed(&intercom_dir));
        assert!(
            intercom_extension_for_env(dir.path().to_path_buf(), dir.path().to_path_buf())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn extension_exposes_all_seam_channels() {
        let dir = tempfile::tempdir().unwrap();
        let ext = IntercomExtension::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            IntercomConfig::default(),
            None,
        )
        .unwrap();
        // All three channels are constructed + reachable (handed to SubagentsExtension::with_channels).
        let _c = ext.clarify_channel();
        let _d = ext.delivery_channel();
        let _s = ext.steer_channel();
    }

    /// Regression proof: pre-fix, `HostEvent::TurnStart`/`TurnEnd` had no arm in `on_event` (the match
    /// fell through to `_ => HookOutcome::Noop`) and neither was subscribed, so `ReplyTracker::begin_turn`
    /// was never invoked in production — a context queued by `inbound.rs::trigger_turn_over_inbound`
    /// would sit in `pending_turn_contexts` forever and `resolve_reply_target`'s `current_turn_context`
    /// priority branch (reply-tracker.ts:37-40,66-68; pi `pi.on("turn_start")`, index.ts:1112-1127) was
    /// permanently dead code. This test fails against that pre-fix behavior: it queues a turn context
    /// directly (mirroring what `trigger_turn_over_inbound` now does), dispatches a real `TurnStart`
    /// event through `on_event`, and asserts a bare `resolve_reply_target(None, None, ..)` (no `to`)
    /// resolves to that queued context even though a SECOND, unrelated pending ask also exists — the
    /// exact "two pending asks, bare reply resolves to the one that triggered this turn" scenario the
    /// dossier describes.
    #[tokio::test]
    async fn turn_start_event_adopts_the_queued_context_as_current() {
        let dir = tempfile::tempdir().unwrap();
        let ext = IntercomExtension::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            IntercomConfig::default(),
            None,
        )
        .unwrap();

        let triggering = crate::reply_tracker::IntercomContext {
            from: SessionInfo {
                id: "s-trigger".to_string(),
                name: Some("trigger-sender".to_string()),
                cwd: "/w".to_string(),
                model: "m".to_string(),
                pid: 1,
                started_at: 0,
                last_activity: 0,
                status: None,
                peer_uid: None,
                trusted_local: None,
            },
            message: crate::transport::protocol::Message {
                id: "q-trigger".to_string(),
                timestamp: 0,
                reply_to: None,
                expects_reply: Some(true),
                content: crate::transport::protocol::MessageContent {
                    text: "the message that triggered this turn".to_string(),
                    attachments: None,
                },
            },
            received_at: now_ms(),
        };
        {
            let mut tracker = ext.state().tracker.lock().unwrap();
            // The turn-triggering context, queued by `trigger_turn_over_inbound` before this turn began.
            tracker.queue_turn_context(triggering.clone());
            // An unrelated, older pending ask (e.g. from a different session) that must NOT win.
            tracker.record_incoming_message(
                SessionInfo {
                    id: "s-other".to_string(),
                    name: Some("other-sender".to_string()),
                    cwd: "/w".to_string(),
                    model: "m".to_string(),
                    pid: 2,
                    started_at: 0,
                    last_activity: 0,
                    status: None,
                    peer_uid: None,
                    trusted_local: None,
                },
                crate::transport::protocol::Message {
                    id: "q-other".to_string(),
                    timestamp: 0,
                    reply_to: None,
                    expects_reply: Some(true),
                    content: crate::transport::protocol::MessageContent {
                        text: "unrelated older ask".to_string(),
                        attachments: None,
                    },
                },
                now_ms(),
            );
        }

        let ctx = HostCtx::event(cyrup_ext::ExtMode::Print, false, dir.path().to_path_buf());
        let ev = HostEvent::TurnStart { turn_index: 0, timestamp: now_ms() };
        let _ = ext.on_event(&ev, &ctx).await;

        let resolved = ext
            .state()
            .tracker
            .lock()
            .unwrap()
            .resolve_reply_target(None, None, now_ms())
            .expect("current_turn_context resolves a bare reply with no `to`, despite 2 pending asks");
        assert_eq!(resolved.message.id, triggering.message.id);
    }

    /// Regression proof for the `IntercomExtension::new` fallibility change (pi `getAskTimeoutMs`
    /// throws uncaught on an invalid `PI_INTERCOM_ASK_TIMEOUT_MS`/`CYRUP_INTERCOM_ASK_TIMEOUT_MS`,
    /// `config.ts:14-16`, crashing `piIntercomExtension(pi)` construction, `index.ts:433`). This crate
    /// `#![forbid(unsafe_code)]`, so this test cannot mutate the real process env (`set_var`/
    /// `remove_var` are `unsafe`) to drive `new` through its env-sourced `ask_timeout_ms()` — the
    /// injectable core of that validation (never-default-on-invalid-input) is proven directly by
    /// `config::tests::ask_timeout_invalid_value_is_a_hard_error_not_a_silent_default` instead. What
    /// THIS test proves is
    /// the wiring half of the same fix: `new` returns a plain `Self` in `extension.rs:84`'s pre-fix
    /// signature (`SharedIntercomState::new(config, ask_timeout, cwd)` fed a bare `u64`) would not
    /// typecheck against today's `Result<Self, String>` — `.unwrap()` below only compiles because `new`
    /// actually returns a `Result` that must be unwrapped, never a bare `Self`.
    #[test]
    fn new_returns_result_that_must_be_unwrapped() {
        let dir = tempfile::tempdir().unwrap();
        let ext: Result<IntercomExtension, String> = IntercomExtension::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            IntercomConfig::default(),
            None,
        );
        assert!(ext.unwrap().state().ask_timeout_ms > 0);
    }
}
