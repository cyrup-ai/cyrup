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
use crate::identity::{
    ChildOrchestratorMetadata, ENV_INTERCOM_SESSION_ID, preferred_supervisor_target,
    presence_name, read_child_orchestrator_metadata,
};
use crate::inbound::spawn_inbound_loop;
use crate::paths::{agent_dir_path, broker_socket_path, intercom_dir_path};
use crate::seams::{IntercomClarifyChannel, IntercomDeliveryChannel, IntercomSteerChannel};
use crate::session_state::SharedIntercomState;
use crate::tools::contact_supervisor::ContactSupervisorTool;
use crate::tools::intercom::IntercomTool;
use crate::transport::client::IntercomClient;
use crate::transport::protocol::{SessionInfo, SessionRegistration, now_ms};
use crate::transport::spawn::ensure_broker;
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
    #[must_use]
    pub fn new(
        agent_dir: PathBuf,
        cwd: PathBuf,
        config: IntercomConfig,
        metadata: Option<ChildOrchestratorMetadata>,
    ) -> Self {
        let ask_timeout = ask_timeout_ms();
        let state = Arc::new(SharedIntercomState::new(config, ask_timeout, cwd));
        let supervisor_target = metadata.as_ref().map(preferred_supervisor_target);
        let clarify = Arc::new(IntercomClarifyChannel::new(state.clone()));
        let delivery = Arc::new(IntercomDeliveryChannel::new(state.clone(), supervisor_target));
        let steer = Arc::new(IntercomSteerChannel::new(state.clone()));
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            state,
            agent_dir,
            metadata,
            clarify,
            delivery,
            steer,
        }
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

    fn build_registration(&self, model: Option<&str>) -> SessionRegistration {
        let session_id_env = std::env::var(ENV_INTERCOM_SESSION_ID).ok();
        // The presence name is:
        //   1. a subagent child's own deterministic label (`metadata.session_name`), else
        //   2. (a top-level/plain orchestrator) the presence name derived from the LIVE `HostServices`
        //      — `presence_name(session_name, session_id)` — matching pi `buildPresenceIdentity`
        //      (`pi-intercom/index.ts:387-389`). This is REQUIRED so a spawned child can address this
        //      orchestrator: the child's `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET` is
        //      `orchestrator_presence_target(session_name, session_id)` over the SAME session id/name,
        //      so the two independently-produced strings match at the broker (before this, a top-level
        //      orchestrator had no `CYRUP_INTERCOM_SESSION_ID` → registered `name: None` → unaddressable).
        //   3. else the `CYRUP_INTERCOM_SESSION_ID`-derived alias (refined post-register).
        let name = self
            .metadata
            .as_ref()
            .and_then(|m| m.session_name.clone())
            .or_else(|| {
                self.state.host_services().and_then(|services| {
                    services
                        .session_id()
                        .filter(|id| !id.is_empty())
                        .map(|id| presence_name(services.session_name().as_deref(), &id))
                })
            })
            .or_else(|| {
                session_id_env.as_deref().map(|id| presence_name(None, id))
            });
        SessionRegistration {
            name,
            cwd: self.state.cwd.to_string_lossy().to_string(),
            model: model.unwrap_or("cyrup").to_string(),
            pid: std::process::id(),
            started_at: now_ms(),
            last_activity: now_ms(),
            status: self.state.config.status.clone(),
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
        let Some(client) = self.state.client() else {
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
                // Connect off the event path: `ensure_broker` may spawn the broker + wait up to 5s;
                // blocking the SessionStart dispatch that long is unacceptable (the port doc §2 notes
                // intercom must not stall the session), so the connect runs on a background task and
                // stashes the live client into the shared state when ready.
                let state = self.state.clone();
                let agent_dir = self.agent_dir.clone();
                let registration = self.build_registration(ctx.model());
                let session_id = std::env::var(ENV_INTERCOM_SESSION_ID).ok();
                tokio::spawn(async move {
                    if let Err(e) = ensure_broker(&agent_dir).await {
                        tracing::warn!(error = %e, "intercom: broker unavailable; coordination disabled this session");
                        return;
                    }
                    let socket = broker_socket_path(&intercom_dir_path(&agent_dir));
                    match IntercomClient::connect(&socket, registration, session_id).await {
                        Ok(client) => {
                            let client = Arc::new(client);
                            state.set_client(Some(client.clone()));
                            spawn_inbound_loop(state, client);
                        }
                        Err(e) => tracing::warn!(error = %e, "intercom: failed to register with the broker"),
                    }
                });
                HookOutcome::Noop
            }
            HostEvent::SessionShutdown { .. } => {
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
#[must_use]
pub fn intercom_extension_for_env(agent_dir: PathBuf, cwd: PathBuf) -> Option<Arc<dyn NativeExtension>> {
    intercom_extension_for_env_concrete(agent_dir, cwd).map(|ext| ext as Arc<dyn NativeExtension>)
}

/// As [`intercom_extension_for_env`], but returns the CONCRETE [`IntercomExtension`] so the caller
/// (`crates/cyrup/src/main.rs`) can extract its [`IntercomExtension::clarify_channel`]/
/// [`IntercomExtension::delivery_channel`] seam channels and hand them to
/// `SubagentsExtension::with_channels` (the port doc §8.4 item 1 / P5 handoff — CLOSING R-SA-037/
/// 119/120/123/124/125) BEFORE attaching this same extension via `.with_native_extension(..)`. The
/// two seam channels reference the one `SharedIntercomState` this extension owns, so handing them
/// out and then attaching the extension wires BOTH ends to the same live broker client.
#[must_use]
pub fn intercom_extension_for_env_concrete(agent_dir: PathBuf, cwd: PathBuf) -> Option<Arc<IntercomExtension>> {
    let intercom_dir = intercom_dir_path(&agent_dir);
    let config = load_config(&intercom_dir);
    if !config.enabled {
        return None;
    }
    let metadata = read_child_orchestrator_metadata();
    if metadata.is_none() && !is_installed(&intercom_dir) {
        return None;
    }
    Some(Arc::new(IntercomExtension::new(agent_dir, cwd, config, metadata)))
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
        assert!(intercom_extension_for_env(dir.path().to_path_buf(), dir.path().to_path_buf()).is_none());
    }

    #[test]
    fn plain_session_without_optin_attaches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // No config.json, no CYRUP_INTERCOM, no child metadata (env not set in this test process) →
        // a plain session attaches nothing (zero overhead, no broker spawned).
        assert!(!is_installed(&intercom_dir_path(dir.path())));
    }

    #[test]
    fn installed_when_config_file_present() {
        let dir = tempfile::tempdir().unwrap();
        let intercom_dir = intercom_dir_path(dir.path());
        std::fs::create_dir_all(&intercom_dir).unwrap();
        std::fs::write(config_path(&intercom_dir), "{}").unwrap();
        assert!(is_installed(&intercom_dir));
        assert!(intercom_extension_for_env(dir.path().to_path_buf(), dir.path().to_path_buf()).is_some());
    }

    #[test]
    fn extension_exposes_all_seam_channels() {
        let dir = tempfile::tempdir().unwrap();
        let ext = IntercomExtension::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            IntercomConfig::default(),
            None,
        );
        // All three channels are constructed + reachable (handed to SubagentsExtension::with_channels).
        let _c = ext.clarify_channel();
        let _d = ext.delivery_channel();
        let _s = ext.steer_channel();
    }
}
