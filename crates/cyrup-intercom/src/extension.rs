//! The [`IntercomExtension`] `NativeExtension` facade + the binary-wiring entry point
//! [`intercom_extension_for_env`] (a port of `pi-intercom/index.ts:430` `piIntercomExtension(pi)`).
//!
//! WIRING (all reachable in this phase, no dead primitives):
//! - `init` registers the `intercom` tool always, and `contact_supervisor` ONLY when child-
//!   orchestrator metadata is present (`index.ts:1162-1163`); it subscribes the lifecycle events.
//! - `init` also registers BOTH slash commands: `/intercom` (`index.ts:2360-2363`) and
//!   `/intercom-id` (`v0.9.2 index.ts:2365-2368`), dispatched by [`IntercomExtension::execute_command`].
//! - `on_event(SessionStart)` spawns the connect: `ensure_broker` (re-exec the detached broker) →
//!   `IntercomClient::connect` → stash the live client + start the inbound event loop (the outbound
//!   waiter match + `ReplyTracker` record, `index.ts:709-764`).
//! - `on_event(SessionShutdown)` disconnects; the agent/tool lifecycle events drive presence
//!   (`index.ts:562-621`).
//! - [`intercom_extension_for_env`] is called at the three `crates/cyrup/src/main.rs` session-build
//!   sites, child-mode gated (a subagent child with metadata always attaches so `contact_supervisor`
//!   registers; a plain session attaches only when opt-in-installed).
//!
//! CHANNEL HANDOFF (WIRED): the three seam channels ([`IntercomExtension::clarify_channel`]/
//! [`IntercomExtension::delivery_channel`]/[`IntercomExtension::steer_channel`]) are handed into
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
/// The `/intercom-id` handoff-snippet slash command
/// (`v0.9.2 index.ts:2365-2368` — `pi.registerCommand("intercom-id", { description, handler })`).
///
/// VERSION-LAG, not a port bug: added upstream in **v0.8.0** (`v0.9.2 CHANGELOG.md:31`, "Added
/// `/intercom-id` to insert a stable handoff snippet for the current session into the editor");
/// `git grep intercom-id v0.7.0` (cyrup's ported baseline) returns nothing.
pub const INTERCOM_ID_COMMAND: &str = "intercom-id";
/// The width the `/intercom` session picker renders at (the session-list overlay's max width).
const INTERCOM_OVERLAY_WIDTH: usize = crate::ui::session_list::SESSION_LIST_MAX_WIDTH;

