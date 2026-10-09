//! The [`IntercomExtension`] `NativeExtension` facade + the binary-wiring entry point
//! [`intercom_extension_for_env`] (a port of `pi-intercom/index.ts:430` `piIntercomExtension(pi)`).
//!
//! WIRING (all reachable in this phase, no dead primitives):
//! - `init` registers the `intercom` tool always, and `contact_supervisor` ONLY when child-
//!   orchestrator metadata is present (`index.ts:1162-1163`); it subscribes the lifecycle events.
//! - `init` also registers the four slash commands — `/intercom`, `/intercom-id`, `/alias` and
//!   `/handover` (`v0.16.1 index.ts:3223-3241`), dispatched by
//!   [`IntercomExtension::execute_command`] — and the `alt+m` shortcut (`:3243-3246`), dispatched by
//!   [`IntercomExtension::execute_shortcut`]. Bare `/intercom`, `alt+m` and `/handover` drive LIVE
//!   overlays through `HostServices::open_overlay` (`crate::ui`).
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
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cyrup_core::ExtensionId;
use cyrup_ext::registry::CommandDescriptor;
use cyrup_ext::{
    EventKind, ExtError, ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension, NotifyKind,
};
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
use crate::tools::intercom::{HandoverRoute, IntercomTool, build_handover_text, deliver_handover};
use crate::transport::client::IntercomClient;
use crate::transport::protocol::{SessionInfo, now_ms};
use crate::ui::compose::{COMPOSE_MAX_WIDTH, ComposeOverlayHost};
use crate::ui::handover_loader::{HandoverGeneration, HandoverLoader, HandoverTextFuture};
use crate::ui::handover_picker::{
    HandoverPicker, HandoverPickerHost, HandoverPickerTarget, HerdrRemoteLister,
};
use crate::ui::session_list::{SessionListOverlayHost, SessionListSelection};
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
/// The `/alias` session-alias command (ICOM-061; `v0.14.0 index.ts:2911-2914` —
/// `pi.registerCommand("alias", { description, handler })`, added by `90e6ad4`, v0.13.0 #126).
pub const ALIAS_COMMAND: &str = "alias";
/// The `/handover` command (ICOM-078; `v0.16.1 index.ts:3238-3241` —
/// `pi.registerCommand("handover", { description, handler: runHandoverCommand })`).
pub const HANDOVER_COMMAND: &str = "handover";
/// The `/intercom` overlay's keyboard shortcut (`v0.16.1 index.ts:3243-3246` —
/// `pi.registerShortcut("alt+m", { description: "Open session intercom", handler:
/// openIntercomOverlay })`).
pub const INTERCOM_SHORTCUT: &str = "alt+m";
/// `/handover`'s refusal outside the interactive terminal UI (`v0.16.1 index.ts:3027`), verbatim.
pub const HANDOVER_REQUIRES_TUI: &str = "/handover requires the interactive terminal UI; use the intercom tool's handover action instead.";
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
    format!(
        r#"Use cyrup-intercom: intercom({{ action: "send", to: "{session_id}", message: "..." }})"#
    )
}

/// How a live overlay run ended.
enum OverlayRun {
    /// The overlay ran (or a failure before it was reported); the command's output, if any.
    Ran(Option<String>),
    /// No interactive surface took the overlay — fall back to the text rendering.
    NoSurface,
}

