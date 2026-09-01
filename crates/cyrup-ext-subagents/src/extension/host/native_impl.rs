//! The [`NativeExtension`] impl itself: id/init/on_event/execute_command and the call/result
//! renderers.

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::ExtensionId;
use cyrup_ext::{ExtError, HookOutcome, HostEvent};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};

use crate::registration::slash_commands::{SlashCommandName, SLASH_COMMANDS};
use crate::extension::TOOL_NAME;
use crate::extension::host::SubagentsExtension;
use crate::extension::host::registration::RegistrationMode;
use crate::extension::tool::SubagentTool;
use crate::extension::tool::text::SUBAGENT_TOOL_DESCRIPTION;
use crate::extension::wait_tool::WaitTool;


#[async_trait]
impl NativeExtension for SubagentsExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Ambient (SEAM-071/SEAM-074): upstream pi-subagents is an installed package in the PATH tier
    /// that `noExtensions` collapses (`resource-loader.ts:451-453` @v0.83.0). A subagent CHILD still
    /// keeps it — pi's launcher re-injects it by path (`pi-subagents/src/runs/shared/pi-args.ts:413-417`
    /// @v0.47.1) — which is why `SUBAGENT_CHILD_RUNTIME_NATIVES` in cyrup-session-svc's builder
    /// carves it back in rather than this flag being the whole answer.
    fn is_ambient(&self) -> bool {
        true
    }

    /// Register the extension surface for this process's [`RegistrationMode`] (T6 child-mode gate):
    ///
    /// - [`RegistrationMode::Full`] (root orchestrator): the `subagent` tool (R-SA-128), all 12
    ///   slash commands (R-SA-129), and the session-lifecycle subscriptions (func-SA §5.6).
    /// - [`RegistrationMode::ChildSafe`] (fanout-authorized child, pi `fanout-child.ts`): ONLY the
    ///   restricted, mutation-blocked `subagent` tool — no slash commands, and no lifecycle
    ///   subscriptions, so `on_event`'s background-completion watcher + startup housekeeping never
    ///   install in a child.
    ///
    /// A plain (non-fanout) child never reaches `init` at all: the binary's `subagent_extension_for_env`
    /// gate returns `None`, so no extension is attached (pi `extension/index.ts:243-245` registers nothing).
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        match self.mode {
            RegistrationMode::ChildSafe => {
                api.register_tool(Arc::new(SubagentTool::new_child_safe(
                    self.executor.clone(),
                    self.cwd.clone(),
                )));
                // No commands, no subscriptions: a child installs no orchestrator UI/watcher surface.
                // A fanout-authorized child also runs none of the Full arm's startup housekeeping
                // below — pi's own `fanout-child.ts` entry point likewise never calls
                // `ensureAccessibleDir`/the cleanup sweeps at all.
                //
                // pi `startNestedControlInboxListener(pi, state)` (`fanout-child.ts:171`): started
                // AFTER the restricted tool registers, so a grandparent orchestrator's interrupt/
                // resume request targeting a run nested inside THIS child is serviced rather than
                // rotting unread in the controls inbox.
                self.executor.start_nested_control_inbox_listener();
            }
            RegistrationMode::Full => {
                // T6 startup housekeeping (pi `extension/index.ts:257-264`), run ONCE here at
                // extension load — BEFORE any tool/command/subscription registration, exactly
                // mirroring pi's registration function body, where `ensureAccessibleDir(RESULTS_DIR)`/
                // `ensureAccessibleDir(ASYNC_DIR)` run at the very top and THROW on a persistent
                // failure, aborting the whole registration before `pi.registerTool(tool)` is ever
                // reached. A persistent failure here likewise fails `init()` outright
                // (`ExtError::Component`) rather than silently degrading (the pre-fix behavior) every
                // session this process ever starts to "no completion notifications" — this crate's
                // own [`crate::background::ensure_accessible_dir`] doc comment names the exact
                // Windows/Azure-AD null-DACL scenario this guards. `cleanup_old_chain_dirs`/
                // `cleanup_all_artifact_dirs` are pi's own once-per-load sweeps (`extension/index.ts:329,339`),
                // NOT a per-`session_start` concern — moved here so they run exactly once per process
                // load rather than re-running (redundantly, if harmlessly throttled) on every session.
                let artifact_roots = crate::background::run_artifact_roots_in(
                    &self.executor.config_snapshot().await.roots,
                    &self.cwd,
                );
                crate::background::ensure_accessible_dir(&artifact_roots.async_root)
                    .await
                    .map_err(|e| {
                        ExtError::Component(format!(
                            "subagents: async root {} is not accessible: {e}",
                            artifact_roots.async_root.display()
                        ))
                    })?;
                crate::background::ensure_accessible_dir(&artifact_roots.results_dir)
                    .await
                    .map_err(|e| {
                        ExtError::Component(format!(
                            "subagents: results dir {} is not accessible: {e}",
                            artifact_roots.results_dir.display()
                        ))
                    })?;
                crate::artifacts::cleanup_old_chain_dirs(&self.cwd);
                // SUBA-059 / pi `const artifactCleanupDays = config.artifactConfig?.cleanupDays ??
                // DEFAULT_ARTIFACT_CONFIG.cleanupDays; cleanupAllArtifactDirs(artifactCleanupDays);`
                // (`extension/index.ts:369-370` @v0.47.1). This was the hardcoded 7-day constant, so
                // a user who wanted subagent transcripts kept for audit — or deleted sooner, or not
                // swept at all — had no way to say so, and every extension load silently deleted run
                // inputs, outputs and JSONL older than a week.
                crate::artifacts::cleanup_all_artifact_dirs(
                    &self.cwd,
                    self.executor.config_snapshot().await.artifact_cleanup_days(),
                );

                // SUBA-025 / pi `description: buildSubagentToolDescription(config)`
                // (`extension/index.ts:458` @v0.34.0, `:540` @v0.43.0): the advertised description
                // is RESOLVED from config at registration, not a constant picked by registration
                // mode. Three surfaces ride on this — `toolDescriptionMode: "compact"` to trim the
                // (long) full text out of every request's context, a project/user
                // `subagent-tool-description.md` to steer the orchestrator with deployment-specific
                // text, and `withMandatorySafetyGuidance`, which makes that override incapable of
                // dropping the safety block. Only the Full arm resolves: upstream's fanout child
                // builds its own literal (`extension/fanout-child.ts:159` @v0.34.0) and never calls
                // `buildSubagentToolDescription`, so the ChildSafe arm above must not either.
                let mut description_warnings = Vec::new();
                let resolved_description =
                    crate::registration::tool_description::build_subagent_tool_description(
                        self.executor
                            .config_snapshot()
                            .await
                            .tool_description_mode
                            .as_ref(),
                        SUBAGENT_TOOL_DESCRIPTION,
                        &crate::registration::tool_description::ToolDescriptionOptions::new(
                            self.cwd.clone(),
                        ),
                        &mut description_warnings,
                    );
                for warning in description_warnings {
                    // pi `console.warn("[pi-subagents] " + message)` (`tool-description.ts:94`),
                    // under this crate's own product prefix.
                    tracing::warn!("[cyrup-subagents] {warning}");
                }

                api.register_tool(Arc::new(
                    SubagentTool::new(self.executor.clone(), self.cwd.clone())
                        .with_watchdog(Arc::clone(&self.watchdog))
                        .with_description(resolved_description),
                ));

                // SUBA-004 (pi `extension/index.ts:519-527`): the `wait` tool registers alongside
                // `subagent`, in the Full arm only. Without it an orchestrator has NO way to block
                // on a background run — it can only end its turn and hope a completion notification
                // arrives, which is impossible in a skill that must run to completion or in a
                // single-turn `cyrup -p …` invocation. Registered even when configured off (pi does
                // the same): the disabled tool returns immediately with an explanation, so the model
                // is told why nothing was waited on instead of the tool silently vanishing.
                let wait_enabled = WaitTool::resolve_enabled(&self.executor).await;
                api.register_tool(Arc::new(WaitTool::new(
                    self.executor.clone(),
                    self.cwd.clone(),
                    wait_enabled,
                )));

                // G106 (pi `createNativeSupervisorChannel`'s `registerParentTools`,
                // `native-supervisor-channel.ts:635-638`): the PARENT half of the native supervisor
                // channel. Without it a child that blocks on `contact_supervisor` has nobody to
                // answer it unless the orchestrator happens to have opted into `cyrup-intercom` AND
                // holds a live broker presence — which a plain session never does
                // (`cyrup_intercom::is_installed` gates a non-child session on `CYRUP_INTERCOM` or an
                // `intercom/config.json`). Registered in the Full arm only, matching upstream: the
                // channel's parent tools are registered from `start()`, which only a session-start
                // subscriber reaches, and `ChildSafe` subscribes to nothing.
                api.register_tool(Arc::new(
                    crate::native_supervisor::SubagentSupervisorTool::new(
                        self.supervisor_channel.clone(),
                    ),
                ));

                // G106, upstream's SECOND parent registration (`:637`): the same channel under the
                // bare name `intercom`, guarded by `!hasTool(pi, "intercom")`. `InitApi` has no
                // tool-registry query, so the precedence is decided from the signal that says
                // whether `cyrup-intercom` will attach and own the name — see
                // [`crate::native_supervisor::native_intercom_alias_should_register`].
                //
                // It is not decoration. `intercom` is the name pi-intercom uses, the name the
                // child-side bridge instruction names, and the name every prompt and skill that
                // predates the native channel reaches for; on an orchestrator that never installed
                // intercom — precisely the one this channel exists for — that name resolved to no
                // tool at all.
                // Both gates read the environment through the crate's injectable resolver, so
                // `config.env_overrides` can pin (or scrub) what they see without this process
                // mutating anything global. With no overrides this is byte-for-byte the previous
                // `std::env::var(k).ok()`.
                let env = self.env_lookup();
                if crate::native_supervisor::native_intercom_alias_should_register(
                    &env,
                    &crate::native_supervisor::intercom_agent_dir_from(
                        &env,
                        Some(self.cwd.clone()),
                    ),
                ) {
                    api.register_tool(Arc::new(
                        crate::native_supervisor::SubagentSupervisorTool::new_intercom_alias(
                            self.supervisor_channel.clone(),
                        ),
                    ));
                }

                // C20 / EXT-006: this extension draws its OWN `subagent` tool rows. pi declares the
                // same thing as `renderCall`/`renderResult` members of its `ToolDefinition`
                // (`extension/index.ts:547,569` @v0.43.0); cyrup's native tools are already-
                // executable `Arc<dyn Tool>` values with no descriptor, so the declaration goes
                // through `InitApi` instead (`cyrup-ext/src/native.rs:277`). Without this the host's
                // `has_tool_renderer("subagent")` pre-check short-circuits and
                // `NativeExtension::render_call`/`render_result` are never called at all — which is
                // why `tui::events::render_inline_result` had no non-test caller.
                //
                // Full arm ONLY, matching upstream: `fanout-child.ts`'s restricted `ToolDefinition`
                // (`:156-168`) deliberately declares NEITHER renderer.
                api.register_tool_renderer(TOOL_NAME);

                for cmd in SLASH_COMMANDS {
                    api.register_command(
                        cmd.name.as_str(),
                        cyrup_ext::registry::CommandDescriptor {
                            description: cmd.description.to_string(),
                            completions: Vec::new(),
                        },
                    );
                }

                // pi `registerMainWatchdog`'s own two registrations (`watchdog/register-main.ts:392-409`):
                // the `/subagents-watchdog` command and the renderer for its warning message. Both
                // in the Full arm only — a `ChildSafe` child registers no orchestrator UI at all,
                // and its own watchdog role is `register_child`'s, not this one's.
                api.register_command(
                    crate::watchdog::register_main::WATCHDOG_COMMAND_NAME,
                    cyrup_ext::registry::CommandDescriptor {
                        description: crate::watchdog::register_main::WATCHDOG_COMMAND_DESCRIPTION
                            .to_string(),
                        completions: Vec::new(),
                    },
                );
                api.register_message_renderer(
                    crate::watchdog::types::SUBAGENT_WATCHDOG_WARNING_TYPE,
                );

                api.subscribe(&[
                    cyrup_ext::EventKind::SessionStart,
                    cyrup_ext::EventKind::SessionShutdown,
                    // pi `register-main.ts:411-433` — the watchdog's own seven lifecycle
                    // subscriptions, on top of the two this extension already had. Without them the
                    // runtime is constructed and never fed: no turn deltas buffer, no boundary
                    // review fires, and `/subagents-watchdog status` reports an eternally idle
                    // machine. `session_before_compact` is deliberately absent — upstream subscribes
                    // to `session_compact` (`:433`), the AFTER edge.
                    cyrup_ext::EventKind::BeforeAgentStart,
                    cyrup_ext::EventKind::TurnEnd,
                    cyrup_ext::EventKind::ToolResult,
                    cyrup_ext::EventKind::AgentEnd,
                    cyrup_ext::EventKind::SessionBeforeSwitch,
                    cyrup_ext::EventKind::SessionBeforeFork,
                    cyrup_ext::EventKind::SessionCompact,
                    // R-SA-132/134 — the packaged-resources contribution. Upstream declares it
                    // statically in `package.json`'s `pi` block (`"skills": ["./skills"]`,
                    // `"prompts": ["./prompts"]`, `pi-subagents/package.json:52-62` @v0.34.0), which
                    // pi's package manager reads when the extension package is installed. cyrup's
                    // subagents extension is a NATIVE built-in with no package.json, so the same
                    // declaration has to travel the extension seam instead: `resources_discover`
                    // (R-09-022), whose aggregate `cyrup-session-svc`'s builder folds into the
                    // discovered resource registry BEFORE the skill pointers and system prompt are
                    // derived (`builder.rs:975-1002`). Without this subscription the bundled
                    // `skills/pi-subagents/SKILL.md` — 58 KB of shipped operational guidance — was
                    // never registered anywhere and `bundled_skill_files()` had no non-test caller.
                    cyrup_ext::EventKind::ResourcesDiscover,
                ]);
            }
        }
        Ok(())
    }

    /// Session lifecycle handling (func-SA §5.6): on `SessionStart`, resume tracking any
    /// background runs still recorded on disk from a prior process (R-SA-093); on
    /// `SessionShutdown`, mirror pi's own teardown (`extension/index.ts:644-680`) for every piece
    /// this crate has a live analog of — stop the completion watcher (pi `stopResultWatcher()`),
    /// abort+clear the job tracker's poll loop and in-memory job map (pi `clearInterval(state.poller)`
    /// + `state.asyncJobs.clear()`), and clear the captured parent-session anchor (pi `delete
    /// process.env[SUBAGENT_PARENT_SESSION_ENV]`). Pieces pi's teardown also touches that this crate
    /// has no live analog for yet are deliberately left alone here: pi's `pendingForegroundControlNotices`/
    /// `cleanupTimers`/slash-snapshot state and its two slash-invoked-run bridges
    /// (`slashBridge`/`promptTemplateBridge`, whose `cancelAll()` aborts in-flight slash-dispatched
    /// runs) have no ported equivalent in this crate (slash dispatch here is a direct in-process call
    /// via `dispatch_slash`, R-SA-130, not an event-bus bridge with its own cancellable in-flight
    /// registry); pi's `ui.setWidget(WIDGET_KEY, undefined)` has no analog since this crate renders no
    /// persistent host-UI widget. None of this omitted state affects whether a detached background
    /// run survives shutdown — a detached run MUST continue to completion even after the
    /// orchestrating process exits (R-SA-071/DI-SA-8), and nothing here sends it any signal.
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionStart { .. } => {
                // T6's once-per-load housekeeping (`ensureAccessibleDir`/`cleanupOldChainDirs`/
                // `cleanupAllArtifactDirs`) now runs in `init()`, above — matching pi's own
                // registration-time closure body exactly (`extension/index.ts:257-264` runs once,
                // NOT per `session_start`). What DOES belong here, per-session, is pi's OWN
                // `session_start` handler body (`extension/index.ts:628-642`): the per-session-file
                // artifact sweep (`cleanupOldArtifacts(getArtifactsDir(sessionFile))`,
                // `resetSessionState`'s `cleanupSessionArtifacts` at `extension/index.ts:684-693`),
                // best-effort — a failure here must never block a session from starting.
                if let Some(session_file) =
                    self.executor.host_services().and_then(|s| s.session_file())
                {
                    let cfg = self.executor.config_snapshot().await;
                    // SUBA-048 / pi `getArtifactsDir(sessionFile, undefined,
                    // state.artifactDirPreference)` — the per-session sweep must look where the
                    // configured preference actually WRITES, or `"artifactDir": "temp"` would leave
                    // its own artifacts unswept forever.
                    let artifacts_dir = crate::artifacts::resolve_artifacts_dir(
                        Some(&session_file),
                        None,
                        &ctx.cwd,
                        cfg.artifact_dir_preference(),
                    );
                    // SUBA-059: the per-session sweep honours the same configured retention the
                    // once-per-load sweep does (pi passes the resolved `artifactCleanupDays` to both,
                    // `extension/index.ts:370,684-693`).
                    crate::artifacts::cleanup_old_artifacts(
                        &artifacts_dir,
                        cfg.artifact_cleanup_days(),
                    );
                }

                // R-SA-P1 (port doc §4 P-4): capture the canonical parent-session anchor ONCE from
                // the live session id (P-2) at the root orchestrator's SessionStart (depth 0 — a
                // `ChildSafe` child never subscribes to SessionStart, so this arm only runs for the
                // root). Every child this session spawns then inherits it via the spawn env overlay,
                // so the permission companion's child→parent ask-forwarding spool can address this
                // session's inbox.
                self.executor.capture_parent_session_anchor();

                // pi `resetSessionState`'s `state.subagentSpawns = { sessionId: state.currentSessionId,
                // count: 0 }` (`extension/index.ts:695-803`): a new session always starts with a fresh
                // per-session spawn budget. Ordered AFTER the anchor capture so the budget is stamped
                // with THIS session's id.
                self.executor.reset_spawn_budget();

                // G106 (pi `extension/index.ts:757` `supervisorChannel.start()`): bind the live
                // capability backend — the channel needs `session_id()` to decide which pending
                // requests belong to THIS orchestrator, and `inject_message` to surface them — then
                // start the poll loop. Idempotent across a session rebuild.
                if let Some(services) = self.executor.host_services() {
                    self.supervisor_channel.bind_services(services);
                }
                self.supervisor_channel.start();

                // pi `register-main.ts:411-414` — `session_start` binds the watchdog to this
                // session: new cwd, session overrides dropped, everything reset.
                self.watchdog.bind_session(&ctx.cwd);

                self.executor.resume_tracking(&ctx.cwd).await;
                // C6: install the background-completion watcher (notify.ts / result-watcher.ts) so a
                // detached run that finishes during this session surfaces its `subagent-notify`
                // message (with `triggerTurn`) and has its result file deleted (R-SA-099/101). When the
                // P-1 host-services slot is bound this installs the live turn-injecting
                // `HostServicesCompletionSink` (R-SA-101); otherwise the stderr LoggingCompletionSink.
                self.executor.install_completion_watcher(&ctx.cwd).await;

                // pi `fleetStatus.setContext(ctx)` (`tui/fleet-status.ts:271-288`): arm the
                // always-on fleet status widget for this session and paint it once. See
                // [`Self::refresh_fleet_status_widget`] for why the tick rides host event edges
                // rather than upstream's 500 ms interval.
                self.refresh_fleet_status_widget(&ctx.cwd, ctx.has_ui).await;
            }
            // pi's `agent_end` handler (`extension/index.ts:585-601` @v0.43.0). Its first line
            // (`drainOutstandingWork` when there is no UI) belongs to the background-drain
            // subsystem, not to missions; the goal-mission scan below is the rest of that handler.
            HostEvent::AgentEnd { .. } => {
                // ORDER IS UPSTREAM'S. pi registers two `agent_end` handlers for this extension and
                // its runner awaits them one at a time in REGISTRATION order
                // (`coding-agent/src/core/extensions/runner.ts:805-811` — `for (const handler of
                // handlers) { await handler(event, ctx) }`). `registerMainWatchdog(pi)` runs at
                // `extension/index.ts:375`, the goal-mission handler registers at `:583`, so the
                // watchdog's boundary review completes BEFORE any goal continuation notice is
                // raised. That is observable: the review is awaited, can block for
                // `agentEndTimeoutMs`, and can inject a warning or steer message — so running the
                // goal scan first interleaves the two injections the other way round.
                //
                // pi `register-main.ts:427-430` — AWAITED (upstream RETURNS the promise from its
                // handler, so pi's runner awaits it too).
                self.watchdog.handle_agent_end(&ctx.cwd).await;
                // pi's `agent_end` goal-mission handler (`extension/index.ts:585-601`).
                let _ = self.executor.raise_goal_continuation_notices(&ctx.cwd).await;
                // The fleet status widget's repaint edge (pi's 500 ms `setInterval` tick) — not a
                // registered handler, so its position here is free.
                self.refresh_fleet_status_widget(&ctx.cwd, ctx.has_ui).await;
            }
            HostEvent::SessionShutdown { .. } => {
                // pi `runtimeCleanup`/`session_shutdown` both call `supervisorChannel.dispose()`
                // (`extension/index.ts:412-430`): stop the poller and drop the pending map, so a
                // rebuilt session never re-surfaces the previous session's requests.
                self.supervisor_channel.dispose();
                // pi `runtimeCleanup`'s `mainWatchdog.dispose()` (`extension/index.ts:416`) and
                // `register-main.ts:434-437`'s own `session_shutdown` handler.
                self.watchdog.dispose();
                self.executor.teardown_session().await;
                // pi `fleetStatus.dispose()` — clear the widget and drop every piece of
                // registration state (`tui/fleet-status.ts:290-299,533-563`).
                if let Ok(mut widget) = self.fleet_status.lock() {
                    widget.set_ui_available(false);
                }
                if let Some(services) = self.executor.host_services() {
                    // EXT-047: upstream's `setWidget(key, undefined)` is a REMOVAL. The old
                    // hand-rolled `{"key": …, "content": null}` blob could not express one, so the
                    // slot stayed occupied after dispose.
                    services.set_widget(
                        crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY,
                        None,
                        cyrup_ext::host::WidgetPlacement::default(),
                    );
                }
            }
            // pi `register-main.ts:415-418`.
            HostEvent::BeforeAgentStart { prompt, system_prompt, .. } => {
                self.watchdog.handle_before_agent_start(
                    &serde_json::json!({ "prompt": prompt, "systemPrompt": system_prompt }),
                    &ctx.cwd,
                );
            }
            // pi `register-main.ts:419-422`. The event is re-shaped into the `{type:"turn_end",
            // message, toolResults}` object `formatWatchdogTurnDelta`/`eventIndicatesRepoEdit`
            // duck-type against — the same JSON pi's own handler receives.
            HostEvent::TurnEnd { message, tool_results, .. } => {
                let event = crate::watchdog::turn_delta::watchdog_turn_end_event(
                    message,
                    tool_results,
                );
                self.watchdog.handle_turn_end(&event, &ctx.cwd);
            }
            // pi `register-main.ts:423-426` — the mid-run cadence trigger.
            HostEvent::ToolResult { .. } => {
                self.watchdog.handle_tool_result(&ctx.cwd);
            }
            // pi `register-main.ts:431-432` — a switch or a fork abandons this session's review
            // state entirely, scope artifact and auto-follow counters included.
            HostEvent::SessionBeforeSwitch { .. } | HostEvent::SessionBeforeFork { .. } => {
                self.watchdog.reset(crate::watchdog::runtime::WatchdogResetOptions {
                    clear_review_input_signature: true,
                    clear_lsp_ledger: true,
                    clear_scope: true,
                    reset_auto_follow: true,
                    ..crate::watchdog::runtime::WatchdogResetOptions::default()
                });
            }
            // pi `register-main.ts:433` — a compaction rewrote the history the scope record was
            // built from, so the scope goes; the auto-follow counters and the review-input hash do
            // NOT (upstream passes only `clearScope`).
            HostEvent::SessionCompact { .. } => {
                self.watchdog.reset(crate::watchdog::runtime::WatchdogResetOptions {
                    clear_scope: true,
                    ..crate::watchdog::runtime::WatchdogResetOptions::default()
                });
            }
            // R-SA-132/134: contribute this crate's BUNDLED packaged resources — the
            // `skills/pi-subagents/SKILL.md` operational skill and the seven `prompts/*.md`
            // recipes — exactly the two entries upstream's `package.json` `pi` block declares
            // (`"skills": ["./skills"]`, `"prompts": ["./prompts"]`,
            // `pi-subagents/package.json:56-61` @v0.34.0).
            //
            // The host CONCATENATES every extension's contribution and loads each at the
            // `Discovered` scope (`cyrup_resources::ResourceRegistry::extend`), so a same-named
            // user/project/package resource still wins — the bundled skill is a floor, never an
            // override. A contribution of nothing at all returns `Noop`, which leaves the
            // discovered registry untouched (the host's own early return, `builder.rs:997`).
            HostEvent::ResourcesDiscover { .. } => {
                let skill_paths: Vec<String> = crate::registration::resources::bundled_skill_files()
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                let prompt_paths: Vec<String> =
                    crate::registration::resources::bundled_prompt_files()
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect();
                if skill_paths.is_empty() && prompt_paths.is_empty() {
                    return HookOutcome::Noop;
                }
                return HookOutcome::Handled(cyrup_ext::HandledValue(serde_json::json!({
                    "skillPaths": skill_paths,
                    "promptPaths": prompt_paths,
                })));
            }
            _ => {}
        }
        HookOutcome::Noop
    }

    /// Dispatch a registered slash command through the SAME executor the `subagent` tool uses
    /// (R-SA-130: "a direct in-process function call" — this native extension has no
    /// module-decoupling boundary to bridge, unlike pi-subagents' own event-bus slash-bridge).
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        ctx.require_command_tier()?;

        // pi `register-main.ts:403-409` registers `/subagents-watchdog` as its OWN command, separate
        // from this crate's twelve `SLASH_COMMANDS`, so it routes before the table lookup.
        if name == crate::watchdog::register_main::WATCHDOG_COMMAND_NAME {
            return Ok(self.execute_watchdog_command(args, ctx));
        }

        let Some(command) = SlashCommandName::from_str_exact(name) else {
            return Err(ExtError::Component(format!(
                "native extension has no handler for command `{name}`"
            )));
        };

        let output = self
            .dispatch_slash(command, args, &ctx.cwd, ctx.has_ui)
            .await
            .unwrap_or_else(|err| format!("subagent command failed: {err}"));

        Ok(Some(output))
    }

    /// Late-bind the live capability backend (P-1, reconciliation §2 item 1). The session builder
    /// calls this via `load_native_with_services` (facade.rs:181) BEFORE `init`; stash the shared
    /// `Arc` in the executor's slot so the `SessionStart` anchor capture (R-SA-P1), the fork-context
    /// resolver (blocker #4), and the completion watcher's turn-injecting sink (R-SA-101) all reach
    /// the live session id/file + `inject_message` from OUTSIDE any `HostCtx`. Idempotent.
    fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        self.executor.set_host_services(services);
    }

    /// Draw the `subagent` tool's CALL row — a 1:1 port of pi's `renderCall`
    /// (`extension/index.ts:548-568` @v0.43.0), on the raw tool arguments the host hands over
    /// (`AgentSessionEvent::ToolExecutionStart.args`).
    ///
    /// Reached by: the model issues a `subagent` tool call → `cyrup-tui`'s `extension_render`
    /// resolves this extension for the tool name and calls here (`cyrup-tui/src/app.rs:4283,4295`).
    fn render_call(&self, key: &str, call: &serde_json::Value) -> Option<serde_json::Value> {
        // pi `pi.registerMessageRenderer(SUBAGENT_WATCHDOG_WARNING_TYPE, …)`
        // (`register-main.ts:392-401`). `render_call` carries BOTH surfaces: `key` is a tool name
        // for a tool renderer and a customType for a message renderer (`cyrup-ext/src/native.rs:402-404`).
        if key == crate::watchdog::types::SUBAGENT_WATCHDOG_WARNING_TYPE {
            return crate::watchdog::register_main::render_watchdog_warning_message(call);
        }
        if key != TOOL_NAME {
            return None;
        }
        Some(serde_json::Value::String(render_subagent_call(call)))
    }

    /// Draw the `subagent` tool's RESULT row — pi's `renderResult` (`extension/index.ts:569-576`),
    /// which delegates to `renderSubagentResult` (`tui/render.ts:1678`).
    ///
    /// The host hands over the whole `AgentToolResult` (`{content, details, terminate}`,
    /// `cyrup-agent/src/agent.rs:123-142`), which is exactly pi's `renderResult(result, …)`
    /// argument, so the two branches port directly:
    ///
    /// * `!d || !d.results.length` (`:1413-1423`) — an async start, a management action, or any
    ///   result with no settled run: draw the content text with pi's `[fork]` prefix;
    /// * `d.mode === "single" && d.results.length === 1` (`:1428-1430`) — the compact settled row,
    ///   via [`crate::tui::events::render_inline_result`], which reuses the same header/stat
    ///   primitives `tui::render` already owns.
    ///
    /// EXPANDED rendering (pi's `options.expanded` arm, `:1431-1500`) is NOT reachable here: the
    /// host's renderer contract passes no expansion state (`ExtensionHost::render_tool_result`
    /// takes only the payload), so this always draws pi's COMPACT tier — the tier a collapsed row
    /// shows, which is what the transcript draws by default.
    fn render_result(&self, key: &str, result: &serde_json::Value) -> Option<serde_json::Value> {
        if key != TOOL_NAME {
            return None;
        }
        Some(render_subagent_result(result))
    }
}

