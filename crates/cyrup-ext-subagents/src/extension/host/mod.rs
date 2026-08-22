//! [`SubagentsExtension`]: the `NativeExtension` facade (arch-SA §3.1/§3.2) — its own state
//! and construction. The trait impl itself is [`native_impl`], the registration gate and the
//! three factories the binary calls are [`registration`].

pub(crate) mod native_impl;
pub(crate) mod profiles;
pub(crate) mod registration;
pub(crate) mod slash;
pub(crate) mod slash_render;

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::ExtensionId;
use cyrup_ext::native::HostCtx;

use crate::registration::SubagentExtensionConfig;
use crate::watchdog::register_main::{watchdog_config_dirs, watchdog_model_info};
use crate::extension::EXTENSION_ID;
use crate::extension::executor::SubagentExecutor;
use crate::extension::host::registration::RegistrationMode;
use crate::extension::tool::SubagentTool;

/// The SubAgents extension's `NativeExtension` facade (arch-SA §3.1). In [`RegistrationMode::Full`]
/// registers the `subagent` tool + all 12 slash commands at [`NativeExtension::init`], resumes
/// background-run tracking on [`HostEvent::SessionStart`], and routes every slash command through the
/// SAME [`SubagentExecutor`] the tool itself uses (R-SA-130). In [`RegistrationMode::ChildSafe`]
/// registers only the restricted, mutation-blocked tool (the fanout-child surface).
/// The intercom companion's three broker-backed seam channels (delivery + clarify + steer), handed
/// to [`SubagentsExtension::with_channels`] as one unit. A named alias so the `with_mode_and_channels`
/// parameter stays within clippy's `type_complexity` budget.
type IntercomSeamChannels = (
    Arc<dyn crate::tui::intercom::DeliveryChannel>,
    Arc<dyn crate::tui::intercom::ClarifyChannel>,
    Arc<dyn crate::tui::intercom::SteerChannel>,
);

pub struct SubagentsExtension {
    id: ExtensionId,
    executor: Arc<SubagentExecutor>,
    /// Captured at construction time (mirrors [`SubagentTool`]'s own doc: `NativeExtension::init`
    /// carries no `HostCtx`, so the session's working directory must be threaded in explicitly by
    /// whichever caller constructs this extension — `crates/cyrup/src/main.rs`'s three call
    /// sites, each of which already resolves the session's cwd before constructing this type).
    cwd: PathBuf,
    /// The child-mode registration surface (T6). Defaults to [`RegistrationMode::Full`] for the root
    /// orchestrator; a fanout-authorized child is built with [`RegistrationMode::ChildSafe`].
    mode: RegistrationMode,
    /// The NATIVE supervisor channel (pi `createNativeSupervisorChannel(pi, state)`,
    /// `extension/index.ts:372`). Constructed for every extension and STARTED at `SessionStart`
    /// (`extension/index.ts:757`) in [`RegistrationMode::Full`] only — upstream registers its parent
    /// tools inside `start()`, which a `ChildSafe` child never reaches because it does not subscribe
    /// to `session_start` at all.
    supervisor_channel: Arc<crate::native_supervisor::NativeSupervisorChannel>,
    /// The orchestrator's watchdog runtime (pi `const mainWatchdog = registerMainWatchdog(pi)`,
    /// `extension/index.ts:375`). Built for every extension, wired to the SAME executor-held
    /// capability backend the supervisor channel uses, and driven from `on_event` in
    /// [`RegistrationMode::Full`] only — a `ChildSafe` child subscribes to nothing, and the CHILD
    /// role's watchdog is a different object entirely
    /// ([`crate::watchdog::register_child`], installed by [`crate::prompt_runtime`]).
    ///
    /// Default OFF: `DEFAULT_WATCHDOG_CONFIG.enabled` is `false`, so this reviews nothing until a
    /// `settings.json` or `/subagents-watchdog on` turns it on.
    watchdog: Arc<crate::watchdog::runtime::MainWatchdogRuntime>,
    /// pi's `let fleetOpen = false` closure variable in `registerSlashCommands`
    /// (`slash/slash-commands.ts:632`) — the re-entrancy guard `showFleet` checks before opening
    /// the inspector a second time (`:639-642`). Process-lifetime, exactly like pi's closure scope.
    fleet_open: Arc<std::sync::atomic::AtomicBool>,
    /// pi's `state.fleetInspectorOpen` (`tui/fleet.ts:844-845`) — a SEPARATE latch from
    /// [`Self::fleet_open`]: the status widget reads it to unregister itself while the overlay is
    /// up (`tui/fleet-status.ts:306`), so the two surfaces never render at once.
    fleet_inspector_open: Arc<std::sync::atomic::AtomicBool>,
    /// pi `const fleetViewEnabled = config.fleetView !== false` (`extension/index.ts:333`) —
    /// when false, upstream leaves `fleetStatus` `undefined` entirely (`:378-383`) and no widget
    /// ever registers. Captured at construction, exactly as upstream captures it.
    fleet_view_enabled: bool,
    /// pi's single `SubagentFleetStatus` instance, constructed with the resolved
    /// `fleetViewPlacement` (`extension/index.ts:334,382`). Published through
    /// [`cyrup_ext::HostServices::set_widget`] by [`Self::refresh_fleet_status_widget`].
    fleet_status: Arc<std::sync::Mutex<crate::tui::fleet_status::SubagentFleetStatus>>,
}