/// `/^(\/|\.\.?\/|~(\/|$))/.test(target)` (`v0.16.1 index.ts:3038`): `/`, `./`, `../`, `~/` or a
/// bare `~` makes a `/handover` target a project path.
fn is_project_path(target: &str) -> bool {
    target.starts_with('/')
        || target.starts_with("./")
        || target.starts_with("../")
        || target.starts_with("~/")
        || target == "~"
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
    /// Whether intercom is genuinely INSTALLED (`CYRUP_INTERCOM`, or an
    /// `<agent_dir>/intercom/config.json`), as opposed to this extension having attached only
    /// through the child-metadata escape hatch in [`intercom_extension_for_env_concrete`], where
    /// `metadata.is_some()` short-circuits the install gate.
    ///
    /// Used together with [`Self::native_supervisor_channel`] to decide who owns the bare tool
    /// name `intercom` — see [`Self::init`] for the full argument. Probed ONCE at construction,
    /// never re-read inside `init`, exactly as `native_supervisor_channel` above.
    ///
    /// ⚠ "This extension stands down" is NOT the same claim as "the other side stands up". The
    /// subagents crate's native fallback needs BOTH `!intercom_available` AND `intercom` present in
    /// `required_child_tools`; ownership here is decided against
    /// [`Self::native_supervisor_channel`] alone, which is the term that governs whether that side
    /// can register AT ALL. [`Self::init`] spells out the one state where both stand down, and why
    /// that is intended.
    ///
    /// Both sides read the install marker at `<agent_dir>/intercom/config.json` over the SAME
    /// agent-dir ladder — `cyrup_config::paths::cyrup_agent_dir_from`, i.e. `<home>/.cyrup/agent`.
    /// That agreement is load-bearing and was once broken; see
    /// [`crate::paths::agent_dir_path_from`]'s "Why the `/agent` component is load-bearing".
    ///
    /// Scope note: this pair governs the `intercom` name only. The separate `contact_supervisor`
    /// gap — installed + native channel leaves that tool registered by neither provider — is NOT
    /// addressed here and is not made worse; closing it needs a broker-vs-native routing decision
    /// of its own.
    installed: bool,
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
        let delivery = Arc::new(IntercomDeliveryChannel::new(
            state.clone(),
            supervisor_target,
        ));
        let steer = Arc::new(IntercomSteerChannel::new(state.clone()));
        Ok(Self {
            id: ExtensionId::from(EXTENSION_ID),
            state,
            agent_dir,
            metadata,
            native_supervisor_channel: crate::identity::native_supervisor_channel_available(),
            // "This attachment owns the name" — what every caller that did not reach the
            // force-attach path means. `intercom_extension_for_env_concrete`, the ONE production
            // constructor and the only one that has already probed the install state, overrides it
            // via [`Self::with_installed`].
            installed: true,
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

    /// Override the install probe instead of reading the process environment — for
    /// [`intercom_extension_for_env_concrete`], which has already computed it, and for tests, which
    /// must not mutate process-global env state. See [`Self::installed`].
    #[must_use]
    pub fn with_installed(mut self, installed: bool) -> Self {
        self.installed = installed;
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
        let client =
            match connect::ensure_connected(&self.state, connect::ConnectReason::Tool).await {
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

    /// The `/alias` command body — pi `setIntercomAlias(args, ctx)` (`v0.14.0 index.ts:2779-2830`).
    ///
    /// `/alias <name>` renames the session (`pi.setSessionName(alias)`, `:2819`) and then pushes the
    /// new identity to the broker at once (`syncPresenceIdentity(…)`, `:2828`), because — upstream's
    /// own comment — "Pi's session_info_changed event updates the built-in UI, but it is not an
    /// ExtensionAPI event", so without the push a peer would only see the rename on the next name-poll
    /// tick. Bare `/alias` and `/alias menu` open an input dialog when there is a UI; without one,
    /// bare `/alias` reports the current alias and `/alias menu` warns (`:2786-2796`).
    ///
    /// Output follows [`Self::notify_command`]. `ui.input` is the blocking
    /// `HostServices::input` bridge, so it is driven on a blocking thread exactly as the clarify
    /// seam drives it (`seams.rs`).
    async fn run_alias_command(&self, args: &str, ctx: &HostCtx) -> Option<String> {
        // `const commandGeneration = runtimeGeneration; if (!getLiveContext(ctx, …)) return;`
        let generation = self.state.connect.generation();
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        let services = self.state.host_services();
        // `pi.getSessionName()?.trim()`, read where upstream reads it; blank is falsy.
        let current_alias = || {
            services
                .as_ref()
                .and_then(|s| s.session_name())
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
        };
        let mut alias = args.trim().to_string();
        let opens_alias_input = alias.is_empty() || alias.to_lowercase() == "menu";
        if opens_alias_input {
            if !ctx.has_ui {
                let (message, kind) = if !alias.is_empty() {
                    (
                        "The alias menu requires an interactive UI; use /alias <name>.".to_string(),
                        cyrup_ext::NotifyKind::Warning,
                    )
                } else if let Some(current) = current_alias() {
                    (
                        format!("Session alias: {current}"),
                        cyrup_ext::NotifyKind::Info,
                    )
                } else {
                    (
                        "No session alias set. Use /alias <name>.".to_string(),
                        cyrup_ext::NotifyKind::Info,
                    )
                };
                return self.notify_command(ctx, generation, message, kind);
            }
            // No bound backend has no dialog to open: the same "dismissed" outcome as the
            // `HostServices::input` default, so the command ends silently.
            let services = services.clone()?;
            let placeholder = match current_alias() {
                Some(current) => format!("Current alias: {current}"),
                None => "Enter an alias".to_string(),
            };
            let entered = tokio::task::spawn_blocking(move || {
                services.input(
                    "Set session alias",
                    Some(&placeholder),
                    &cyrup_ext::DialogOptions::default(),
                )
            })
            .await;
            let entered = match entered {
                Ok(entered) => entered,
                Err(e) => {
                    return self.notify_command(
                        ctx,
                        generation,
                        format!("Unable to set session alias: {e}"),
                        cyrup_ext::NotifyKind::Error,
                    );
                }
            };
            // `if (entered === undefined) return;` — a dismissed dialog is not an error.
            let entered = entered?;
            alias = entered.trim().to_string();
            if alias.is_empty() {
                return self.notify_command(
                    ctx,
                    generation,
                    "Session alias cannot be empty.".to_string(),
                    cyrup_ext::NotifyKind::Warning,
                );
            }
        }

        // The dialog may have outlived the runtime it was opened for.
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        // `pi.setSessionName(alias)` — always present upstream. A session with no bound backend
        // has nothing to rename, which is upstream's `catch` arm rather than a claimed success.
        let Some(services) = services else {
            return self.notify_command(
                ctx,
                generation,
                "Unable to set session alias: no live session".to_string(),
                cyrup_ext::NotifyKind::Error,
            );
        };
        services.set_session_name(&alias);
        // `pi.setSessionName` either renames or throws into the `catch` arm. cyrup's seam returns
        // nothing, and the live backend drops a rename it cannot apply right now (its manager is
        // taken with `try_lock`, so a turn holding it reads as "session busy"). Read the name back:
        // a rename that did not land is upstream's error arm, not a success that pushes the old
        // name to every peer.
        if services.session_name().as_deref().map(str::trim) != Some(alias.as_str()) {
            return self.notify_command(
                ctx,
                generation,
                "Unable to set session alias: the session did not accept the rename (it may be \
                 busy); try again"
                    .to_string(),
                cyrup_ext::NotifyKind::Error,
            );
        }
        self.sync_presence_identity();
        self.notify_command(
            ctx,
            generation,
            format!("Session alias set: {alias}"),
            cyrup_ext::NotifyKind::Info,
        )
    }

    /// pi `notifyAliasCommand(ctx, message, level, generation)` (`v0.14.0 index.ts:799-809`), which
    /// `/handover` uses too (`v0.16.1 index.ts:3027`), and — because every `/handover` and live
    /// `/intercom` path runs only with a UI — what pi's `notifyIfLive` amounts to there: nothing
    /// once the runtime has moved on; without a UI the text goes to the one channel a headless
    /// session has (upstream `console.error`, "Keep alias guidance visible without injecting a
    /// synthetic Pi message"); with one, a notification at `level`.
    ///
    /// cyrup's command RETURN STRING is both the headless channel and an Info notification (the
    /// `Ok(None)` convention on [`NativeExtension::execute_command`], as in
    /// [`Self::run_intercom_id_command`]), so a UI warning or error notifies itself at its own level
    /// and returns nothing — returning it would show it at Info.
    fn notify_command(
        &self,
        ctx: &HostCtx,
        generation: u64,
        message: String,
        kind: cyrup_ext::NotifyKind,
    ) -> Option<String> {
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        if !ctx.has_ui || kind == cyrup_ext::NotifyKind::Info {
            return Some(message);
        }
        match self.state.host_services() {
            Some(services) => {
                services.notify(&message, kind);
                None
            }
            None => Some(message),
        }
    }

    /// pi `openIntercomOverlay(ctx)` (`v0.16.1 index.ts:3149-3220`) — the LIVE session list bare
    /// `/intercom` and `alt+m` open, then either the compose box (Enter) or the handover picker
    /// (`h`, `:3191-3194`).
    ///
    /// [`OverlayRun::NoSurface`] is the one outcome upstream cannot have: `open_overlay` returned
    /// `false` because nothing interactive is attached, which is pi's `!ctx.hasUI` branch reached
    /// late. `/intercom` then renders the same list as text; the shortcut has nothing to fall back
    /// to.
    async fn open_intercom_overlay(&self, ctx: &HostCtx) -> OverlayRun {
        let generation = self.state.connect.generation();
        if !connect::is_live_at(&self.state, generation) {
            return OverlayRun::Ran(None);
        }
        let Some(services) = self.state.host_services() else {
            return OverlayRun::NoSurface;
        };
        let client =
            match connect::ensure_connected(&self.state, connect::ConnectReason::Overlay).await {
                Ok(client) => client,
                Err(e) => {
                    return OverlayRun::Ran(self.notify_command(
                        ctx,
                        generation,
                        format!("Intercom unavailable: {e}"),
                        NotifyKind::Error,
                    ));
                }
            };
        if !connect::is_live_at(&self.state, generation) {
            return OverlayRun::Ran(None);
        }
        self.sync_presence_identity();
        let all_sessions = match client.list_sessions().await {
            Ok(sessions) => sessions,
            Err(e) => {
                return OverlayRun::Ran(self.notify_command(
                    ctx,
                    generation,
                    format!("Failed to list sessions: {e}"),
                    NotifyKind::Error,
                ));
            }
        };
        if !connect::is_live_at(&self.state, generation) {
            return OverlayRun::Ran(None);
        }
        let my_id = client.session_id();
        let Some(current) = my_id
            .as_deref()
            .and_then(|id| all_sessions.iter().find(|s| s.id == id).cloned())
        else {
            return OverlayRun::Ran(self.notify_command(
                ctx,
                generation,
                "Current session is missing from intercom session list".to_string(),
                NotifyKind::Error,
            ));
        };
        let duplicates = crate::identity::duplicate_session_names(
            all_sessions.iter().map(|s| s.name.as_deref()),
        );
        let others: Vec<SessionInfo> = all_sessions
            .into_iter()
            .filter(|s| Some(s.id.as_str()) != my_id.as_deref())
            .collect();

        let picked = Arc::new(Mutex::new(None));
        let list =
            SessionListOverlayHost::new(SessionListOverlay::new(current, others), picked.clone());
        if !services.open_overlay(Box::new(list)) {
            return OverlayRun::NoSurface;
        }
        let selection = picked.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(selection) = selection else {
            return OverlayRun::Ran(None);
        };
        if !connect::is_live_at(&self.state, generation) {
            return OverlayRun::Ran(None);
        }
        let selected = match selection {
            SessionListSelection::Handover(session) => {
                return OverlayRun::Ran(
                    self.open_handover_picker(ctx, generation, Some(session.id))
                        .await,
                );
            }
            SessionListSelection::Message(session) => session,
        };

        let client =
            match connect::ensure_connected(&self.state, connect::ConnectReason::Overlay).await {
                Ok(client) => client,
                Err(e) => {
                    return OverlayRun::Ran(self.notify_command(
                        ctx,
                        generation,
                        format!("Intercom unavailable: {e}"),
                        NotifyKind::Error,
                    ));
                }
            };
        if !connect::is_live_at(&self.state, generation) {
            return OverlayRun::Ran(None);
        }
        let target_label = crate::identity::format_session_label(
            selected.name.as_deref(),
            &selected.id,
            &duplicates,
        );
        let composed = Arc::new(Mutex::new(None));
        let compose = ComposeOverlayHost::new(
            ComposeOverlay::new(selected.clone(), target_label.clone()),
            client,
            tokio::runtime::Handle::current(),
            composed.clone(),
        );
        if !services.open_overlay(Box::new(compose)) {
            return OverlayRun::Ran(None);
        }
        let result = composed.lock().unwrap_or_else(|e| e.into_inner()).take();
        // `if (result?.sent && result.messageId && result.text && getLiveContext(…))` (`:3212`).
        let Some(crate::ui::compose::ComposeResult {
            sent: true,
            message_id: Some(message_id),
            text: Some(text),
        }) = result
        else {
            return OverlayRun::Ran(None);
        };
        if !connect::is_live_at(&self.state, generation) {
            return OverlayRun::Ran(None);
        }
        // `pi.appendEntry("intercom_sent", { to: selectedSession.name || selectedSession.id, … })`.
        let to = selected
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| selected.id.clone());
        if let Err(e) = services.append_entry(
            "intercom_sent",
            &serde_json::json!({
                "to": to,
                "message": { "text": text },
                "messageId": message_id,
                "timestamp": now_ms(),
            }),
        ) {
            tracing::warn!(error = %e, kind = "intercom_sent", "intercom: failed to append audit entry");
        }
        OverlayRun::Ran(self.notify_command(
            ctx,
            generation,
            format!("Message sent to {target_label}"),
            NotifyKind::Info,
        ))
    }

    /// pi `runHandoverCommand(args, ctx)` (`v0.16.1 index.ts:3023-3041`).
    ///
    /// `/handover` alone opens the picker; `/handover <target> [next task]` hands over directly. A
    /// target that starts like a path (`/`, `./`, `../`, `~/` or a bare `~`) is a PROJECT: the
    /// session live there, or a new Herdr pane started there (`openProjectPaneIfMissing: true`).
    /// Anything else is a session name, id or id prefix, and a target with `@` is a session on
    /// another machine.
    async fn run_handover_command(&self, args: &str, ctx: &HostCtx) -> Option<String> {
        let generation = self.state.connect.generation();
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        if !ctx.has_ui || ctx.mode != ExtMode::Tui {
            return self.notify_command(
                ctx,
                generation,
                HANDOVER_REQUIRES_TUI.to_string(),
                NotifyKind::Error,
            );
        }
        let input = args.trim();
        if input.is_empty() {
            return self.open_handover_picker(ctx, generation, None).await;
        }
        // `input.split(/\s+/, 1)[0]` and `input.slice(target.length).trim() || undefined`.
        let target = input.split_whitespace().next().unwrap_or_default();
        let goal = input
            .get(target.len()..)
            .map(str::trim)
            .filter(|goal| !goal.is_empty())
            .map(str::to_string);
        let is_path = is_project_path(target);
        let route = if is_path {
            HandoverRoute {
                cwd: Some(crate::cwd::expand_home_path(target)),
                open_project_pane_if_missing: true,
                ..HandoverRoute::default()
            }
        } else {
            HandoverRoute {
                to: Some(target.to_string()),
                ..HandoverRoute::default()
            }
        };
        self.perform_handover(
            ctx,
            generation,
            route,
            goal,
            !is_path && target.contains('@'),
        )
        .await
    }

    /// pi `openHandoverPicker(ctx, generation, preselectSessionId)` (`v0.16.1 index.ts:3043-3095`).
    async fn open_handover_picker(
        &self,
        ctx: &HostCtx,
        generation: u64,
        preselect_session_id: Option<String>,
    ) -> Option<String> {
        let client =
            match connect::ensure_connected(&self.state, connect::ConnectReason::Tool).await {
                Ok(client) => client,
                Err(e) => {
                    return self.notify_command(
                        ctx,
                        generation,
                        format!("Intercom unavailable: {e}"),
                        NotifyKind::Error,
                    );
                }
            };
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        self.sync_presence_identity();
        let sessions = match client.list_sessions().await {
            Ok(sessions) => sessions,
            Err(e) => {
                return self.notify_command(
                    ctx,
                    generation,
                    format!("Failed to list sessions: {e}"),
                    NotifyKind::Error,
                );
            }
        };
        let my_id = client.session_id();
        let Some(current) = my_id
            .as_deref()
            .and_then(|id| sessions.iter().find(|s| s.id == id).cloned())
        else {
            return self.notify_command(
                ctx,
                generation,
                "Current session is missing from intercom session list".to_string(),
                NotifyKind::Error,
            );
        };
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        let Some(services) = self.state.host_services() else {
            return Some(HANDOVER_REQUIRES_TUI.to_string());
        };
        // `{ run: runCommand, herdrBin: process.env.HERDR_BIN_PATH ?? "herdr" }` (`:3064`), over the
        // session's own runner so a test can stand in for `herdr`.
        let lister = Arc::new(HerdrRemoteLister::new(
            self.state.cross_machine_runner(),
            crate::cross_machine::herdr_bin_from(|key| std::env::var(key).ok()),
        ));
        let picker = HandoverPicker::new(
            &current,
            sessions,
            lister,
            preselect_session_id.as_deref(),
            tokio::runtime::Handle::current(),
        );
        let picked = Arc::new(Mutex::new(None));
        if !services.open_overlay(Box::new(HandoverPickerHost::new(picker, picked.clone()))) {
            // Inside the TUI gate a host that takes no overlay has no interactive surface after
            // all; say so rather than end silently.
            return self.notify_command(
                ctx,
                generation,
                HANDOVER_REQUIRES_TUI.to_string(),
                NotifyKind::Error,
            );
        }
        let picked = picked.lock().unwrap_or_else(|e| e.into_inner()).take();
        // `if (!picked || !getLiveContext(ctx, generation)) return;` — Escape is silent.
        let picked = picked?;
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        let goal = picked.goal;
        match picked.target {
            HandoverPickerTarget::Local(session) => {
                let route = HandoverRoute {
                    to: Some(session.id.clone()),
                    ..HandoverRoute::default()
                };
                self.perform_handover(ctx, generation, route, goal, false)
                    .await
            }
            HandoverPickerTarget::Remote(target) => {
                let route = HandoverRoute {
                    to: Some(target),
                    ..HandoverRoute::default()
                };
                self.perform_handover(ctx, generation, route, goal, true)
                    .await
            }
            HandoverPickerTarget::Project => {
                // `(await ctx.ui.input("Project path for the new session", "~/dev/project"))?.trim()`
                // — the blocking dialog bridge, driven off the async worker as `/alias` drives it.
                let dialog = services.clone();
                let path = tokio::task::spawn_blocking(move || {
                    dialog.input(
                        "Project path for the new session",
                        Some("~/dev/project"),
                        &cyrup_ext::DialogOptions::default(),
                    )
                })
                .await
                .ok()
                .flatten()
                .map(|path| path.trim().to_string())
                .filter(|path| !path.is_empty());
                if !connect::is_live_at(&self.state, generation) {
                    return None;
                }
                let Some(path) = path else {
                    return self.notify_command(
                        ctx,
                        generation,
                        "Handover cancelled".to_string(),
                        NotifyKind::Info,
                    );
                };
                let route = HandoverRoute {
                    cwd: Some(crate::cwd::expand_home_path(&path)),
                    open_project_pane_if_missing: true,
                    ..HandoverRoute::default()
                };
                self.perform_handover(ctx, generation, route, goal, false)
                    .await
            }
        }
    }

    /// pi `performHandover(ctx, generation, request, { goal, crossMachine })`
    /// (`v0.16.1 index.ts:3097-3147`): generate under a cancellable loader, let the human edit the
    /// summary, then send it through the shared delivery. Nothing is sent unless the editor returns
    /// non-blank text; `confirmSend` still applies on top, as upstream's README says.
    async fn perform_handover(
        &self,
        ctx: &HostCtx,
        generation: u64,
        route: HandoverRoute,
        goal: Option<String>,
        cross_machine: bool,
    ) -> Option<String> {
        let client =
            match connect::ensure_connected(&self.state, connect::ConnectReason::Tool).await {
                Ok(client) => client,
                Err(e) => {
                    return self.notify_command(
                        ctx,
                        generation,
                        format!("Intercom unavailable: {e}"),
                        NotifyKind::Error,
                    );
                }
            };
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        let Some(services) = self.state.host_services() else {
            return Some(HANDOVER_REQUIRES_TUI.to_string());
        };

        // The `BorderedLoader` (`:3112-3121`): `buildHandoverText(…, loader.signal)` under the
        // loader's own token, so Escape aborts the model call.
        let cancel = cyrup_core::CancelToken::new();
        let generation_future: HandoverTextFuture = {
            let state = self.state.clone();
            let client = client.clone();
            let cancel = cancel.clone();
            Box::pin(async move {
                build_handover_text(&state, &client, goal.as_deref(), cross_machine, &cancel).await
            })
        };
        let generated = Arc::new(Mutex::new(None));
        let loader = HandoverLoader::spawn(
            generation_future,
            cancel,
            &tokio::runtime::Handle::current(),
            generated.clone(),
        );
        if !services.open_overlay(Box::new(loader)) {
            return self.notify_command(
                ctx,
                generation,
                HANDOVER_REQUIRES_TUI.to_string(),
                NotifyKind::Error,
            );
        }
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        let generated = generated.lock().unwrap_or_else(|e| e.into_inner()).take();
        let text = match generated {
            Some(HandoverGeneration::Text(text)) => text,
            Some(HandoverGeneration::Failed(error)) => {
                return self.notify_command(
                    ctx,
                    generation,
                    format!("Handover failed: {error}"),
                    NotifyKind::Error,
                );
            }
            Some(HandoverGeneration::Cancelled) | None => {
                return self.notify_command(
                    ctx,
                    generation,
                    "Handover cancelled".to_string(),
                    NotifyKind::Info,
                );
            }
        };

        // `ctx.ui.editor("Edit handover", generated.text)` (`:3131`).
        let dialog = services.clone();
        let edited = tokio::task::spawn_blocking(move || dialog.editor("Edit handover", &text))
            .await
            .ok()
            .flatten();
        if !connect::is_live_at(&self.state, generation) {
            return None;
        }
        let Some(edited) = edited.filter(|edited| !edited.trim().is_empty()) else {
            return self.notify_command(
                ctx,
                generation,
                "Handover cancelled".to_string(),
                NotifyKind::Info,
            );
        };

        // The editor can be open for a long time; reconnect before sending (`:3138-3143`).
        let client =
            match connect::ensure_connected(&self.state, connect::ConnectReason::Tool).await {
                Ok(client) => client,
                Err(e) => {
                    return self.notify_command(
                        ctx,
                        generation,
                        format!("Intercom unavailable: {e}"),
                        NotifyKind::Error,
                    );
                }
            };
        let result = deliver_handover(&self.state, &client, &route, &edited).await;
        // `const failed = result.details.error === true || result.details.delivered === false;`
        // cyrup's shared delivery reports every upstream error RESULT as an `Err` carrying the same
        // sentence, so both shapes are read here.
        let (failed, text) = match result {
            Ok(result) => {
                let details = result.details.as_ref();
                let flag = |key: &str| {
                    details
                        .and_then(|d| d.get(key))
                        .and_then(serde_json::Value::as_bool)
                };
                let failed = flag("error") == Some(true) || flag("delivered") == Some(false);
                (failed, crate::tools::first_text_content(&result))
            }
            Err(error) => (true, error.message.replace("**", "")),
        };
        if failed {
            self.notify_command(ctx, generation, text, NotifyKind::Error)
        } else {
            self.notify_command(
                ctx,
                generation,
                format!("Handover: {text}"),
                NotifyKind::Info,
            )
        }
    }

    /// The `/intercom` command body (pi `openIntercomOverlay`, `index.ts:1810-1874`, degraded to text
    /// per the port doc §4.3): list sessions over the live broker, then either render the session
    /// picker (no args) or resolve `<target>` + send `<message…>` via [`compose_send`].
    async fn run_intercom_command(
        &self,
        client: &Arc<IntercomClient>,
        args: &str,
    ) -> crate::Result<String> {
        let sessions = client.list_sessions().await?;
        let my_id = client.session_id();
        let Some(current) = my_id
            .as_deref()
            .and_then(|id| sessions.iter().find(|s| s.id == id).cloned())
        else {
            // `v0.16.1 index.ts:3174` — no article, no trailing period.
            return Ok("Current session is missing from intercom session list".to_string());
        };
        // `duplicates = duplicateSessionNames(allSessions)` (`v0.10.1 index.ts:2393`) — computed
        // over EVERY session including the current one, before the self-filter below, so a peer
        // sharing this session's own name is still labelled with its id suffix.
        let duplicates =
            crate::identity::duplicate_session_names(sessions.iter().map(|s| s.name.as_deref()));
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
                return Ok(format!(
                    "{compose}\n\nType `/intercom {target} <message>` to send."
                ));
            }
            return Ok(format!(
                "Usage: /intercom <session> <message>\n\n{}",
                render_picker(others)
            ));
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
        // `contact_supervisor` only for a subagent child with orchestrator metadata
        // (index.ts:1162-1163,1425).
        //
        // Upstream registers `intercom` UNCONDITIONALLY (`pi-intercom index.ts:2088`) and is right
        // to: there an uninstalled intercom is an ABSENT npm package, so nothing else can claim the
        // name. cyrup diverges by force-attaching this extension for any bridged child even with no
        // install (`intercom_extension_for_env_concrete`), so an unconditional registration can
        // DUPLICATE the subagents crate's `NativeChildIntercomTool` — and a duplicate tool name is
        // a hard extension-load failure that stops the child booting at all.
        //
        // Stand down ONLY when BOTH hold: we are here purely by force-attach (`!installed`) AND the
        // native provider is present and functional (`native_supervisor_channel`, i.e.
        // `CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR` is set). That var is exactly what the native
        // fallback's own metadata requires (every lookup there is `?`-terminated), and it is the
        // root of its request/reply channel — so when it is ABSENT the native side registers
        // nothing at all and this extension is the only possible provider.
        //
        // The CONVERSE does not hold, and the asymmetry matters: a channel dir being PRESENT does
        // not guarantee the native side claims the name. Its gate has a SECOND term — `intercom`
        // must appear in `required_child_tools` (`native_supervisor.rs`'s
        // `native_child_intercom_fallback_should_register`) — and upstream's
        // `legacySupervisorPairing` filter DROPS `intercom` from that list when the persona also
        // declares `contact_supervisor` (`exec/tool_surface.rs`'s `required_child_tools`). So for
        // such a persona, with no install, NEITHER side registers `intercom`.
        //
        // That is intended, not a hole to plug here: the child still gets `contact_supervisor` from
        // the native side (whose own gate does NOT read `required_child_tools`), and it matches
        // upstream, where an uninstalled intercom is an absent package and the paired alias is
        // explicitly legacy plumbing rather than a requirement (pi #1207). Widening this guard to
        // cover it would re-introduce the duplicate in exactly the case upstream removed the name
        // for, and would drag this crate into reading `required_child_tools`.
        //
        // ⚠ Gating on `!installed` ALONE is a real, measured defect, not a hypothetical: a
        // BACKGROUND child is bridged (so this extension attaches) but has no channel dir, because
        // the detached runner resolves its parent-session anchor independently of the orchestrator
        // target it was handed. Standing down there leaves ZERO providers for a name the persona
        // REQUIRES, and the child refuses to start with "requested unavailable child tools:
        // intercom".
        if self.installed || !self.native_supervisor_channel {
            api.register_tool(Arc::new(IntercomTool::new(self.state.clone())));
        }
        // `v0.10.1 index.ts:1505-1507`:
        //   `if (childOrchestratorMetadata && !nativeSupervisorChannelAvailable) { pi.registerTool(…) }`
        // A child launched through the NATIVE supervisor channel must not also be handed the legacy
        // broker-routed tool: the model picks one, so the same decision can be requested through two
        // mechanisms while the parent polls only one of them.
        if let Some(metadata) = &self.metadata
            && !self.native_supervisor_channel
        {
            api.register_tool(Arc::new(ContactSupervisorTool::new(
                self.state.clone(),
                metadata.clone(),
            )));
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
        // The `/intercom` overlay command (pi `registerCommand("intercom", …)`, `v0.16.1
        // index.ts:3223-3226`). Bare, with a terminal UI, it opens the live session list
        // ([`Self::open_intercom_overlay`]); `/intercom <target> <message>` is cyrup's text form of
        // the compose send, and without an interactive surface the list renders as text.
        api.register_command(
            INTERCOM_COMMAND,
            CommandDescriptor {
                description: "Open the session intercom picker / send a message".to_string(),
                completions: Vec::new(),
            },
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
        // `/alias` (`v0.14.0 index.ts:2911-2914`), description verbatim (`:2912`).
        api.register_command(
            ALIAS_COMMAND,
            CommandDescriptor {
                description: "Set the current session alias (usage: /alias <name> or /alias menu)"
                    .to_string(),
                completions: Vec::new(),
            },
        );
        // `/handover` (`v0.16.1 index.ts:3238-3241`), description verbatim (`:3239`).
        api.register_command(
            HANDOVER_COMMAND,
            CommandDescriptor {
                description: "Summarize this session and hand it over to another session (usage: /handover to pick a session, or /handover <target or project path> [next task])"
                    .to_string(),
                completions: Vec::new(),
            },
        );
        // `pi.registerShortcut("alt+m", { description: "Open session intercom", … })`
        // (`v0.16.1 index.ts:3243-3246`); the press lands at [`Self::execute_shortcut`].
        api.register_shortcut(INTERCOM_SHORTCUT, Some("Open session intercom".to_string()));
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
        // ICOM-042 §5-A — bind the project-pane launcher from the same hook, for the same reason:
        // it is a host capability, and this is the one place that runs before `init` with the host
        // in hand. `HerdrLauncher::from_env()` resolves `HERDR_BIN` and the agent binary but probes
        // nothing, so binding it costs no process; whether Herdr is actually installed is decided
        // on first use, where it becomes an honest `HERDR_UNAVAILABLE` naming what to install
        // rather than a silently-dropped flag.
        self.state
            .set_project_pane_launcher(Arc::new(crate::project_pane::HerdrLauncher::from_env()));
    }

    /// Dispatch this extension's four commands (command-tier).
    ///
    /// - `/intercom` — no args → the live session list ([`Self::open_intercom_overlay`]), or its
    ///   text rendering when no interactive surface takes the overlay; `<target> <message…>` →
    ///   resolve the target and send it over the broker.
    /// - `/handover` — summarize this session and hand it over ([`Self::run_handover_command`], pi
    ///   `v0.16.1 index.ts:3023-3147`).
    /// - `/intercom-id` — insert this session's handoff snippet into the editor
    ///   ([`Self::run_intercom_id_command`], pi `v0.9.2 index.ts:2270-2289`).
    /// - `/alias` — rename this session and push the new identity to peers at once
    ///   ([`Self::run_alias_command`], pi `v0.14.0 index.ts:2779-2830`).
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        ctx.require_command_tier()?;
        if name == INTERCOM_ID_COMMAND {
            return Ok(self.run_intercom_id_command(ctx).await);
        }
        if name == ALIAS_COMMAND {
            return Ok(self.run_alias_command(args, ctx).await);
        }
        if name == HANDOVER_COMMAND {
            return Ok(self.run_handover_command(args, ctx).await);
        }
        if name != INTERCOM_COMMAND {
            return Err(ExtError::Component(format!(
                "native extension has no handler for command `{name}`"
            )));
        }
        // UW-10 / ICOM-085 — bare `/intercom` in the terminal UI is pi's live overlay
        // (`openIntercomOverlay`'s `hasUI && mode === "tui"` gate, `v0.16.1 index.ts:3152`). Only
        // when no interactive surface takes it does the text rendering below stand in.
        if args.trim().is_empty() && ctx.has_ui && ctx.mode == ExtMode::Tui {
            match self.open_intercom_overlay(ctx).await {
                OverlayRun::Ran(output) => return Ok(output),
                OverlayRun::NoSurface => {}
            }
        }
        // pi's overlay opens through `ensureConnected("overlay")` (index.ts:1827,1864) rather than a
        // bare `client` read: an overlay is a deliberate user action, so it is worth (re)spawning the
        // broker and reconnecting for. A failure still degrades to the same text, never a hard error.
        let Ok(client) =
            connect::ensure_connected(&self.state, connect::ConnectReason::Overlay).await
        else {
            return Ok(Some(
                "Intercom is not connected in this session.".to_string(),
            ));
        };
        let output = self
            .run_intercom_command(&client, args.trim())
            .await
            .unwrap_or_else(|e| format!("intercom command failed: {e}"));
        Ok(Some(output))
    }

    /// `alt+m` — pi's shortcut handler `async (ctx) => openIntercomOverlay(ctx)`
    /// (`v0.16.1 index.ts:3243-3246`). A shortcut returns nothing, so an outcome the command form
    /// would return as text (an Info notification) is notified here instead.
    async fn execute_shortcut(&self, key: &str, ctx: &HostCtx) -> Result<(), ExtError> {
        ctx.require_command_tier()?;
        if key != INTERCOM_SHORTCUT {
            return Err(ExtError::Component(format!(
                "native extension has no handler for shortcut `{key}`"
            )));
        }
        // `if (!liveContext?.hasUI || mode !== "tui") return;` (`:3152`).
        if !ctx.has_ui || ctx.mode != ExtMode::Tui {
            return Ok(());
        }
        if let OverlayRun::Ran(Some(message)) = self.open_intercom_overlay(ctx).await
            && let Some(services) = self.state.host_services()
        {
            services.notify(&message, NotifyKind::Info);
        }
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionStart { .. } => {
                // Capture this session's static `has_ui` (pi `hasUI`) ONCE, before the inbound loop
                // starts, so the inbound delivery policy (`inbound.rs`) can pick the interactive
                // trigger-turn branch vs. the non-interactive busy auto-reply (index.ts:739-758).
                self.state.set_has_ui(ctx.has_ui);
                // `agentRunning = false; expireHeldInboundMessages("session replaced before
                // injection")` (`v0.14.0 index.ts:1660-1661`, ICOM-062): a message held for the
                // previous runtime is never injected into this one. Run BEFORE `begin_runtime`
                // drops the previous client, so the sender is still told `expired` — upstream
                // expires after nulling its client and so tells nobody.
                self.state.set_agent_running(false);
                self.state
                    .expire_held_inbound("session replaced before injection");
                // `startSessionRuntime` (index.ts:926-951): publish the params every connect attempt
                // rebuilds its registration from, clear the shutdown latch, bump the generation and
                // reset the backoff ladder.
                connect::begin_runtime(&self.state, self.connect_params(ctx.model()));
                // ICOM-064 — `pi.events.emit(INTERCOM_SESSION_IDENTITY_EVENT, identityRequest)`
                // (`v0.14.0 index.ts:1645-1653`), after the runtime reset and before the id is
                // chosen. The typed emit runs every claimant before it returns, so the claim is
                // settled here and the connect below registers under it from its first attempt.
                let identity_request = crate::identity::IntercomSessionIdentityRequestV1::new();
                if let Some(services) = self.state.host_services() {
                    services.emit_typed_event(
                        crate::identity::INTERCOM_SESSION_IDENTITY_EVENT,
                        &identity_request,
                    );
                }
                self.state
                    .set_claimed_intercom_session_id(identity_request.claimed());
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
                    if let Err(e) =
                        connect::ensure_connected(&state, connect::ConnectReason::Startup).await
                    {
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
                // `expireHeldInboundMessages("session shut down before injection")`
                // (`v0.14.0 index.ts:1799`, ICOM-062) — before the disconnect below, so the
                // `expired` receipts still reach the senders.
                self.state
                    .expire_held_inbound("session shut down before injection");
                if let Some(client) = self.state.client() {
                    client.disconnect();
                }
                self.state.set_client(None);
                self.state
                    .tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .reset();
                // `agentRunning = false; activeTools.clear()` (`v0.10.1 index.ts:1408-1409`).
                self.state.set_agent_running(false);
                HookOutcome::Noop
            }
            HostEvent::AgentStart => {
                // `idleWakeRequestedAt = 0;` FIRST (`index.ts:2077@v0.16.1`, `104b83c` #154): the
                // woken run has started, so the reservation is spent — before the liveness test,
                // exactly as upstream orders it, and before the flush below, so a `human-first`
                // idle release is no longer held back by it.
                self.state.clear_idle_wake();
                // `agentRunning = true; if (runtimeContext) flushHeldInboundMessages(runtimeContext,
                // runtimeGeneration); activeTools.clear(); syncPresenceStatus()`
                // (`v0.14.0 index.ts:1824-1832`). The flush is ICOM-062's `agent_start` edge: a
                // message held while the session was busy without a run is steered onto the run
                // that has just started (or, non-interactive, auto-replied).
                self.state.set_agent_running(true);
                crate::inbound::flush_held_inbound_messages(
                    &self.state,
                    self.state.connect.generation(),
                );
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
                        identity
                            .name
                            .as_ref()
                            .map(|_| identity.runtime_fallback_alias),
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
                self.state
                    .tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .begin_turn(now_ms());
                HookOutcome::Noop
            }
            HostEvent::TurnEnd { message, .. } => {
                // `pi.on("turn_end") -> replyTracker.endTurn()` (`v0.10.1 index.ts:1416-1424`).
                self.state
                    .tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .end_turn();
                // ICOM-063 — `busyDelivery: "human-first"` releases one held peer per turn
                // boundary (`v0.14.0 index.ts:1814-1822`). This handler is awaited before the
                // agent loop's next steering poll, and the live host's
                // `HostServices::inject_message_steer` steers onto the running agent synchronously
                // (pi's `agent.steer`), so the released peer is already queued when this returns
                // and rides this run.
                crate::inbound::release_held_inbound_at_turn_end(&self.state, message);
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
    /// The options-free form of [`Self::render_call_under`], under the default (collapsed)
    /// options.
    fn render_call(&self, key: &str, call: &serde_json::Value) -> Option<serde_json::Value> {
        self.render_call_under(key, call, &cyrup_ext::RenderOptions::default())
    }

    /// `renderCall(args, theme, context)` for both tools (`index.ts:2394-2411` and
    /// `:2891-2914@v0.16.1`). ICOM-083 (`d5a8fd1` #153): `context.expanded` swaps the 96-char
    /// message preview for the full, raw outgoing body, so this is the hook the host re-invokes
    /// when Ctrl+O toggles. See [`crate::tools::render`] for what the seam still does not carry
    /// (`theme`).
    fn render_call_under(
        &self,
        key: &str,
        call: &serde_json::Value,
        opts: &cyrup_ext::RenderOptions,
    ) -> Option<serde_json::Value> {
        let text = match key {
            "intercom" => crate::tools::render::render_intercom_call(call, opts.expanded),
            "contact_supervisor" => {
                crate::tools::render::render_contact_supervisor_call(call, opts.expanded)
            }
            _ => return None,
        };
        Some(serde_json::Value::String(text))
    }

    /// `renderResult` for both tools, under the default options (collapsed, not partial) — the
    /// options-free form of [`Self::render_result_under`].
    fn render_result(&self, key: &str, result: &serde_json::Value) -> Option<serde_json::Value> {
        self.render_result_under(key, result, &cyrup_ext::RenderOptions::default())
    }

    /// `renderResult(result, { isPartial }, theme, context)` for both tools (`v0.14.0
    /// index.ts:2723-2744` and `:2157-2175`). Upstream branches on `isPartial` and
    /// `context.expanded` — the latter is what collapses a `list` / `list-cwd` roster to one line
    /// (ICOM-066) — so this is the hook the host re-invokes when either moves.
    fn render_result_under(
        &self,
        key: &str,
        result: &serde_json::Value,
        opts: &cyrup_ext::RenderOptions,
    ) -> Option<serde_json::Value> {
        let text = match key {
            "intercom" => crate::tools::render::render_intercom_result(result, opts),
            "contact_supervisor" => {
                crate::tools::render::render_contact_supervisor_result(result, opts)
            }
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
        Some(std::sync::Arc::new(crate::ui::InlineMessageComponent::new(
            card,
        )))
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
    Ok(intercom_extension_for_env_concrete(agent_dir, cwd)?
        .map(|ext| ext as Arc<dyn NativeExtension>))
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
    // Probed once, used twice: the attach gate immediately below, and the name-ownership decision
    // in `init` (see [`IntercomExtension::installed`]).
    let installed = is_installed(&intercom_dir);
    // The force-attach: a bridged child attaches even with no install so the supervisor surface
    // stays reachable. Whether it also OWNS the `intercom` name is decided in `init`.
    if metadata.is_none() && !installed {
        return Ok(None);
    }
    Ok(Some(Arc::new(
        IntercomExtension::new(agent_dir, cwd, config, metadata)?.with_installed(installed),
    )))
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

    /// ICOM-084 — `pi.on("agent_start", () => { idleWakeRequestedAt = 0; … })`
    /// (`index.ts:2077@v0.16.1`) and `startSessionRuntime`'s `idleWakeRequestedAt = 0` (`:1883`):
    /// the woken run starting spends the reservation, and so does a new runtime. Without the first,
    /// every idle delivery after the first wake would be denied its wake for 10 s; without the
    /// second, a wake reserved by the previous session would gag the new one.
    #[tokio::test]
    async fn agent_start_and_a_new_runtime_spend_the_idle_wake_reservation() {
        let (dir, ext) = test_extension();
        let ctx = HostCtx::event(cyrup_ext::ExtMode::Print, false, dir.path().to_path_buf());
        ext.state.request_idle_wake();
        assert!(ext.state.idle_wake_pending());
        let _ = ext.on_event(&HostEvent::AgentStart, &ctx).await;
        assert!(!ext.state.idle_wake_pending(), "agent_start clears it");

        ext.state.request_idle_wake();
        crate::connect::begin_runtime(
            &ext.state,
            crate::connect::ConnectParams {
                agent_dir: dir.path().join("agent"),
                metadata: None,
                model: None,
            },
        );
        assert!(!ext.state.idle_wake_pending(), "a new runtime clears it");
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
        let paths = value["skillPaths"]
            .as_array()
            .expect("skillPaths is a list");
        assert_eq!(paths.len(), 1, "exactly the one bundled skill: {paths:?}");
        let path = std::path::PathBuf::from(paths[0].as_str().expect("a path string"));
        assert!(
            path.is_file(),
            "the declared path must exist on disk: {path:?}"
        );
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
        host.load_native(Arc::new(ext))
            .await
            .expect("the native loads");
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
        host.load_native(Arc::new(ext))
            .await
            .expect("the native loads");
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
        let legacy =
            registered_tool_names(child_extension().with_native_supervisor_channel(false)).await;
        assert!(
            legacy.iter().any(|n| n == "contact_supervisor"),
            "a child WITHOUT the native channel must be handed the legacy tool: {legacy:?}"
        );
        assert!(
            legacy.iter().any(|n| n == "intercom"),
            "`intercom` is registered always: {legacy:?}"
        );

        // Native channel available: the SAME extension minus that one tool — `intercom` still
        // registers, so the absence below is the gate firing and not a failed `init`.
        let native =
            registered_tool_names(child_extension().with_native_supervisor_channel(true)).await;
        assert!(
            !native.iter().any(|n| n == "contact_supervisor"),
            "a child ON the native channel must NOT also get the broker-routed tool: {native:?}"
        );
        assert!(
            native.iter().any(|n| n == "intercom"),
            "`intercom` is registered always: {native:?}"
        );
    }

    /// The two tool renderers are wired to the names the tools actually register under — a renderer
    /// keyed on the wrong string is a silent no-op, since the host simply falls through.
    #[test]
    fn both_tool_renderers_are_keyed_on_the_registered_tool_names() {
        let (_dir, ext) = test_extension();
        let call = serde_json::json!({ "action": "send", "to": "reviewer", "message": "hi" });
        assert_eq!(
            ext.render_call("intercom", &call)
                .unwrap()
                .as_str()
                .unwrap(),
            "intercom send → reviewer\n  hi"
        );
        assert!(
            ext.render_call("contact_supervisor", &serde_json::json!({}))
                .is_some()
        );
        assert!(ext.render_call("not_our_tool", &call).is_none());
        // ICOM-083: the options-aware hook is the one the host re-invokes on Ctrl+O, and expanded
        // it shows the raw body instead of the normalized preview.
        let spaced =
            serde_json::json!({ "action": "send", "to": "reviewer", "message": "a\n\n  b" });
        let expanded = cyrup_ext::RenderOptions {
            expanded: true,
            ..cyrup_ext::RenderOptions::default()
        };
        assert_eq!(
            ext.render_call_under("intercom", &spaced, &expanded)
                .unwrap()
                .as_str()
                .unwrap(),
            "intercom send → reviewer\n  a\n\n  b"
        );
        assert_eq!(
            ext.render_call("intercom", &spaced)
                .unwrap()
                .as_str()
                .unwrap(),
            "intercom send → reviewer\n  a b"
        );
        assert!(
            ext.render_call_under("not_our_tool", &spaced, &expanded)
                .is_none()
        );

        let result = serde_json::json!({
            "content": [{ "type": "text", "text": "Message sent to reviewer" }],
            "details": { "messageId": "0192f3c1-9a10-7000", "delivered": true },
        });
        assert_eq!(
            ext.render_result("intercom", &result)
                .unwrap()
                .as_str()
                .unwrap(),
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
                endpoint_epoch: None,
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
                tmux_pane: None,
                herdr_pane_id: None,
                herdr_location: None,
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
                    endpoint_epoch: None,
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
                    tmux_pane: None,
                    herdr_pane_id: None,
                    herdr_location: None,
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
        let ev = HostEvent::TurnStart {
            turn_index: 0,
            timestamp: now_ms(),
        };
        let _ = ext.on_event(&ev, &ctx).await;

        let resolved = ext
            .state()
            .tracker
            .lock()
            .unwrap()
            .resolve_reply_target(None, None, now_ms())
            .expect(
                "current_turn_context resolves a bare reply with no `to`, despite 2 pending asks",
            );
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