/// pi `formatIntercomContactSnippet(sessionId)` (`v0.9.2 index.ts:412-414`, 3 lines):
///
/// ```text
/// return `Use pi-intercom: intercom({ action: "send", to: "${sessionId}", message: "..." })`;
/// ```
///
/// `pi-intercom` → `cyrup-intercom` is the standard port rebrand (same class as `.pi/` → `.cyrup/`
/// and [`EXTENSION_ID`]); the snippet is a hint the user pastes into a prompt so a peer agent knows
/// how to address this session, and naming an extension that does not exist under this binary would
/// make it wrong. The wire `protocol` string stays `pi-intercom`
/// ([`crate::transport::protocol::PROTOCOL_NAME`]) precisely because THAT one is compatibility, not
/// branding. Everything else — the tool name, the argument names, the literal `"..."` placeholder —
/// is byte-for-byte upstream.
#[must_use]
fn format_intercom_contact_snippet(session_id: &str) -> String {
    format!(r#"Use cyrup-intercom: intercom({{ action: "send", to: "{session_id}", message: "..." }})"#)
}

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
    /// `nativeSupervisorChannelAvailable` (`v0.10.1 index.ts:1504`), probed ONCE at construction the
    /// way [`read_child_orchestrator_metadata`] is, never re-read inside `init`.
    native_supervisor_channel: bool,
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
        // Publish the metadata so `SharedIntercomState::sync_presence_identity` (reachable from the
        // `intercom` tool, `v0.10.1 index.ts:1853`) derives the SAME presence name
        // `connect::build_registration` does.
        state.set_presence_metadata(metadata.clone());
        let supervisor_target = metadata.as_ref().map(preferred_supervisor_target);
        let clarify = Arc::new(IntercomClarifyChannel::new(state.clone()));
        let delivery = Arc::new(IntercomDeliveryChannel::new(state.clone(), supervisor_target));
        let steer = Arc::new(IntercomSteerChannel::new(state.clone()));
        Ok(Self {
            id: ExtensionId::from(EXTENSION_ID),
            state,
            agent_dir,
            metadata,
            native_supervisor_channel: crate::identity::native_supervisor_channel_available(),
            clarify,
            delivery,
            steer,
        })
    }

    /// Override the `nativeSupervisorChannelAvailable` probe (`v0.10.1 index.ts:1504`) instead of
    /// reading the process environment — for tests, which must not mutate process-global env state.
    #[must_use]
    pub fn with_native_supervisor_channel(mut self, available: bool) -> Self {
        self.native_supervisor_channel = available;
        self
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

    /// `syncPresenceStatus()` (`v0.10.1 index.ts:843-849`, 7 lines):
    ///
    /// ```text
    /// if (!client || !currentSessionId || !getLiveContext()) return;
    /// // context% rides the status heartbeat so peers see live usage at turn boundaries.
    /// client.updatePresence({ status: currentStatus(), ...currentContextUsage() });
    /// ```
    ///
    /// The status is derived by [`SharedIntercomState::current_status`] from the active-tool map and
    /// the `agentRunning` flag — it is NOT a per-call-site literal. It used to be: each lifecycle
    /// arm passed its own `base` string, so with two overlapping tool calls the first `ToolExecEnd`
    /// reset presence to `thinking` while the other tool was still running.
    fn sync_presence_status(&self) {
        if let Some(client) = self.state.client() {
            let ctx_usage = self.state.current_context_usage();
            client.update_presence_with_context(
                None,
                Some(self.state.current_status()),
                None,
                ctx_usage.pct,
                ctx_usage.tokens,
                ctx_usage.window,
            );
        }
    }

    /// `syncPresenceIdentity(sessionId)` (`v0.10.1 index.ts:808-815`, 8 lines):
    ///
    /// ```text
    /// if (!client || !getLiveContext()) return;
    /// const identity = buildPresenceIdentity(pi, currentIntercomSessionId ?? sessionId);
    /// lastPresenceName = identity.name;
    /// client.updatePresence({ ...identity, status: currentStatus(), ...currentContextUsage() });
    /// ```
    ///
    /// The difference from [`Self::sync_presence_status`] is the **name**: this one re-derives it
    /// from the live host, so a session renamed by `/name`, a branch switch or a title change stops
    /// advertising its startup label. Upstream calls it from three places — the name poll, every
    /// `turn_start`, and the head of every `intercom` tool call.
    pub fn sync_presence_identity(&self) {
        self.state.sync_presence_identity();
    }


    /// pi `insertIntoEditor(ctx, text)` (`v0.9.2 index.ts:2261-2268`, 8 lines):
    ///
    /// ```text
    /// if (!ctx.hasUI) return false;
    /// const ui = ctx.ui as { getEditorText?; setEditorText? };
    /// if (typeof ui.setEditorText !== "function") return false;
    /// const existing = typeof ui.getEditorText === "function" ? ui.getEditorText() : "";
    /// ui.setEditorText(existing.trim() ? `${existing.trimEnd()}\n\n${text}` : text);
    /// return true;
    /// ```
    ///
    /// The two upstream capability probes (`ctx.hasUI`, `typeof ui.setEditorText === "function"`)
    /// collapse to `ctx.has_ui` + "a live `HostServices` backend is bound": cyrup's trait always
    /// *has* `set_editor_text`, but its default impl is a silent no-op
    /// (`cyrup-ext/src/host/services.rs:250`), so an unbound backend is exactly upstream's "the host
    /// cannot do this" case and must report `false` rather than claim an insert that went nowhere.
    ///
    /// `is_paste = false` is upstream's `setEditorText` (REPLACE) rather than `pasteEditorText`
    /// (`cyrup-ext/src/host/services.rs:247-250`) — the concatenation is done here, not by the host.
    fn insert_into_editor(&self, ctx: &HostCtx, text: &str) -> bool {
        if !ctx.has_ui {
            return false;
        }
        let Some(services) = self.state.host_services() else {
            return false;
        };
        let existing = services.editor_text();
        let next = if existing.trim().is_empty() {
            text.to_string()
        } else {
            format!("{}\n\n{text}", existing.trim_end())
        };
        services.set_editor_text(&next, false);
        true
    }

    /// The `/intercom-id` command body — pi `insertIntercomId(ctx)`
    /// (`v0.9.2 index.ts:2270-2289`, 20 lines).
    ///
    /// Upstream connects through `ensureConnected("tool")` (`:2276`, NOT the `"overlay"` reason
    /// `/intercom` uses) because the snippet needs this session's broker-assigned id and that id only
    /// exists once registered; a connect failure notifies `Intercom unavailable: …` and returns
    /// (`:2277-2280`). On success it formats the snippet, inserts it, and notifies either
    /// `Inserted intercom contact target: <id>` (`:2285`) or — when there is no editor to insert
    /// into — `Intercom contact target: <id>` (`:2288`), so a headless/RPC user still gets the id.
    ///
    /// cyrup's degrade (the port doc §4.3, the same one `/intercom` already takes): a command's
    /// RETURN STRING is this crate's user-visible command surface, so upstream's `notifyIfLive` toast
    /// becomes the returned text. Both upstream INFO messages are preserved verbatim, and which one
    /// comes back is exactly upstream's insert-succeeded/insert-failed branch.
    ///
    /// The connect-failure path is the exception, and it follows the `Ok(None)` convention on
    /// [`NativeExtension::execute_command`]: the session surfaces a returned string at
    /// `NotifyKind::Info`, but upstream raises this one at `"error"` (`v0.9.2 index.ts:2279`). A
    /// handler needing a non-Info level notifies itself and returns nothing, so the level survives.
    /// Returning the text here instead would show a connect FAILURE as an ordinary info toast.
    async fn run_intercom_id_command(&self, ctx: &HostCtx) -> Option<String> {
        let client = match connect::ensure_connected(&self.state, connect::ConnectReason::Tool).await {
            Ok(client) => client,
            Err(e) => {
                let message = format!("Intercom unavailable: {e}");
                match self.state.host_services() {
                    Some(services) => services.notify(&message, cyrup_ext::NotifyKind::Error),
                    // No live backend (headless with no effect sink): fall back to the return
                    // channel so the text is not lost entirely.
                    None => return Some(message),
                }
                return None;
            }
        };
        // `const sessionId = contactClient.sessionId; if (!sessionId ...) return;` (`:2281-2282`) —
        // upstream returns SILENTLY here (no notify), so this yields no command output either.
        let session_id = client.session_id()?;
        let snippet = format_intercom_contact_snippet(&session_id);
        if self.insert_into_editor(ctx, &snippet) {
            return Some(format!("Inserted intercom contact target: {session_id}"));
        }
        Some(format!("Intercom contact target: {session_id}"))
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
        // `duplicates = duplicateSessionNames(allSessions)` (`v0.10.1 index.ts:2393`) — computed
        // over EVERY session including the current one, before the self-filter below, so a peer
        // sharing this session's own name is still labelled with its id suffix.
        let duplicates = crate::identity::duplicate_session_names(
            sessions.iter().map(|s| s.name.as_deref()),
        );
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
                // pi hands the ComposeOverlay `targetLabel`, the SAME `formatSessionLabel` value
                // the confirmation below uses (`v0.10.1 index.ts:2415-2419`) — not a bare name.
                let label = crate::identity::format_session_label(
                    session.name.as_deref(),
                    &session.id,
                    &duplicates,
                );
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
        let sent = compose_send(client, &target_id, &message).await?;
        // `pi.appendEntry("intercom_sent", { to, message: { text }, messageId, timestamp })` on the
        // compose-overlay result path (`index.ts:1878-1884`). Without this the `/intercom <target>
        // <message>` leg was the ONLY send in the crate that left no trace in the transcript — the
        // `intercom` tool's `send`/`ask`/`reply` arms all append (`tools/intercom.rs`), so a session
        // driven from the slash command had an audit log that silently omitted its own outbound
        // messages (and the `intercom_sent` renderer had nothing to render for them). The §4.3
        // rendering carve-out degrades pi's interactive OVERLAY to text; it does not excuse dropping
        // the persistence half.
        //
        // `to` is pi's `selectedSession.name || selectedSession.id` — the resolved peer's label, not
        // the caller-supplied token (JS `||`, so a blank name falls through to the id).
        let selected = others.iter().find(|s| s.id == target_id).cloned();
        if let Some(services) = self.state.host_services() {
            let to = selected
                .as_ref()
                .map(|s| {
                    s.name
                        .clone()
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| s.id.clone())
                })
                .unwrap_or_else(|| target_id.clone());
            if let Err(e) = services.append_entry(
                "intercom_sent",
                &serde_json::json!({
                    "to": to,
                    "message": { "text": message },
                    "messageId": sent.id,
                    "timestamp": now_ms(),
                }),
            ) {
                tracing::warn!(error = %e, kind = "intercom_sent", "intercom: failed to append audit entry");
            }
        }
        // ICOM-013's residual: `notifyIfLive(ctx, \`Message sent to ${targetLabel}\`, "info", …)`
        // (`v0.10.1 index.ts:2429`). Two divergences, and they had to be fixed together — cyrup
        // printed a trailing period upstream does not have, AND echoed the caller's raw token where
        // pi names the RESOLVED peer through `formatSessionLabel`. Fixing only the period would
        // still have told a human who typed a prefix or an id which prefix they typed, not which
        // session actually received the message.
        let target_label = selected.as_ref().map_or_else(
            || target.clone(),
            |s| crate::identity::format_session_label(s.name.as_deref(), &s.id, &duplicates),
        );
        Ok(format!("Message sent to {target_label}"))
    }
}