/// pi `renderCall` (`extension/index.ts:548-568` @v0.43.0), rendered as plain text: cyrup's
/// renderer contract returns a serialized widget tree the host flattens, and pi's own return here
/// is a single `Text` node in every branch.
fn render_subagent_call(args: &serde_json::Value) -> String {
    let string_field = |key: &str| args.get(key).and_then(serde_json::Value::as_str).unwrap_or("");
    // `:466-472` — a management/control action names its target when it has one.
    let action = string_field("action");
    if !action.is_empty() {
        let target = match string_field("agent") {
            "" => string_field("chainName"),
            agent => agent,
        };
        return if target.is_empty() {
            format!("subagent {action}")
        } else {
            format!("subagent {action} {target}")
        };
    }
    let array_len =
        |key: &str| args.get(key).and_then(serde_json::Value::as_array).map_or(0, Vec::len);
    // `:475` — the `[async]` badge, suppressed while clarifying.
    let async_label = if args.get("async") == Some(&serde_json::Value::Bool(true))
        && args.get("clarify") != Some(&serde_json::Value::Bool(true))
    {
        " [async]"
    } else {
        ""
    };
    // `:476-481` — a chain names its LENGTH, not its steps.
    let chain_len = array_len("chain");
    if chain_len > 0 {
        return format!("subagent chain ({chain_len}){async_label}");
    }
    // `:473-474,482-487` — a parallel fan-out names its EFFECTIVE task count, which is
    // `effectiveParallelTaskCount` (`:447-453`): each task's integer `count >= 1`, else 1.
    if array_len("tasks") > 0 {
        return format!(
            "subagent parallel ({}){async_label}",
            effective_parallel_task_count(args)
        );
    }
    // `:488-492` — a single run names its persona, `?` when none was given.
    let agent = match string_field("agent") {
        "" => "?",
        agent => agent,
    };
    format!("subagent {agent}{async_label}")
}