impl SubagentsExtension {
    /// Construct the extension under its fixed, well-known id, with default config (tier 5 of
    /// R-SA-133 — the hardcoded extension defaults every other config tier layers on top of) and
    /// the current process working directory (mirrors [`cyrup_ext::facade::HostConfig::default`]'s
    /// own `std::env::current_dir()` fallback convention). Prefer
    /// [`SubagentsExtension::with_config_and_cwd`] when the caller already has a resolved session
    /// cwd in hand (every real `cyrup` binary call site does).
    #[must_use]
    pub fn new() -> Self {
        Self::with_config_and_cwd(
            SubagentExtensionConfig::default(),
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    }

    /// Construct the extension with an explicit, pre-resolved [`SubagentExtensionConfig`] (the
    /// config-layering rules per R-SA-133's tiers 2-5, resolved by the caller before
    /// construction — normally `crates/cyrup/src/main.rs`'s own config-loading step, per this
    /// crate's `registration/mod.rs` doc), using the current process working directory.
    #[must_use]
    pub fn with_config(config: SubagentExtensionConfig) -> Self {
        Self::with_config_and_cwd(
            config,
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    }

    /// Construct the extension with both an explicit config and an explicit `cwd` (the
    /// session/harness's own working directory) — the constructor a test harness (or a future
    /// per-session extension-construction seam) should prefer, since it avoids any dependence on
    /// the process's own current directory at all.
    #[must_use]
    pub fn with_config_and_cwd(config: SubagentExtensionConfig, cwd: PathBuf) -> Self {
        Self::with_mode(config, cwd, RegistrationMode::Full)
    }

    /// Construct the extension with an explicit [`RegistrationMode`] (T6 child-mode gate): the
    /// binary builds a [`RegistrationMode::Full`] extension for the root orchestrator and a
    /// [`RegistrationMode::ChildSafe`] one for a fanout-authorized child. See
    /// [`crate::extension::host::registration::subagent_extension_for`]/[`crate::extension::host::registration::subagent_extension_for_env`] for the callers.
    #[must_use]
    pub fn with_mode(config: SubagentExtensionConfig, cwd: PathBuf, mode: RegistrationMode) -> Self {
        Self::with_mode_and_channels(config, cwd, mode, None)
    }

    /// Construct a [`RegistrationMode::Full`] root orchestrator extension whose out-of-band delivery,
    /// clarify/ask, and live-child steer channels are the intercom companion's REAL broker-backed
    /// impls (item 2 of reconciliation §4 step 5), replacing the
    /// `NoTransportChannel`/no-live `AskLock`/`NoTransportSteerChannel` defaults — CLOSING
    /// R-SA-123/124/125 (out-of-band grouped delivery + reduced inline receipt), R-SA-119/120
    /// (clarify pause) + backing the R-SA-037 detach-trigger arm, and R-SA-086 (live-child
    /// `action='resume'` follow-up delivery). Called from the `crates/cyrup/src/main.rs`
    /// session-build sites with `IntercomExtension::{delivery_channel,clarify_channel,steer_channel}`
    /// (the port doc §8.4 item 1 handoff).
    #[must_use]
    pub fn with_channels(
        config: SubagentExtensionConfig,
        cwd: PathBuf,
        delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
        clarify: Arc<dyn crate::tui::intercom::ClarifyChannel>,
        steer: Arc<dyn crate::tui::intercom::SteerChannel>,
    ) -> Self {
        Self::with_mode_and_channels(
            config,
            cwd,
            RegistrationMode::Full,
            Some((delivery, clarify, steer)),
        )
    }

    /// The shared constructor body: builds the [`SubagentExecutor`], applies `config`, and — when
    /// `channels` is `Some` — threads the real intercom delivery/clarify channels into the executor
    /// (item 2). `None` keeps this crate's `NoTransportChannel`/no-live-`AskLock` degrade defaults.
    #[must_use]
    fn with_mode_and_channels(
        config: SubagentExtensionConfig,
        cwd: PathBuf,
        mode: RegistrationMode,
        channels: Option<IntercomSeamChannels>,
    ) -> Self {
        let executor = match channels {
            Some((delivery, clarify, steer)) => {
                SubagentExecutor::new().with_channels(delivery, clarify, steer)
            }
            None => SubagentExecutor::new(),
        };
        // pi `const fleetViewEnabled = config.fleetView !== false` +
        // `const fleetViewPlacement = resolveFleetViewPlacement(config.fleetViewPlacement)`
        // (`extension/index.ts:333-334`), consumed by the `fleetStatus` construction at `:378-383`.
        let fleet_view_enabled = config.fleet_view;
        let fleet_status = crate::tui::fleet_status::SubagentFleetStatus::new(
            crate::tui::fleet_status::FleetStatusOptions {
                placement: crate::tui::fleet_status::resolve_fleet_view_placement(
                    config.fleet_view_placement.as_deref(),
                ),
                ..crate::tui::fleet_status::FleetStatusOptions::default()
            },
        );
        // `SubagentExecutor::new()`'s own config lock is freshly constructed and uncontended at
        // this point (no other clone of `executor.config` can exist yet), so a `try_lock` here is
        // guaranteed to succeed; falling through to the default on the (unreachable) contended
        // case keeps this constructor infallible rather than needing `async`/panic.
        if let Ok(mut guard) = executor.config_cell().try_lock() {
            *guard = config;
        }
        let executor = Arc::new(executor);
        // pi `extension/index.ts:375`. The services closure resolves LATE: `set_host_services` runs
        // before `init`, but this constructor runs before both, so the sinks read the executor's
        // slot at delivery time rather than capturing a backend that does not exist yet.
        let watchdog_executor = Arc::clone(&executor);
        let watchdog = crate::watchdog::register_main::register_main_watchdog(
            Arc::new(move || watchdog_executor.host_services()),
            &cwd,
            crate::watchdog::register_main::RegisterMainWatchdogOptions::default(),
        );
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            executor,
            cwd,
            mode,
            supervisor_channel: Arc::new(crate::native_supervisor::NativeSupervisorChannel::new()),
            watchdog,
            fleet_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            fleet_inspector_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            fleet_view_enabled,
            fleet_status: Arc::new(std::sync::Mutex::new(fleet_status)),
        }
    }

    /// The orchestrator's watchdog runtime — exposed so an integration test (and any future
    /// non-`InitApi` caller) can drive the same state machine `on_event` drives.
    #[must_use]
    pub fn watchdog(&self) -> &Arc<crate::watchdog::runtime::MainWatchdogRuntime> {
        &self.watchdog
    }

    /// `/subagents-watchdog <args>` (pi `handleWatchdogCommand`, `register-main.ts:246-375`,
    /// registered at `:403-409`).
    ///
    /// Upstream has two output channels — `sendSlashText` (a transcript message) and
    /// `ctx.ui.notify(…, "error")` — and this maps them onto cyrup's two: the returned `String`
    /// (surfaced as an Info notification by `try_execute_extension_command`) and
    /// [`cyrup_ext::host::HostServices::notify`] at `Error` level, which is exactly the split
    /// [`cyrup_ext::native::NativeExtension::execute_command`]'s own doc prescribes.
    ///
    /// The `test concern|blocker <text>` arm additionally SENDS the recorded warning
    /// (`register-main.ts:371`), which is the caller's job here because the delivery capability is
    /// the extension's, not the runtime's.
    fn execute_watchdog_command(&self, args: &str, ctx: &HostCtx) -> Option<String> {
        use crate::watchdog::register_main::{
            handle_watchdog_command, WatchdogCommandContext, WatchdogCommandOutcome,
        };
        let services = self.executor.host_services();
        let registry = crate::watchdog::model_selection::BuiltinWatchdogModelRegistry::new(
            watchdog_config_dirs().as_ref(),
        );
        let command_ctx = WatchdogCommandContext {
            cwd: ctx.cwd.clone(),
            registry: &registry,
            current_model: services
                .as_ref()
                .and_then(|s| s.current_model())
                .as_deref()
                .and_then(watchdog_model_info),
            thinking_level: services.as_ref().and_then(|s| s.thinking_level()),
        };
        let (outcome, warning) = handle_watchdog_command(&self.watchdog, args, &command_ctx);
        if let (Some(details), Some(services)) = (warning.as_ref(), services.as_ref()) {
            let message =
                crate::watchdog::warning_format::create_watchdog_warning_message_from_details(
                    details, true,
                );
            let _ = services.inject_message(
                &message.content,
                Some(crate::watchdog::types::SUBAGENT_WATCHDOG_WARNING_TYPE),
                message.display,
                false,
            );
        }
        match outcome {
            WatchdogCommandOutcome::Text(text) if text.trim().is_empty() => None,
            WatchdogCommandOutcome::Text(text) => Some(text),
            WatchdogCommandOutcome::UsageError(message) => {
                if let Some(services) = services.as_ref() {
                    services.notify(&message, cyrup_ext::NotifyKind::Error);
                }
                None
            }
        }
    }

    /// The NATIVE supervisor channel this extension owns — exposed so an integration test (and any
    /// future non-`InitApi` caller) can drive the real poll/reply path exactly as the host would.
    #[must_use]
    pub fn supervisor_channel(&self) -> &Arc<crate::native_supervisor::NativeSupervisorChannel> {
        &self.supervisor_channel
    }

    /// The shared executor, exposed so a caller (e.g. a future TUI progress widget, or a test)
    /// can drive the exact same dispatch path the tool/commands use without going through the
    /// `NativeExtension` trait object.
    #[must_use]
    pub fn executor(&self) -> &Arc<SubagentExecutor> {
        &self.executor
    }

    /// Construct the same [`SubagentTool`] `init` registers with the host, bound to this
    /// extension's own executor and cwd — exposed so an integration test (or a future non-`InitApi`
    /// caller) can drive the real `cyrup_core::Tool::execute` dispatch (the `tasks[]`/`chain[]`
    /// PARALLEL/CHAIN routing) exactly as the host would, without a `SessionBuilder` round-trip.
    #[must_use]
    pub fn subagent_tool(&self) -> SubagentTool {
        SubagentTool::new(self.executor.clone(), self.cwd.clone())
            .with_watchdog(Arc::clone(&self.watchdog))
    }
}

impl Default for SubagentsExtension {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use cyrup_core::Tool;
    use cyrup_ext::native::NativeExtension;
    use crate::background::RunMode;
    use crate::error::SubagentError;
    use crate::extension::testsupport::bare_single_step;
    use crate::extension::testsupport::dispatch_tool;
    use crate::extension::testsupport::scoped_missions;
    use crate::extension::testsupport::seed_running_run;
    use crate::extension::testsupport::tool_text;
    use crate::fork_context::ContextMode;
    use crate::registration::slash_commands::SlashCommandName;
    use crate::spawn::chain_graph::RunnerStep;
    use cyrup_core::CancelToken;
    use cyrup_core::ToolCallId;
    use cyrup_core::ToolError;
    use cyrup_core::ToolResult;