#[async_trait]
impl NativeExtension for IntercomExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Ambient (SEAM-071/SEAM-074): cyrup compiles in what pi *installs*. Upstream pi-intercom is an
    /// ordinary installed package living in the PATH tier that `noExtensions` collapses to the
    /// explicit `-e` paths (`resource-loader.ts:451-453` @v0.83.0), so `--no-extensions` must drop it
    /// here too. Declared on the type rather than by an id list in cyrup-session-svc, because only a
    /// built-in knows which of pi's two tiers it stands in for — an id list also catches a test's
    /// hand-injected double that merely shares the name, which is pi's INLINE tier and is never
    /// gated (`loadFinalExtensionSet` calls `loadExtensionFactories` unconditionally, `:579-581`,
    /// over `main.ts:523`).
    fn is_ambient(&self) -> bool {
        true
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        // `intercom` is always registered; `contact_supervisor` only for a subagent child with
        // orchestrator metadata (index.ts:1162-1163,1425).
        api.register_tool(Arc::new(IntercomTool::new(self.state.clone())));
        // `v0.10.1 index.ts:1505-1507`:
        //   `if (childOrchestratorMetadata && !nativeSupervisorChannelAvailable) { pi.registerTool(…) }`
        // A child launched through the NATIVE supervisor channel must not also be handed the legacy
        // broker-routed tool: the model picks one, so the same decision can be requested through two
        // mechanisms while the parent polls only one of them.
        if let Some(metadata) = &self.metadata
            && !self.native_supervisor_channel
        {
            api.register_tool(Arc::new(ContactSupervisorTool::new(self.state.clone(), metadata.clone())));
        }
        // ICOM-028: claim the durable inbound-message entry so [`Self::render_entry`] is reached.
        // Without this registration the TUI's `has_entry_renderer` check short-circuits
        // (`cyrup-tui/src/app.rs:5845`) and the card `surface_incoming_message` pre-renders is
        // written to the session and then drawn by nothing — the human sees only a grey
        // `entry appended → intercom_message` status line. This surface is cyrup's, not pi's; see
        // [`Self::render_entry`] for why upstream has no counterpart.
        api.register_entry_renderer(crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE);
        // `pi.registerMessageRenderer("intercom_message", …)` (`index.ts:1816-1820`). The injected
        // custom message is upstream's ONE surface for an inbound message; `render_live` answers for
        // it with a component the TUI re-renders per frame at the live width, theme and expansion.
        api.register_message_renderer(crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE);
        // The `/intercom` overlay command (pi `registerCommand("intercom", …)`, index.ts:1877). cyrup
        // has no `register_shortcut`, so the `alt+m` binding degrades to this command (the port doc
        // §4.3); `execute_command` renders the session picker + drives the compose send.
        api.register_command(
            INTERCOM_COMMAND,
            CommandDescriptor { description: "Open the session intercom picker / send a message".to_string(), completions: Vec::new() },
        );
        // `/intercom-id` (`v0.9.2 index.ts:2365-2368`). Description is upstream's verbatim
        // ("Insert a stable pi-intercom handoff snippet for this session into the editor", `:2366`)
        // with the same `pi-intercom` → `cyrup-intercom` rebrand the snippet itself takes.
        api.register_command(
            INTERCOM_ID_COMMAND,
            CommandDescriptor {
                description: "Insert a stable cyrup-intercom handoff snippet for this session into the editor"
                    .to_string(),
                completions: Vec::new(),
            },
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
            // `pi.on("model_select")` (`v0.10.1 index.ts:1471-1481`) → presence carrying the new
            // model, so `intercom{list}` shows which worker is on which model.
            EventKind::ModelSelect,
            // ICOM-004 — the bundled `skills/pi-intercom/SKILL.md`. pi declares it statically in
            // `package.json`'s `pi` block (`"skills": ["./skills"]`, `package.json:26-28`) and its
            // resource discovery loads it on install; cyrup's discovery ASKS each loaded extension,
            // so the declaration is this subscription plus the answer in [`Self::on_event`].
            EventKind::ResourcesDiscover,
        ]);
        // ICOM-056 / `v0.12.0 index.ts:1687,1716`: the two inbound bus topics. cyrup-intercom is the
        // first native in the workspace to use `pi.events` at all; deliveries land in
        // [`Self::on_bus_event`].
        api.subscribe_bus(crate::outbox::INTERCOM_EXTENSION_REGISTER_EVENT);
        api.subscribe_bus(crate::outbox::INTERCOM_OUTBOX_REQUEST_EVENT);
        // `pi.events.emit(INTERCOM_EXTENSION_REGISTRY_READY_EVENT, { version: 1 })`
        // (`v0.12.0 index.ts:1700`) — UNCONDITIONAL, and immediately after the listeners so no
        // extension can ever observe "ready" before the request topic is live. This is the handshake
        // an extension waits on before emitting its first outbox request; without it the outbox is
        // listening to a bus nobody knows is there. `set_host_services` runs BEFORE `init`, so the
        // backend is already bound here.
        if let Some(services) = self.state.host_services() {
            services.emit_event(
                crate::outbox::INTERCOM_EXTENSION_REGISTRY_READY_EVENT,
                &serde_json::json!({ "version": 1 }),
            );
        }
        Ok(())
    }

    /// The `pi.events` listeners (`v0.12.0 index.ts:1687-1698,1716`). An `Err` here is contained by
    /// the host and reported on the `onError` channel, matching pi's per-listener `catch`.
    async fn on_bus_event(
        &self,
        topic: &str,
        payload: &serde_json::Value,
        _ctx: &HostCtx,
    ) -> Result<(), ExtError> {
        match topic {
            // `index.ts:1716`. This NEVER blocks the fan-out: the synchronous prelude (parse,
            // dedupe, track) settles inline so `invalid_request`/`duplicate_request` keep upstream's
            // ordering against the emit, and the delivery leg is spawned.
            crate::outbox::INTERCOM_OUTBOX_REQUEST_EVENT => {
                crate::outbox::handle_outbox_request(self.state.clone(), payload.clone());
            }
            // `index.ts:1687-1698`: shape-check, then register. Front door only — the channel
            // effects behind it are ICOM-016 and are deliberately not stubbed.
            crate::outbox::INTERCOM_EXTENSION_REGISTER_EVENT => {
                crate::outbox::handle_extension_register(&self.state, payload);
            }
            _ => {}
        }
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

    /// Dispatch this extension's two commands (command-tier).
    ///
    /// - `/intercom` — no args → render the session picker; `<target> <message…>` → resolve the
    ///   target and send it over the broker (the port doc §4.3 degrade of pi's interactive overlay).
    /// - `/intercom-id` — insert this session's handoff snippet into the editor
    ///   ([`Self::run_intercom_id_command`], pi `v0.9.2 index.ts:2270-2289`).
    async fn execute_command(&self, name: &str, args: &str, ctx: &HostCtx) -> Result<Option<String>, ExtError> {
        ctx.require_command_tier()?;
        if name == INTERCOM_ID_COMMAND {
            return Ok(self.run_intercom_id_command(ctx).await);
        }
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
                // `startNamePoll()` (`v0.10.1 index.ts:1276`, inside `startSessionRuntime`): the
                // third name-sync point. Cancelled in the `SessionShutdown` arm below.
                self.state.start_name_poll();
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
                // `clearNamePollTimer()` (`v0.10.1 index.ts:1407`).
                self.state.stop_name_poll();
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
                // `agentRunning = false; activeTools.clear()` (`v0.10.1 index.ts:1408-1409`).
                self.state.set_agent_running(false);
                HookOutcome::Noop
            }
            HostEvent::AgentStart => {
                // `agentRunning = true; activeTools.clear(); syncPresenceStatus()`
                // (`v0.10.1 index.ts:1429-1431`).
                self.state.set_agent_running(true);
                self.sync_presence_status();
                HookOutcome::Noop
            }
            HostEvent::AgentEnd { .. } => {
                // `agentRunning = false; activeTools.clear(); syncPresenceStatus()`
                // (`v0.10.1 index.ts:1451-1453`).
                self.state.set_agent_running(false);
                self.sync_presence_status();
                // NO `scheduleInboundFlush(0)` here: v0.9.3 (`25ffb96`) deleted both the
                // `agent_end` and `turn_end` flush calls along with the queue they drained
                // (`v0.10.1 index.ts:1447-1454`, `:1416-1424` — neither handler mentions inbound
                // delivery any more). A busy inbound message is steered onto the live run when it
                // ARRIVES, so there is nothing left to drain at the end of one.
                HookOutcome::Noop
            }
            HostEvent::ToolExecStart { call_id, name, .. } => {
                // `activeTools.set(event.toolCallId, event.toolName); syncPresenceStatus()`
                // (`v0.10.1 index.ts:1437-1438`) — keyed by CALL ID, so overlapping calls nest.
                self.state.tool_started(call_id.clone(), name.clone());
                self.sync_presence_status();
                HookOutcome::Noop
            }
            HostEvent::ToolExecEnd { call_id, .. } => {
                // `activeTools.delete(event.toolCallId); syncPresenceStatus()`
                // (`v0.10.1 index.ts:1444-1445`).
                self.state.tool_ended(call_id);
                self.sync_presence_status();
                HookOutcome::Noop
            }
            HostEvent::ModelSelect { model, .. } => {
                // `pi.on("model_select")` (`v0.10.1 index.ts:1471-1481`): presence carries the new
                // model alongside the identity and the derived status. Without it every peer's
                // `intercom{list}` shows the model this session registered with forever, so a
                // supervisor cannot tell which worker is on which model.
                if let Some(client) = self.state.client() {
                    let model_id = model
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let identity = connect::presence_identity(&self.state, self.metadata.as_ref());
                    let ctx_usage = self.state.current_context_usage();
                    client.update_presence_full(
                        identity.name.clone(),
                        identity.name.as_ref().map(|_| identity.runtime_fallback_alias),
                        Some(self.state.current_status()),
                        model_id,
                        ctx_usage.pct,
                        ctx_usage.tokens,
                        ctx_usage.window,
                    );
                }
                HookOutcome::Noop
            }
            HostEvent::TurnStart { .. } => {
                // `pi.on("turn_start")` (`v0.10.1 index.ts:1459-1469`) calls `syncPresenceIdentity`
                // BEFORE `replyTracker.beginTurn()` — one of upstream's three name-sync points, and
                // the cheapest: a session renamed mid-run stops advertising its startup label at the
                // very next turn instead of forever.
                self.sync_presence_identity();
                // `replyTracker.beginTurn()`: prune expired pending asks, then adopt the oldest
                // queued turn context (queued by `trigger_turn_over_inbound` right before this turn
                // started) as `current_turn_context`.
                self.state.tracker.lock().unwrap_or_else(|e| e.into_inner()).begin_turn(now_ms());
                HookOutcome::Noop
            }
            HostEvent::TurnEnd { .. } => {
                // `pi.on("turn_end") -> replyTracker.endTurn()` (`v0.10.1 index.ts:1416-1424`).
                self.state.tracker.lock().unwrap_or_else(|e| e.into_inner()).end_turn();
                HookOutcome::Noop
            }
            // ICOM-004 — hand cyrup's resource discovery the bundled skill
            // (`resources/skills/pi-intercom/SKILL.md`, the port of pi's
            // `skills/pi-intercom/SKILL.md` @ v0.10.1). pi has no event for this: it declares the
            // directory in `package.json` (`"pi": { "skills": ["./skills"] }`, `package.json:26-28`)
            // and pi's installer walks it. cyrup's host ASKS, so the same declaration is made here,
            // in exactly the shape `cyrup-ext-subagents` already answers with
            // (`extension.rs:11014-11031`) — `Noop` when nothing ships, so a relocated install with
            // no resources root is silent rather than advertising an empty list.
            HostEvent::ResourcesDiscover { .. } => {
                let skill_paths: Vec<String> = crate::resources::bundled_skill_files()
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                if skill_paths.is_empty() {
                    return HookOutcome::Noop;
                }
                HookOutcome::Handled(cyrup_ext::HandledValue(serde_json::json!({
                    "skillPaths": skill_paths,
                })))
            }
            _ => HookOutcome::Noop,
        }
    }

    /// `renderCall` for both tools (`v0.10.1 index.ts:2298-2315` and `:1743-1756`). Until this
    /// landed, neither intercom tool registered a renderer at all, so every `intercom` /
    /// `contact_supervisor` row in the transcript fell back to the host's generic tool rendering —
    /// upstream draws an action-coloured header with the target and a message preview.
    ///
    /// See [`crate::tools::render`] for the three upstream renderer inputs this seam does not carry
    /// (`theme`, `isPartial`, `context`) and which branches are therefore unreachable.
    fn render_call(&self, key: &str, call: &serde_json::Value) -> Option<serde_json::Value> {
        let text = match key {
            "intercom" => crate::tools::render::render_intercom_call(call),
            "contact_supervisor" => crate::tools::render::render_contact_supervisor_call(call),
            _ => return None,
        };
        Some(serde_json::Value::String(text))
    }

    /// `renderResult` for both tools (`v0.10.1 index.ts:2316-2331` and `:1757-1773`).
    fn render_result(&self, key: &str, result: &serde_json::Value) -> Option<serde_json::Value> {
        let text = match key {
            "intercom" => crate::tools::render::render_intercom_result(result),
            "contact_supervisor" => crate::tools::render::render_contact_supervisor_result(result),
            _ => return None,
        };
        Some(serde_json::Value::String(text))
    }

    /// ICOM-028 — draw the durable `intercom_message` entry [`crate::inbound::surface_incoming_message`]
    /// writes, instead of letting the TUI fall through to `push_status("entry appended → …")`.
    ///
    /// **This surface has no upstream analogue and is not a port.** `pi-intercom` registers no entry
    /// renderer at any tag — its one displayed custom MESSAGE (`v0.10.1 index.ts:656`, `display:
    /// true`) is simultaneously the model's context and the human's card, drawn by
    /// `registerMessageRenderer("intercom_message", …)`. cyrup splits the two (the port doc
    /// §4.2/§7.2), so the durable half needs a renderer of its own or the pre-rendered card it
    /// carries is written and never drawn. This is ICOM-028's option (a): option (b) — delete the
    /// split — depends on ICOM-024/ICOM-029, and `HostServices::inject_message` still carries no
    /// `details` for a message renderer to read, so it is not reachable from this crate.
    ///
    /// `entry` is the SERIALIZED session entry (`KnownEntry::Custom`, `cyrup-session/src/entry.rs:125-131`,
    /// `#[serde(tag = "type", rename_all_fields = "camelCase")]`), so the payload
    /// `surface_incoming_message` wrote is under `data` — not at the top level.
    ///
    /// `pi.registerMessageRenderer("intercom_message", (message, options, theme) => …)`
    /// (`index.ts:1816-1820`). `payload` is the serialized `AgentMessage::Custom`
    /// (`{role, kind, payload, details, timestamp}`), so the entry rides `details`.
    ///
    /// Also answers for the durable ENTRY surface, whose payload nests the same fields under `data`.
    ///
    /// `None` is upstream's `if (!details) return undefined`: a v0.9.2 peer, or a payload written
    /// before the injection seam carried `details`, falls through to [`Self::render_entry`]'s
    /// pre-rendered `card` / markdown `content` rather than drawing an empty box.
    fn render_live(
        &self,
        key: &str,
        payload: &serde_json::Value,
    ) -> Option<std::sync::Arc<dyn cyrup_ext::RenderedComponent>> {
        if key != crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE {
            return None;
        }
        let details = payload.get("details").or_else(|| payload.get("data"))?;
        let card = crate::ui::InlineMessage::from_details(details)?;
        Some(std::sync::Arc::new(crate::ui::InlineMessageComponent::new(card)))
    }

    /// The card is emitted at the width it was rendered at (`SURFACE_CARD_WIDTH`). That degrade is
    /// now the FALLBACK path only: a live inbound delivery is drawn by [`Self::render_live`] at the
    /// real terminal width. This seam still serves the durable entry a busy non-interactive session
    /// writes, and any payload carrying no `details` (a v0.9.2 peer, or one written before the
    /// injection seam carried them).
    fn render_entry(
        &self,
        custom_type: &str,
        entry: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        if custom_type != crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE {
            return None;
        }
        let data = entry.get("data")?;
        // The pre-rendered card is an array of lines. Fall back to the markdown `content` (the same
        // body the model was injected with) if a payload from an older writer has no `card`, and
        // return `None` — upstream's `Component | undefined`, i.e. "draw nothing" — rather than an
        // empty box if neither is present.
        let card: Option<String> = data
            .get("card")
            .and_then(serde_json::Value::as_array)
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|s| !s.is_empty());
        let text = card.or_else(|| {
            data.get("content")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })?;
        Some(serde_json::Value::String(text))
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
    // `v0.10.1 config.ts:153-155`: a malformed config is a hard error naming the path, not a silent
    // `inboundTrigger: "never"`. This function already returns `Result<_, String>` for the
    // analogous `ask_timeout_ms()` throw, so the precedent for propagating is in place.
    let config = load_config(&intercom_dir)?;
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

    /// The `TempDir` is returned, not dropped, so the extension's agent/cwd paths stay valid for the
    /// life of the test — an extension holding paths into an already-removed directory is a fixture
    /// that only happens to work because these renderers touch no filesystem.
    fn test_extension() -> (tempfile::TempDir, IntercomExtension) {
        let dir = tempfile::tempdir().unwrap();
        let ext = IntercomExtension::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            IntercomConfig::default(),
            None,
        )
        .expect("a default config builds an extension");
        (dir, ext)
    }

    /// ICOM-004 — the bundled operational skill is DECLARED to cyrup's resource discovery.
    ///
    /// Both halves are asserted because either alone is inert: without the subscription the host
    /// never dispatches `ResourcesDiscover` to this extension (`Subscriptions::contains` gates the
    /// dispatch), and without the `on_event` arm the subscription answers `Noop` and the shipped
    /// `SKILL.md` is a file nobody reads. Pre-fix BOTH were absent — `init` subscribed 11 kinds,
    /// none of them `ResourcesDiscover`, and `on_event`'s catch-all returned `Noop` — so this test
    /// fails on the first assertion.
    #[tokio::test]
    async fn the_bundled_skill_is_declared_to_resource_discovery() {
        let (dir, ext) = test_extension();
        let mut api = InitApi::new();
        ext.init(&mut api).await.expect("init");
        assert!(
            api.subscriptions().contains(EventKind::ResourcesDiscover),
            "the extension must be dispatched `ResourcesDiscover` to answer it at all"
        );

        let ctx = HostCtx::event(cyrup_ext::ExtMode::Print, false, dir.path().to_path_buf());
        let ev = HostEvent::ResourcesDiscover {
            cwd: dir.path().display().to_string(),
            reason: "startup".to_string(),
        };
        let answered = match ext.on_event(&ev, &ctx).await {
            HookOutcome::Handled(cyrup_ext::HandledValue(v)) => Some(v),
            _ => None,
        };
        let value = answered.expect("discovery must be answered with the bundled skill paths");
        let paths = value["skillPaths"].as_array().expect("skillPaths is a list");
        assert_eq!(paths.len(), 1, "exactly the one bundled skill: {paths:?}");
        let path = std::path::PathBuf::from(paths[0].as_str().expect("a path string"));
        assert!(path.is_file(), "the declared path must exist on disk: {path:?}");
        assert!(
            path.ends_with("skills/pi-intercom/SKILL.md"),
            "the declared path is the ported skill: {path:?}"
        );
    }

    /// ICOM-028 — the durable `intercom_message` entry is DRAWN, not swallowed. Reads the payload
    /// out of `data`, which is where `KnownEntry::Custom`'s serialization puts it — a renderer that
    /// looked at the top level would silently return `None` on every real entry.
    #[test]
    fn intercom_message_entry_renders_the_prerendered_card() {
        let (_dir, ext) = test_extension();
        let entry = serde_json::json!({
            "type": "custom",
            "customType": crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE,
            "id": "e1",
            "data": {
                "content": "**From reviewer** (/repo)\n\nlooks good",
                "card": ["┌──────┐", "│ hi   │", "└──────┘"],
            },
        });
        let drawn = ext
            .render_entry(crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE, &entry)
            .expect("the registered type must render");
        assert_eq!(drawn.as_str().unwrap(), "┌──────┐\n│ hi   │\n└──────┘");

        // A payload with no `card` degrades to the markdown body rather than drawing nothing.
        let no_card = serde_json::json!({
            "customType": crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE,
            "data": { "content": "body only" },
        });
        assert_eq!(
            ext.render_entry(crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE, &no_card)
                .unwrap()
                .as_str()
                .unwrap(),
            "body only"
        );

        // Neither present is upstream's `Component | undefined` — draw nothing, not an empty box.
        assert!(
            ext.render_entry(
                crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE,
                &serde_json::json!({ "data": {} })
            )
            .is_none()
        );
        // And another extension's entry type is never claimed.
        assert!(ext.render_entry("subagent_run", &entry).is_none());
    }

    /// The renderer is UNREACHABLE unless `init` also claims the type — `cyrup-tui/src/app.rs:5845`
    /// short-circuits on `has_entry_renderer` before ever calling the extension. Asserting the
    /// registration is what stops this from being ICOM-028 all over again with a live renderer
    /// nobody calls (README blind spot: a test asserting an absence must first assert the presence).
    #[tokio::test]
    async fn init_claims_the_intercom_message_entry_type() {
        let host = cyrup_ext::ExtensionHost::new(cyrup_ext::HostConfig::default());
        let (_dir, ext) = test_extension();
        host.load_native(Arc::new(ext)).await.expect("the native loads");
        assert!(
            host.has_entry_renderer(crate::inbound::INBOUND_MESSAGE_CUSTOM_TYPE),
            "init must claim the entry type, or `cyrup-tui/src/app.rs:5845` never calls the renderer"
        );
        // The claim is type-scoped, not a blanket one.
        assert!(!host.has_entry_renderer("subagent_run"));
    }

    /// The child-orchestrator metadata the `v0.10.1 index.ts:1505-1507` gate is keyed on. Built in
    /// the test rather than read from the environment for the same reason
    /// [`IntercomExtension::with_native_supervisor_channel`] exists: a test must not mutate
    /// process-global env state.
    fn child_metadata() -> ChildOrchestratorMetadata {
        ChildOrchestratorMetadata {
            orchestrator_target: "supervisor".to_string(),
            orchestrator_session_id: None,
            run_id: "run-xyz".to_string(),
            agent: "researcher".to_string(),
            index: "0".to_string(),
            session_name: Some("subagent-chat-1".to_string()),
        }
    }

    /// `init` one extension through a real `ExtensionHost` and report the names of the tools it
    /// registered, sorted. `InitApi::into_parts` is `pub(crate)` to `cyrup-ext` and `InitApi`
    /// exposes only `subscriptions()`, so the host's active tool set is the observable side of
    /// `api.register_tool` from here — the same route
    /// [`init_claims_the_intercom_message_entry_type`] takes to observe `register_entry_renderer`.
    /// A fresh host per call because the extension id is fixed ([`EXTENSION_ID`]) and the two arms
    /// must not share a registry.
    async fn registered_tool_names(ext: IntercomExtension) -> Vec<String> {
        let host = cyrup_ext::ExtensionHost::new(cyrup_ext::HostConfig::default());
        host.load_native(Arc::new(ext)).await.expect("the native loads");
        let mut names: Vec<String> = host
            .active_tools(&[])
            .expect("the active tool set materializes")
            .iter()
            .map(|t| cyrup_core::Tool::name(t.as_ref()).to_string())
            .collect();
        names.sort();
        names
    }

    /// `v0.10.1 index.ts:1505-1507` — `if (childOrchestratorMetadata && !nativeSupervisorChannel
    /// Available) { pi.registerTool(…) }`. BOTH arms are pinned here, and both are asserted against
    /// the same fixture, because an absence assertion on its own passes just as well when the tool
    /// failed to register for an unrelated reason. What the `true` arm guards is silent rather than
    /// loud: a child handed the native channel AND the legacy broker-routed `contact_supervisor`
    /// can request the same decision through two mechanisms while the parent polls only one of
    /// them, which is a hang, not an error.
    ///
    /// Driven through [`IntercomExtension::with_native_supervisor_channel`] — the seam written for
    /// exactly this and, until now, never called — so neither arm touches
    /// `CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR` or any other process-global env state.
    #[tokio::test]
    async fn native_supervisor_channel_gates_the_legacy_contact_supervisor_tool() {
        let dir = tempfile::tempdir().unwrap();
        let child_extension = || {
            IntercomExtension::new(
                dir.path().to_path_buf(),
                dir.path().to_path_buf(),
                IntercomConfig::default(),
                Some(child_metadata()),
            )
            .expect("a default config builds an extension")
        };

        // No native channel: the child gets the legacy broker-routed tool (`index.ts:1162-1163`).
        let legacy = registered_tool_names(child_extension().with_native_supervisor_channel(false)).await;
        assert!(
            legacy.iter().any(|n| n == "contact_supervisor"),
            "a child WITHOUT the native channel must be handed the legacy tool: {legacy:?}"
        );
        assert!(legacy.iter().any(|n| n == "intercom"), "`intercom` is registered always: {legacy:?}");

        // Native channel available: the SAME extension minus that one tool — `intercom` still
        // registers, so the absence below is the gate firing and not a failed `init`.
        let native = registered_tool_names(child_extension().with_native_supervisor_channel(true)).await;
        assert!(
            !native.iter().any(|n| n == "contact_supervisor"),
            "a child ON the native channel must NOT also get the broker-routed tool: {native:?}"
        );
        assert!(native.iter().any(|n| n == "intercom"), "`intercom` is registered always: {native:?}");
    }

    /// The two tool renderers are wired to the names the tools actually register under — a renderer
    /// keyed on the wrong string is a silent no-op, since the host simply falls through.
    #[test]
    fn both_tool_renderers_are_keyed_on_the_registered_tool_names() {
        let (_dir, ext) = test_extension();
        let call = serde_json::json!({ "action": "send", "to": "reviewer", "message": "hi" });
        assert_eq!(
            ext.render_call("intercom", &call).unwrap().as_str().unwrap(),
            "intercom send → reviewer\n  hi"
        );
        assert!(ext.render_call("contact_supervisor", &serde_json::json!({})).is_some());
        assert!(ext.render_call("not_our_tool", &call).is_none());

        let result = serde_json::json!({
            "content": [{ "type": "text", "text": "Message sent to reviewer" }],
            "details": { "messageId": "0192f3c1-9a10-7000", "delivered": true },
        });
        assert_eq!(
            ext.render_result("intercom", &result).unwrap().as_str().unwrap(),
            "✓ Message sent to reviewer (0192f3c1)"
        );
        assert!(ext.render_result("contact_supervisor", &result).is_some());
        assert!(ext.render_result("not_our_tool", &result).is_none());
    }

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
                runtime_fallback_alias: None,
                cwd: "/w".to_string(),
                model: "m".to_string(),
                pid: 1u32.into(),
                started_at: 0u64.into(),
                last_activity: 0u64.into(),
                status: None,
                peer_uid: None,
                trusted_local: None,
                context_pct: None,
                context_tokens: None,
                context_window: None,
                extra: Default::default(),
            },
            message: crate::transport::protocol::Message {
                id: "q-trigger".to_string(),
                timestamp: 0u64.into(),
                reply_to: None,
                expects_reply: Some(true),
                content: crate::transport::protocol::MessageContent {
                    text: "the message that triggered this turn".to_string(),
                    attachments: None,
                    ..Default::default()
                },
                ..Default::default()
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
                    runtime_fallback_alias: None,
                    cwd: "/w".to_string(),
                    model: "m".to_string(),
                    pid: 2u32.into(),
                    started_at: 0u64.into(),
                    last_activity: 0u64.into(),
                    status: None,
                    peer_uid: None,
                    trusted_local: None,
                    context_pct: None,
                    context_tokens: None,
                    context_window: None,
                    extra: Default::default(),
                },
                crate::transport::protocol::Message {
                    id: "q-other".to_string(),
                    timestamp: 0u64.into(),
                    reply_to: None,
                    expects_reply: Some(true),
                    content: crate::transport::protocol::MessageContent {
                        text: "unrelated older ask".to_string(),
                        attachments: None,
                        ..Default::default()
                    },
                    ..Default::default()
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