/// pi `effectiveParallelTaskCount` (`extension/index.ts:447-453` @v0.34.0): sum each task's
/// `count` when it is an integer `>= 1`, else 1 per task.
fn effective_parallel_task_count(args: &serde_json::Value) -> u64 {
    let Some(tasks) = args.get("tasks").and_then(serde_json::Value::as_array) else {
        return 0;
    };
    tasks
        .iter()
        .map(|task| {
            task.get("count")
                .and_then(serde_json::Value::as_u64)
                .filter(|n| *n >= 1)
                .unwrap_or(1)
        })
        .sum()
}

/// pi `renderSubagentResult` (`tui/render.ts:1678-1712` @v0.43.0), compact tier — see
/// [`NativeExtension::render_result`]'s doc for the branch map. Returns a JSON array of line
/// strings, which the host flattens newline-joined (`cyrup-tui/src/app.rs:4512`).
fn render_subagent_result(result: &serde_json::Value) -> serde_json::Value {
    let details = result.get("details");
    let payload = details
        .filter(|d| !d.is_null())
        .and_then(|d| serde_json::from_value::<crate::tui::events::SubagentUpdatePayload>(d.clone()).ok());

    // pi `:1413` — no details, or no settled run: the plain-text branch.
    let settled = payload.as_ref().filter(|p| !p.results.is_empty());
    let Some(payload) = settled else {
        // pi `:1414-1416`: the first text content block, `"(no output)"` when absent, prefixed with
        // `[fork]` when the (possibly unparsed) details declared a fork context.
        let text = result
            .get("content")
            .and_then(serde_json::Value::as_array)
            .and_then(|blocks| blocks.first())
            .filter(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .and_then(|b| b.get("text").and_then(serde_json::Value::as_str))
            .unwrap_or("(no output)");
        let prefix = if details.and_then(|d| d.get("context")).and_then(serde_json::Value::as_str)
            == Some("fork")
        {
            "[fork] "
        } else {
            ""
        };
        // pi wraps to the terminal width (`:1420`); cyrup's host owns wrapping, so the lines are
        // handed over unwrapped and the transcript wraps them.
        return serde_json::Value::Array(
            format!("{prefix}{text}")
                .lines()
                .map(|l| serde_json::Value::String(l.to_string()))
                .collect(),
        );
    };

    // pi `:1428-1430` — the compact settled row(s), through the shared render primitives.
    let lines = crate::tui::render::lines_to_plain_text(&crate::tui::events::render_inline_result(
        payload, 0,
    ));
    serde_json::Value::Array(lines.into_iter().map(serde_json::Value::String).collect())
}

/// Resolve one background run's nested descendants ONE level, by reading each nested run's own
/// `status.json` — pi's `nestedChildren` on an `AsyncRunSummary` (`runs/background/
/// async-status.ts:291`, rendered by `tui/fleet-status.ts:193,212`).
///
/// [`crate::background::StepStatus::nested_run_ids`] deliberately stores bare ids rather than
/// embedded snapshots (see its own doc), so a reader that wants the nested run's state has to go
/// to disk; [`crate::background::RunPaths::nested`] is the documented way to get there. This READS
/// only — no reconcile, no repair, no kill — which is the same read-only discipline
/// [`crate::tui::fleet::collect_fleet_history`] applies for pi's `reconcile: false`, and is why
/// this is not the recursive reconciliation `background/fleet_view.rs` declines.
///
/// A nested id whose `status.json` is missing or unparseable is skipped, never fatal.
pub(crate) async fn read_nested_children(
    paths: &crate::background::RunPaths,
    status: &crate::background::RunStatus,
) -> Vec<crate::tui::fleet_state::NestedRunView> {
    let mut out = Vec::new();
    for (step_index, step) in status.steps.iter().enumerate() {
        for nested_id in &step.nested_run_ids {
            let nested_paths = paths.nested(nested_id);
            let Ok(bytes) = tokio::fs::read(&nested_paths.status).await else { continue };
            let Ok(nested) =
                serde_json::from_slice::<crate::background::RunStatus>(&bytes)
            else {
                continue;
            };
            out.push(crate::tui::fleet_state::NestedRunView::from_run_status(
                nested_id.as_str(),
                &nested,
                Some(step_index),
            ));
        }
    }
    out
}