    #[test]
    fn id_is_stable() {
        let ext = SubagentsExtension::new();
        assert_eq!(ext.id(), ExtensionId::from("subagents"));
    }

    /// SUBA-046 — THE user-facing behaviour: an exhausted per-session spawn cap is a speed bump
    /// with an explicitly confirmed grant behind it, not a dead end that requires restarting the
    /// session (pi `subagent-executor.ts:4457-4527` @v0.43.0).
    ///
    /// The whole flow is exercised against the real dispatch surface, because every half of this
    /// item was individually present-and-useless before: the counter existed with no grant path,
    /// the verb was advertised in the child-safe tool description with no dispatch arm, and the
    /// refusal text told the user to "grant budget explicitly" against a verb that answered
    /// `Unknown action`.
    #[tokio::test]
    async fn an_exhausted_spawn_cap_can_be_reopened_by_a_confirmed_grant() {
        /// A host that reports a session id and ACCEPTS the confirmation, recording the body it was
        /// shown — pi passes the preview snapshot into the dialog, and a grant confirmed against
        /// numbers the user never saw would be the interesting way to get this wrong.
        #[derive(Default)]
        struct ConfirmingHost {
            confirmed: Arc<std::sync::Mutex<Vec<String>>>,
            accept: bool,
        }
        impl cyrup_ext::host::HostServices for ConfirmingHost {
            fn session_id(&self) -> Option<String> {
                Some("session-a".to_string())
            }
            fn confirm(
                &self,
                _prompt: &str,
                message: &str,
                _opts: &cyrup_ext::host::DialogOptions,
            ) -> bool {
                if let Ok(mut seen) = self.confirmed.lock() {
                    seen.push(message.to_string());
                }
                self.accept
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                missions: Some(scoped_missions(dir.path())),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        ext.executor().set_host_services(Arc::new(ConfirmingHost {
            confirmed: Arc::clone(&seen),
            accept: true,
        }));
        let tool = ext.subagent_tool();

        // Spend the single configured launch, then confirm the cap is really closed — the
        // precondition without which every assertion below could pass vacuously.
        let _spent = dispatch_tool(&tool, serde_json::json!({ "agent": "ghost", "task": "a" }))
            .await
            .expect_err("an unresolvable agent still fails after the reservation is granted");
        let closed = dispatch_tool(&tool, serde_json::json!({ "agent": "ghost", "task": "b" }))
            .await
            .expect_err("the session's spawn budget is exhausted");
        assert_eq!(
            closed.to_string(),
            "Subagent spawn limit reached for this session (1/1 used, 1 requested). 0 remaining; \
             the declared run cannot fit, so no children were started. Grant budget explicitly \
             from the root interactive session or start a new session.",
            "and the refusal points at the grant path, which must therefore exist"
        );

        // A grant larger than the ORIGINAL configured cap is refused (pi caps total grants at the
        // configured limit) — with the live numbers in the text, not a bare rejection.
        let too_big = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "grant-spawn-budget", "additional": 2 }),
        )
        .await
        .expect_err("2 is more than the 1 grantable");
        assert!(
            too_big.to_string().starts_with(
                "Spawn budget grant rejected: 2 requested but only 1 of the original configured \
                 limit remains grantable."
            ),
            "pi's verbatim grant-allowance refusal: {too_big}"
        );

        let granted = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "grant-spawn-budget", "additional": 1 }),
        )
        .await
        .expect("a grant within the allowance is applied");
        assert_eq!(
            tool_text(&granted),
            "Spawn budget grant applied: +1. Spawn budget: 1/2 used, 1 remaining (configured 1; \
             granted 1; grant allowance 0)"
        );
        assert_eq!(
            granted.details.as_ref().and_then(|d| d.get("spawnBudget")).and_then(|b| b.get("limit")),
            Some(&serde_json::json!(2)),
            "the snapshot rides along in details.spawnBudget: {:?}",
            granted.details
        );
        let bodies = seen.lock().expect("confirm log").clone();
        assert_eq!(bodies.len(), 1, "exactly one confirmation was asked for");
        assert!(
            bodies[0].starts_with("Add 1 launches to this logical session?")
                && bodies[0].contains("Spawn budget: 1/1 used, 0 remaining")
                && bodies[0].ends_with(
                    "Usage is not reset. Compaction keeps the same budget; a new parent session \
                     starts a fresh one."
                ),
            "pi's confirmation body, showing the PREVIEW numbers: {}",
            bodies[0]
        );

        // The grant is real: the next delegation is admitted past the budget and fails only on the
        // unresolvable agent, exactly as the first one did.
        let readmitted = dispatch_tool(&tool, serde_json::json!({ "agent": "ghost", "task": "c" }))
            .await
            .expect_err("an unresolvable agent still fails");
        assert!(
            readmitted.to_string().contains("agent not found: ghost"),
            "the granted launch must be ADMITTED past the budget: {readmitted}"
        );
    }

    /// SUBA-046 — the refusals that do NOT depend on a live confirmation, each pi's verbatim text.
    #[tokio::test]
    async fn a_spawn_budget_grant_is_refused_without_a_root_interactive_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                missions: Some(scoped_missions(dir.path())),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let tool = ext.subagent_tool();

        // pi `:4458` — no host services bound at all is cyrup's `!ctx.hasUI`.
        let no_ui = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "grant-spawn-budget", "additional": 1 }),
        )
        .await
        .expect_err("no interactive parent session");
        assert_eq!(
            no_ui.to_string(),
            "Action 'grant-spawn-budget' is available only from the root interactive parent \
             session."
        );

        // pi `:4465-4471` — a bound host that reports no session id.
        struct NoSessionHost;
        impl cyrup_ext::host::HostServices for NoSessionHost {}
        ext.executor().set_host_services(Arc::new(NoSessionHost));
        let no_session = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "grant-spawn-budget", "additional": 1 }),
        )
        .await
        .expect_err("no session id");
        assert_eq!(
            no_session.to_string(),
            "Action 'grant-spawn-budget' requires an active parent session id."
        );
    }

    /// SUBA-002 regression (pi `reserveSubagentSpawns`, `subagent-executor.ts:266-282` +
    /// `:3434-3441`): `maxSubagentSpawnsPerSession` is ENFORCED across a session's successive
    /// dispatches, not merely parsed. Pre-fix, the config field had no read site anywhere in the
    /// crate, so every call below routed straight into execution and the second/third calls failed
    /// with `"agent not found: ghost"` instead of the spawn-limit notice.
    ///
    /// The budget is charged UP FRONT: call 1 requests 2 of a 2-spawn budget and is admitted (it
    /// then fails on the unresolvable agent, as pi's would), and that failure does NOT refund — call
    /// 2 is rejected before any routing at all.
    #[tokio::test]
    async fn spawn_budget_is_charged_per_session_and_rejects_the_call_that_would_exceed_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 2,
                // Every dispatch below carries a `task`, so each auto-creates a mission and writes
                // a pointer into the GLOBAL index — `agent_dir()/missions/index`, i.e. the real
                // `~/.cyrup/agent`, unless scoped. See [`scoped_missions`].
                missions: Some(scoped_missions(dir.path())),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let tool = ext.subagent_tool();

        async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
            tool.execute(
                ToolCallId::from("t"),
                params,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
        }

        // Call 1: a 2-task fan-out exactly fills the 2-spawn budget (pi's comparison is a STRICT
        // `used + requested > maxSpawns`, so landing on the cap is admitted). It is admitted, and
        // therefore fails downstream on the unresolvable agent — NOT on the budget.
        let admitted = dispatch(&tool, serde_json::json!({
            "tasks": [{ "agent": "ghost", "task": "a" }, { "agent": "ghost", "task": "b" }]
        }))
        .await
        .expect_err("an unresolvable agent still fails after the reservation is granted");
        assert!(
            admitted.to_string().contains("agent not found: ghost"),
            "the first call must be ADMITTED past the budget (failing only on the agent): {admitted}"
        );

        // Call 2: the budget was billed up front and is not refunded by call 1's failure, so a
        // single further spawn is now over the cap and the whole call is rejected before routing.
        let rejected = dispatch(&tool, serde_json::json!({ "agent": "ghost", "task": "c" }))
            .await
            .expect_err("the session's spawn budget is exhausted");
        assert_eq!(
            rejected.to_string(),
            "Subagent spawn limit reached for this session (2/2 used, 1 requested). 0 remaining; \
             the declared run cannot fit, so no children were started. Grant budget explicitly \
             from the root interactive session or start a new session.",
            "pi's verbatim over-limit notice, with used/max/requested filled in"
        );
        assert!(
            !rejected.to_string().contains("agent not found"),
            "the rejection must fire BEFORE any routing/agent resolution: {rejected}"
        );

        // A fresh session zeroes the budget (pi `resetSessionState`), so the very same call is
        // admitted again afterwards.
        ext.executor().reset_spawn_budget();
        let after_reset = dispatch(&tool, serde_json::json!({ "agent": "ghost", "task": "c" }))
            .await
            .expect_err("post-reset the call is admitted and fails only on the agent");
        assert!(
            after_reset.to_string().contains("agent not found: ghost"),
            "a session reset must restore the budget: {after_reset}"
        );
    }

    /// SUBA-002's request-counting rules (pi `countRequestedSubagentSpawns`,
    /// `subagent-executor.ts:439-447`), observed through the rejection notice's `N requested` field:
    /// a CHAIN bills each step, with a dynamic-parallel step billed its worst-case fan-out
    /// (`expand.maxItems`, else `config.chain.dynamicFanout.maxItems`, else 0) and a static parallel
    /// step billed its task count.
    #[tokio::test]
    async fn chain_spawn_count_bills_dynamic_fanout_worst_case_and_parallel_width() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                chain: Some(crate::registration::ExtensionChainConfig {
                    dynamic_fanout: Some(crate::registration::DynamicFanoutConfig {
                        max_items: Some(7),
                    }),
                }),
                // The chains below carry tasks. They are refused by the spawn reservation, which
                // sits AHEAD of the mission binding, so no mission is created today — but that
                // ordering is the only thing standing between this test and a
                // `~/.cyrup/agent/missions/index` pointer. See [`scoped_missions`].
                missions: Some(scoped_missions(dir.path())),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let tool = ext.subagent_tool();

        async fn reject_text(tool: &SubagentTool, params: serde_json::Value) -> String {
            tool.execute(
                ToolCallId::from("t"),
                params,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("over the 1-spawn budget")
            .to_string()
        }

        // Every chain below is STRUCTURALLY VALID (the dynamic steps satisfy
        // `validate_dynamic_step_shape`), so the notice asserted here is the budget's, not a shape
        // diagnostic that happens to precede it.
        //
        // A sequential step (1) + a static parallel step of width 3 (3) + a dynamic-parallel step
        // with its own `expand.maxItems: 5` (5) == 9 requested.
        let explicit = reject_text(&tool, serde_json::json!({ "chain": [
            { "agent": "ghost", "task": "a", "as": "targets" },
            { "parallel": [
                { "agent": "ghost", "task": "b" },
                { "agent": "ghost", "task": "c" },
                { "agent": "ghost", "task": "d" }
            ] },
            {
                "expand": { "from": { "output": "targets", "path": "/items" }, "maxItems": 5 },
                "collect": { "as": "gathered" },
                "parallel": { "agent": "ghost", "task": "Handle {item}" }
            }
        ]}))
        .await;
        assert!(explicit.contains("(0/1 used, 9 requested)"), "got: {explicit}");

        // With `expand.maxItems` omitted the dynamic step falls back to the CONFIGURED
        // `chain.dynamicFanout.maxItems` (7 here), so 1 + 7 == 8 requested.
        ext.executor().reset_spawn_budget();
        let configured = reject_text(&tool, serde_json::json!({ "chain": [
            { "agent": "ghost", "task": "a", "as": "targets" },
            {
                "expand": { "from": { "output": "targets", "path": "/items" } },
                "collect": { "as": "gathered" },
                "parallel": { "agent": "ghost", "task": "Handle {item}" }
            }
        ]}))
        .await;
        assert!(configured.contains("(0/1 used, 8 requested)"), "got: {configured}");
    }

    /// SUBA-002 regression: the per-SESSION spawn budget covers the SLASH surface, not the
    /// `subagent` tool alone. Upstream gets this structurally — `/run`'s handler goes
    /// `runSlashSubagent` -> `requestSlashRun` -> the bridge at `extension/index.ts:512-517` ->
    /// `executeSubagentCollapsed` -> the SAME `executor.execute` whose `reserveSubagentSpawns`
    /// (`subagent-executor.ts:266-282`, called at `:3434-3441`) charges the tool — so the cap is
    /// unbypassable there. In this crate `dispatch_slash` is an independent entry into
    /// `SubagentExecutor`, and pre-fix it reached `run_foreground`/`spawn_background` with no charge
    /// at all: this test's `/run` calls all sailed past an exhausted budget and failed downstream on
    /// the unresolvable agent instead.
    ///
    /// Drives the REAL production surface end to end (`dispatch_slash(SlashCommandName::Run, …)`,
    /// i.e. the argument string a user types), for both the foreground and the `--bg` shape, and
    /// pins the notice to the byte-identical text the tool path emits.
    #[tokio::test]
    async fn slash_run_is_charged_against_the_same_session_spawn_budget_as_the_tool() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                // The task-bearing dispatches below auto-create missions; scope their pointer
                // index into this tempdir ([`scoped_missions`]) so none lands in `~/.cyrup/agent`.
                missions: Some(scoped_missions(dir.path())),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );

        // Spend the session's single spawn through the TOOL. It is admitted past the budget and so
        // fails only on the unresolvable agent — and the reservation is never refunded.
        let spent = ext
            .subagent_tool()
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "agent": "ghost", "task": "a" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("an unresolvable agent still fails after the reservation is granted");
        assert!(
            spent.to_string().contains("agent not found: ghost"),
            "the tool call must be ADMITTED past the budget: {spent}"
        );

        // The slash surface now sees an exhausted budget and must refuse — foreground AND `--bg`,
        // since pi bills the SINGLE shape exactly 1 either way.
        for args in ["ghost do the thing", "ghost do the thing --bg"] {
            let err = ext
                .dispatch_slash(SlashCommandName::Run, args, dir.path(), false)
                .await
                .expect_err("the session's spawn budget is exhausted");
            assert!(
                matches!(err, SubagentError::SpawnLimitExceeded(_)),
                "`/run {args}` must be refused by the budget, got: {err:?}"
            );
            assert_eq!(
                err.to_string(),
                "Subagent spawn limit reached for this session (1/1 used, 1 requested). 0 \
                 remaining; the declared run cannot fit, so no children were started. Grant \
                 budget explicitly from the root interactive session or start a new session.",
                "pi's verbatim over-limit notice, identical to the tool path's"
            );
            assert!(
                !err.to_string().contains("agent not found"),
                "the refusal must fire BEFORE agent resolution / any spawn: {err}"
            );
        }

        // A fresh session zeroes the budget (pi `resetSessionState`), so the very same `/run` is
        // admitted again afterwards and fails only on the agent — proving the refusal above was the
        // budget, not a blanket slash-path rejection.
        ext.executor().reset_spawn_budget();
        let after_reset = ext
            .dispatch_slash(SlashCommandName::Run, "ghost do the thing", dir.path(), false)
            .await
            .expect_err("post-reset the call is admitted and fails only on the agent");
        assert!(
            matches!(after_reset, SubagentError::AgentNotFound(_)),
            "a session reset must restore the slash surface's budget, got: {after_reset:?}"
        );
    }

    /// SUBA-002 follow-up: a dispatch the DEPTH ceiling refuses must not spend a spawn.
    ///
    /// pi checks the recursion ceiling at `subagent-executor.ts:3297-3312` and only then reaches
    /// `reserveSubagentSpawns` (`:3434-3441`), so a blocked call is billed nothing. cyrup's
    /// R-SA-055 guard lives one level down — inside `run_foreground`/`spawn_background`/
    /// `run_or_background_graph` — i.e. strictly AFTER each of the three charge sites SUBA-002
    /// added, so pre-fix every depth-blocked invocation of `/run`, `/chain`, `/parallel` and the
    /// `subagent` TOOL consumed budget it could never use: a subagent pinned at max depth could
    /// drain its whole session's allowance by repeatedly asking for children.
    ///
    /// Asserts BOTH halves, on all four surfaces: the call is refused with `DepthExceeded` (not
    /// `SpawnLimitExceeded`), and the budget is still intact afterwards — proven by a `1`-spawn cap
    /// that a subsequent `reserve_subagent_spawns(1, 1)` can still satisfy. Against the pre-fix
    /// ordering that reserve fails, because the blocked call already took the session's only spawn.
    #[tokio::test]
    async fn a_depth_blocked_dispatch_is_refused_before_it_can_spend_a_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        // `max_subagent_depth: 0` with no `CYRUP_SUBAGENT_DEPTH` in the env ⇒ current (0) >= max (0)
        // ⇒ blocked, the same state the executor-level R-SA-055 tests in `executor::background`
        // and `executor::foreground` use.
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                max_subagent_depth: 0,
                // Task-bearing dispatch, refused by the depth gate that sits AHEAD of the mission
                // binding — scoped anyway, for the same reason as
                // `chain_spawn_count_bills_dynamic_fanout_worst_case_and_parallel_width`.
                missions: Some(scoped_missions(dir.path())),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );

        // (1) the `subagent` TOOL.
        ext.executor().reset_spawn_budget();
        let tool_err = ext
            .subagent_tool()
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "agent": "ghost", "task": "a" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("a blocked depth ceiling refuses the dispatch");
        assert!(
            tool_err.to_string().contains("depth limit exceeded"),
            "the tool must be refused on DEPTH, not billed and then refused: {tool_err}"
        );
        assert!(
            ext.executor().reserve_subagent_spawns(1, 1).is_ok(),
            "a depth-blocked TOOL dispatch must not have consumed the session's only spawn"
        );

        // (2) `/run`, foreground and `--bg` (pi bills the SINGLE shape 1 either way).
        for args in ["ghost do the thing", "ghost do the thing --bg"] {
            ext.executor().reset_spawn_budget();
            let err = ext
                .dispatch_slash(SlashCommandName::Run, args, dir.path(), false)
                .await
                .expect_err("a blocked depth ceiling refuses the dispatch");
            assert!(
                matches!(err, SubagentError::DepthExceeded { .. }),
                "`/run {args}` must be refused on DEPTH, got: {err:?}"
            );
            assert!(
                ext.executor().reserve_subagent_spawns(1, 1).is_ok(),
                "a depth-blocked `/run {args}` must not have consumed the session's only spawn"
            );
        }

        // (3) the chain-shaped slash wrapper `/chain` // `/parallel` // `/run-chain` share.
        for background in [false, true] {
            ext.executor().reset_spawn_budget();
            let err = ext
                .run_or_background_chain(
                    dir.path(),
                    vec![RunnerStep::SingleStep(bare_single_step("ghost", "a"))],
                    RunMode::Chain,
                    None,
                    background,
                    None,
                )
                .await
                .expect_err("a blocked depth ceiling refuses the dispatch");
            assert!(
                matches!(err, SubagentError::DepthExceeded { .. }),
                "background={background}: the chain slash wrapper must be refused on DEPTH, \
                 got: {err:?}"
            );
            assert!(
                ext.executor().reserve_subagent_spawns(1, 1).is_ok(),
                "background={background}: a depth-blocked chain slash dispatch must not have \
                 consumed the session's only spawn"
            );
        }
    }

    /// SUBA-002's no-double-charge invariant: the `subagent` TOOL's chain/parallel shapes reserve
    /// exactly ONCE (in [`SubagentTool::execute`]) and then reach
    /// [`SubagentExecutor::run_or_background_graph`] through
    /// `route_chain_mode`/`route_parallel_mode` — never through the slash-only
    /// [`SubagentsExtension::run_or_background_chain`] wrapper that carries the second charge. A
    /// 3-wide tool fan-out under a 3-spawn budget must therefore bill 3, not 6.
    #[tokio::test]
    async fn tool_chain_dispatch_is_billed_exactly_once_not_twice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 3,
                // The chain below carries tasks, so it auto-creates a mission; scope its pointer
                // index into this tempdir ([`scoped_missions`]) instead of `~/.cyrup/agent`.
                missions: Some(scoped_missions(dir.path())),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );

        let admitted = ext
            .subagent_tool()
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "chain": [
                    { "agent": "ghost", "task": "a" },
                    { "parallel": [
                        { "agent": "ghost", "task": "b" },
                        { "agent": "ghost", "task": "c" }
                    ] }
                ]}),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("an unresolvable agent still fails after the reservation is granted");
        assert!(
            !admitted.to_string().contains("Subagent spawn limit reached"),
            "a 3-wide chain must fit exactly inside a 3-spawn budget: {admitted}"
        );

        // Exactly 3 charged, so `used` reads 3/3 (a double charge would have overflowed the cap
        // during the dispatch above and reported 6 requested against it).
        let exhausted = ext
            .dispatch_slash(SlashCommandName::Run, "ghost do the thing", dir.path(), false)
            .await
            .expect_err("the session's spawn budget is now exactly exhausted");
        assert_eq!(
            exhausted.to_string(),
            "Subagent spawn limit reached for this session (3/3 used, 1 requested). 0 remaining; \
             the declared run cannot fit, so no children were started. Grant budget explicitly \
             from the root interactive session or start a new session.",
            "the tool's chain dispatch must have been billed once (3), not twice (6)"
        );
    }

    /// END-TO-END for the TUNING KNOBS, driven from a real `config.json` document rather than a
    /// hand-built setting: pi reads `ctx.config?.proactiveSkillSubagents` off its `ExtensionConfig`
    /// and forwards it verbatim into `buildProactiveSkillSubagentRecommendationLines`
    /// (`agent-management.ts:765-770` @v0.43.0), whose `resolveProactiveSkillSubagentsConfig`
    /// (`proactive-skills.ts:38-59`) turns `minReferences`/`maxRecommendations`/`preferredAgent`
    /// into the recommender's filter, cap and carrier agent.
    ///
    /// Every assertion below is a knob CHANGING the rendered output away from pi's defaults
    /// (`minReferences: 2`, `maxRecommendations: 3`, `preferredAgent: "reviewer"`), so a config
    /// block that stopped reaching the recommender — silently dropped anywhere between
    /// `serde_json` and [`crate::discovery::management::ProactiveSkillsInput::setting`] — fails
    /// here. The `enabled: false` path cannot serve that role: it happens to also empty the
    /// availability scan, so it stays observable even when the setting is lost.
    #[tokio::test]
    async fn tool_list_applies_the_config_json_proactive_tuning_knobs_end_to_end() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir agents");
        // `audit-trail` and `bundle-up` each get two references (clearing the default
        // `minReferences: 2`); `solo-skill` gets exactly one (below it).
        for (name, skill) in [
            ("auditor-one", "audit-trail"),
            ("auditor-two", "audit-trail"),
            ("packer", "bundle-up"),
            ("stacker", "bundle-up"),
            ("solo", "solo-skill"),
        ] {
            std::fs::write(
                agents_dir.join(format!("{name}.md")),
                format!("---\nname: {name}\ndescription: An agent\nskills: {skill}\n---\nBody.\n"),
            )
            .expect("write agent");
        }
        for skill in ["audit-trail", "bundle-up", "solo-skill"] {
            let skill_dir = dir.path().join(".cyrup").join("skills").join(skill);
            std::fs::create_dir_all(&skill_dir).expect("mkdir skill");
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\ndescription: The {skill} skill.\n---\n\nHow to {skill}.\n"),
            )
            .expect("write skill");
        }

        /// Run a real `{ action: "list" }` through an extension built from a real `config.json`
        /// document, and return the rendered text.
        async fn list_with_config(cwd: &std::path::Path, config_json: &str) -> String {
            let config: SubagentExtensionConfig = serde_json::from_str(config_json)
                .unwrap_or_else(|e| panic!("config.json must deserialize: {e}"));
            let ext = SubagentsExtension::with_config_and_cwd(config, cwd.to_path_buf());
            let out = ext
                .subagent_tool()
                .execute(
                    ToolCallId::from("t"),
                    serde_json::json!({ "action": "list" }),
                    CancelToken::new(),
                    Box::new(|_u: cyrup_core::ToolUpdate| {}),
                )
                .await
                .expect("list is wired");
            out.content
                .iter()
                .find_map(|c| match c {
                    cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        }

        /// The `- <skill> via <agent> (...)` rows between the block header and the guardrails
        /// footer (`formatProactiveSkillSubagentRecommendations`, `proactive-skills.ts:159-173`).
        fn recommendation_rows(text: &str) -> Vec<String> {
            text.lines()
                .skip_while(|l| *l != "Proactive skill subagent suggestions:")
                .skip(1)
                .take_while(|l| !l.starts_with("Guardrails:"))
                .filter(|l| l.starts_with("- "))
                .map(str::to_string)
                .collect()
        }

        // --- `minReferences` ------------------------------------------------------------------
        // Lowering it to 1 admits the single-reference `solo-skill`, which pi's default of 2 keeps out.
        let text = list_with_config(
            dir.path(),
            r#"{ "proactiveSkillSubagents": { "minReferences": 1, "maxRecommendations": 5 } }"#,
        )
        .await;
        assert!(
            text.contains("- solo-skill via reviewer (referenced by 1 configured agents/chains; agent:solo)"),
            "`minReferences: 1` from config.json must admit a once-referenced skill that the \
             default of 2 excludes:\n{text}"
        );

        // --- `maxRecommendations` -------------------------------------------------------------
        // Two skills clear the default `minReferences: 2`; a cap of 1 must render exactly one row.
        let text = list_with_config(
            dir.path(),
            r#"{ "proactiveSkillSubagents": { "maxRecommendations": 1 } }"#,
        )
        .await;
        let rows = recommendation_rows(&text);
        assert_eq!(
            rows.len(),
            1,
            "`maxRecommendations: 1` caps the block at one row (`slice(0, maxRecommendations)`, \
             `proactive-skills.ts:156`); got {rows:#?}\n{text}"
        );
        assert!(
            rows[0].starts_with("- audit-trail via reviewer"),
            "the surviving row is the highest-referenced, name-ascending first:\n{text}"
        );

        // --- `preferredAgent` -----------------------------------------------------------------
        // `chooseRecommendationAgent` (`proactive-skills.ts:92-99`) prefers a configured, enabled
        // agent over the `reviewer` default every other case above renders.
        let text = list_with_config(
            dir.path(),
            r#"{ "proactiveSkillSubagents": { "preferredAgent": "solo" } }"#,
        )
        .await;
        assert!(
            text.contains("- audit-trail via solo (referenced by 2 configured agents/chains;"),
            "`preferredAgent: \"solo\"` must carry every recommendation instead of `reviewer`:\n{text}"
        );
        assert!(
            !text.contains(" via reviewer "),
            "no row may keep the default carrier once `preferredAgent` is configured:\n{text}"
        );

        // --- `positiveInteger` rejection ------------------------------------------------------
        // `0` is not a positive integer, so pi's guard (`proactive-skills.ts:32-36,50,53`) discards
        // it and the DEFAULTS apply — `minReferences` is 2 again, not 0.
        let text = list_with_config(
            dir.path(),
            r#"{ "proactiveSkillSubagents": { "minReferences": 0 } }"#,
        )
        .await;
        assert!(
            !text.contains("- solo-skill via"),
            "`minReferences: 0` must be rejected by `positiveInteger` and fall back to the \
             default of 2, which excludes the once-referenced `solo-skill`:\n{text}"
        );
        assert!(
            text.contains("- audit-trail via reviewer (referenced by 2 configured agents/chains;"),
            "...while the twice-referenced skill still clears the restored default:\n{text}"
        );
    }

    /// R-SA-055 (SAFETY-CRITICAL), end to end through the full slash-command dispatch path:
    /// `/run-chain` must reject on a blocked depth ceiling BEFORE `resolve_chain`'s own real
    /// discovery filesystem scan ever runs. Proven the same "same unresolvable name, which error
    /// wins" way as the foreground/background tests above — no chain named `"ghost-chain"` is
    /// ever written to `dir`, so if the depth guard did NOT run first, this call would surface
    /// [`SubagentError::ChainNotFound`] (discovery's own genuine failure mode for an unresolvable
    /// name) instead of [`SubagentError::DepthExceeded`].
    #[tokio::test]
    async fn dispatch_slash_run_chain_rejects_on_depth_before_chain_discovery_ever_runs() {
        let cfg = SubagentExtensionConfig {
            max_subagent_depth: 0,
            ..SubagentExtensionConfig::default()
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(cfg, dir.path().to_path_buf());

        let err = ext
            .dispatch_slash(
                SlashCommandName::RunChain,
                "ghost-chain -- do something",
                dir.path(),
                false,
            )
            .await
            .expect_err("a blocked depth ceiling must reject before chain discovery runs");

        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded ahead of resolve_chain's own ChainNotFound, got: {err:?}"
        );
    }

    /// The `/chain` and `/parallel` shared tail ([`SubagentsExtension::run_or_background_chain`])
    /// must likewise reject on a blocked depth ceiling before its own fork-context resolution (and
    /// therefore before either `run_chain_foreground`'s or `spawn_background_steps`' own
    /// independent, necessarily-later re-check) — proven directly against that private tail
    /// (accessible from this same-file `tests` submodule) with both `background: false` and
    /// `background: true`, since both branches share the identical guard at the top of the
    /// function, before the `if background` split.
    #[tokio::test]
    async fn run_or_background_chain_rejects_on_depth_before_fork_context_resolution() {
        let cfg = SubagentExtensionConfig {
            max_subagent_depth: 0,
            ..SubagentExtensionConfig::default()
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(cfg, dir.path().to_path_buf());

        let graph = vec![RunnerStep::SingleStep(crate::spawn::chain_graph::SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: "worker".to_string(),
            task: "do something".to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];

        for background in [false, true] {
            let err = ext
                .run_or_background_chain(
                    dir.path(),
                    graph.clone(),
                    RunMode::Chain,
                    Some(ContextMode::Fresh),
                    background,
                    None,
                )
                .await
                .expect_err("a blocked depth ceiling must reject before any dispatch");
            assert!(
                matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
                "background={background}: expected DepthExceeded, got: {err:?}"
            );
        }
    }

    /// G92: `/subagents-fleet` reaches the SAME renderer as the tool call above (pi routes both
    /// through `runSlashSubagent(… { action: "status", view: "fleet" })`), so the two surfaces
    /// cannot drift. Pre-fix the command did not exist and `dispatch_slash` had no arm for it.
    #[tokio::test]
    async fn subagents_fleet_slash_command_renders_the_same_fleet_surface() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_running_run(dir.path(), "slashfleet01", &["scout"]);
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig::default(),
            dir.path().to_path_buf(),
        );
        assert_eq!(
            SlashCommandName::from_str_exact("subagents-fleet"),
            Some(SlashCommandName::SubagentsFleet),
            "the command must be registrable by the name the user types"
        );
        let text = ext
            .dispatch_slash(SlashCommandName::SubagentsFleet, "", dir.path(), false)
            .await
            .expect("/subagents-fleet must render");
        assert!(text.starts_with("Subagent fleet: 1 active"), "{text}");
        assert!(text.contains("- slashfleet01 | running"), "{text}");
    }

}
