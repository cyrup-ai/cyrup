//! The [`NativeExtension`] impl itself: id/init/on_event/execute_command and the call/result
//! renderers.

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::ExtensionId;
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, HookOutcome, HostEvent};

use crate::extension::TOOL_NAME;
use crate::extension::host::SubagentsExtension;
use crate::extension::host::registration::RegistrationMode;
use crate::extension::tool::SubagentTool;
use crate::extension::tool::text::SUBAGENT_TOOL_DESCRIPTION;
use crate::extension::wait_tool::WaitTool;
use crate::registration::slash_commands::{SLASH_COMMANDS, SlashCommandName};

/// How long the `SessionShutdown` handler waits for the herdr pane release AFTER the rest of the
/// teardown has run.
///
/// **This exists because the herdr release and the dispatcher's budget used to be the same five
/// seconds.** `crate::herdr::shutdown()` is bounded by `crate::herdr::reporter::RELEASE_TIMEOUT`
/// (5 s), the handler runs under [`cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET`] (5 s), and the
/// dispatcher enforces that budget by DROPPING the handler future (`Dispatcher::invoke_contained`).
/// So a herdr that accepted the connection and then stopped answering consumed the entire budget
/// on the arm's FIRST statement, and every disposal below it — the supervisor channel, the
/// watchdog, the wait subscriptions, the scheduled runs, `teardown_session`, both widget slots —
/// was silently dropped. Nothing logged, nothing failed: the statements simply never ran.
///
/// The arm now SPAWNS the release, runs the in-process teardown, and joins here. The release still
/// goes out first and is still awaited; what changed is that the waiting happens after the work
/// that cannot hang, and is bounded by this smaller slice. The `const` assertion below is the
/// coupling: the join must leave at least as much of the budget again for everything above it, so
/// shrinking [`cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET`] is a compile error here rather than a
/// teardown that quietly stops happening.
///
/// A join that times out does NOT abort the release — the task keeps draining on the runtime until
/// `RELEASE_TIMEOUT` ends it, which on the `/quit` path is inside `main`'s
/// `runtime.dispose().await` (`crates/cyrup/src/main.rs:729-731`).
const HERDR_RELEASE_JOIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

const _: () = assert!(
    HERDR_RELEASE_JOIN_BUDGET.as_millis() * 2
        < cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET.as_millis(),
    "the herdr release join must leave at least as much of the dispatch budget again for the rest \
     of the SessionShutdown teardown"
);

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
                    self.executor
                        .config_snapshot()
                        .await
                        .artifact_cleanup_days(),
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

                // PB-8: built ONCE, registered, and the SAME `Arc` stashed on the extension so
                // the RPC bridge dispatches into the instance the model uses — same resolved
                // description, same `allow_mutating_management`, same `DispatchGuard`. See
                // `SubagentsExtension::rpc_tool` for why this is not `subagent_tool()`.
                let subagent_tool = Arc::new(
                    SubagentTool::new(self.executor.clone(), self.cwd.clone())
                        .with_watchdog(Arc::clone(&self.watchdog))
                        .with_description(resolved_description),
                );
                let _ = self.rpc_tool.set(Arc::clone(&subagent_tool));
                api.register_tool(subagent_tool);

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

                // VL-S11 R3 — pi `if (options.foregroundDetachShortcut) pi.registerShortcut(…)`
                // (`slash/slash-commands.ts:1007-1012`): the SAME `/subagents-detach` handler,
                // reachable from a chord. Registered right after the command it shares a handler
                // with, exactly where upstream registers it (`:1002` then `:1007`), and in the
                // `Full` arm only — a `ChildSafe` fanout child registers no orchestrator UI, so it
                // binds no key either. `None` here is upstream's falsy config value: no chord.
                // The press lands at `Self::execute_shortcut` below.
                if let Some(key) = self.foreground_detach_shortcut() {
                    api.register_shortcut(
                        key,
                        Some(
                            crate::extension::host::shortcuts::FOREGROUND_DETACH_SHORTCUT_DESCRIPTION
                                .to_string(),
                        ),
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

                // PB-8 — pi `registerSubagentRpcBridge({ events: pi.events, … })`
                // (`extension/index.ts:759-764` @v0.68.0, impl `extension/rpc.ts:817-848`): the
                // ONE inter-extension topic this extension listens on, so a host, an editor or a
                // sibling extension can drive subagents programmatically instead of only the model
                // being able to. Deliveries land at `Self::on_bus_event` below.
                //
                // Full arm ONLY. A `ChildSafe` fanout child registers no orchestrator surface and
                // must not answer RPC — it would otherwise expose spawn/stop/manage on the bus
                // from inside a child.
                api.subscribe_bus(crate::extension::rpc::SUBAGENT_RPC_REQUEST_EVENT);

                // The herdr status bridge's two run-ended edges (`src/herdr/`). `subscribe_bus` is
                // `InitApi`-only (`cyrup-ext/src/native.rs:466`), so they are declared here even
                // though whether they are ACTED on is decided per-session by
                // `herdr::runtime::arm` — outside a herdr pane every delivery is one `Option`
                // test (`herdr::runtime::bridge()` → `None`) and nothing else.
                //
                // Both topics are owned by their emitter, as `background/watch/observer.rs:81-88`
                // argues: this is a subscriber, and it names the constants rather than the
                // strings.
                api.subscribe_bus(crate::background::watch::SUBAGENT_ASYNC_COMPLETE_EVENT);
                api.subscribe_bus(crate::background::watch::SUBAGENT_PROCESS_TERMINAL_EVENT);

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
                    // The herdr status bridge's root-turn edge. This is the one pi's subagents
                    // bridge does NOT have — its pane looks idle while the host itself is
                    // thinking, because only async children feed it — and it is what makes the
                    // pane say `working` while cyrup is working. `[CYRUP-EXCEEDS-UPSTREAM]`; the
                    // premise is on `herdr::state::StateModel::agent_start`.
                    cyrup_ext::EventKind::AgentStart,
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
        // pi `ctx.mode`, latched — see `execute_command`'s note and `slash_inspect_rpc`'s module
        // doc. Recorded here as well so the value is already correct on the FIRST command of a
        // session, rather than only from the second.
        crate::extension::host::slash_inspect_rpc::record_attached_mode(ctx.mode);
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

                // VL-S6 / pi `extension/index.ts:980-981` — restore this session's project-pane
                // map from disk, over the union of the keys already held, every root the owner's
                // on-disk index names, and the owner root itself. Cheap and synchronous: it reads
                // binding files, it never calls herdr, so a session starts at the same speed on a
                // box with no herdr installed.
                //
                // Two readers make this live rather than dead state: the roster's project-pane
                // section (`tui::fleet_status::project_pane_entries`, through
                // `FleetState::herdr_project_panes`) and the herdr status bridge's pane label
                // (`open_herdr_project_pane_count`, pi's `getProjectPaneCount` closure at
                // `extension/index.ts:864`).
                self.executor.restore_herdr_project_panes(&ctx.cwd);

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
                // WORKFLOW_7 — pi `restoreForegroundRunHistory` (`foreground-history.ts:149`), the
                // foreground twin of `resume_tracking`'s `restoreActiveJobs`. Ordered AFTER the
                // parent-session anchor capture (above) so `current_session_id()` is bound: the
                // restore is STRICT and an unbound session restores nothing at all.
                self.executor
                    .restore_foreground_run_history_for(&ctx.cwd)
                    .await;
                // C6: install the background-completion watcher (notify.ts / result-watcher.ts) so a
                // detached run that finishes during this session surfaces its `subagent-notify`
                // message (with `triggerTurn`) and has its result file deleted (R-SA-099/101). When the
                // P-1 host-services slot is bound this installs the live turn-injecting
                // `HostServicesCompletionSink` (R-SA-101); otherwise the stderr LoggingCompletionSink.
                //
                // THIS CALL WENT MISSING. The comment above survived without it, so every reader —
                // including the two tasks that built on it — took the watcher for granted while
                // `install_completion_watcher` had ZERO production callers: only `#[cfg(test)]` and
                // the cyrup-it suites reached it. In a real session nothing surfaced a detached
                // run's completion, no result file was reclaimed, and SCOPE_14's retention sweep —
                // which arms inside this very method — could never start. A comment is not a call.
                self.executor.install_completion_watcher(&ctx.cwd).await;
                // SCOPE_11 — pi `waitSubscriptionManager.restore()` (`extension/index.ts:971`),
                // ordered AFTER `install_completion_watcher` so the composite observer's slot is
                // already shared (the observer resolves the manager LATE, so the order is not
                // load-bearing — this position is upstream's, where restore follows the watcher).
                //
                // `ctx.has_ui` is pi's own gate, relocated from `wait-tool.ts:33`'s per-call
                // `ctx?.hasUI` to the one edge where cyrup actually knows it: a headless runtime
                // (`cyrup -p …`) ends its whole task in a single turn, so a wake scheduled for a
                // later turn could never be received. With no manager installed,
                // `{ nonBlocking: true }` takes pi's own `!deps.subscribe` refusal.
                if ctx.has_ui {
                    self.executor.install_wait_subscriptions(&ctx.cwd).await;
                } else {
                    self.executor.dispose_wait_subscriptions();
                }

                // SUBA-016 — pi `scheduledRunManager.bindSession(ctx)` + `restore()`. Ordered
                // AFTER `capture_parent_session_anchor()` above, because the manager PINS the
                // session identity at install (`ScheduleSessionSnapshot`) and a snapshot taken
                // before the anchor is bound would pin nothing.
                //
                // ⚠ Deliberately NOT gated on `ctx.has_ui`, unlike wait subscriptions directly
                // above. That gate exists because a wake scheduled for a later turn could never be
                // received by a headless run that ends in one turn. A schedule's output is a RUN
                // ON DISK — a headless process can produce one perfectly well, and a `cyrup -p`
                // invocation that fires a due schedule is the intended behaviour, not an
                // accident. Copying the `if ctx.has_ui` by reflex would make every schedule
                // interactive-only.
                self.executor.install_scheduled_runs(&ctx.cwd).await;

                // pi `fleetStatus.setContext(ctx)` (`tui/fleet-status.ts:271-288`): arm the
                // always-on fleet status widget for this session and paint it once. See
                // [`Self::refresh_fleet_status_widget`] for why the tick rides host event edges
                // rather than upstream's 500 ms interval.
                self.refresh_fleet_status_widget(&ctx.cwd, ctx.has_ui, ctx.mode)
                    .await;

                // The herdr status bridge, armed HERE — after `refresh_fleet_status_widget` and
                // BEFORE `emit_rpc_ready` below. That is pi's own order, which the comment on the
                // `emitReady` block already records: *"the tail of its own `session_start`
                // handler, after the herdr bridge and before `supervisorChannel.start()`"*.
                //
                // `arm` answers `None` unless this process is inside a herdr pane
                // (`HERDR_ENV=1` AND a non-empty `HERDR_PANE_ID`, the conjunction herdr's
                // `PaneLaunchIdentity::OmitPane` makes load-bearing) AND the session has a UI.
                // Outside a pane nothing is constructed: no socket, no task, no signal handler.
                // See `crate::herdr::runtime`'s four-state table.
                //
                // The human-wait input is `HostServices::human_interaction_lock()` — the ONE
                // session-scoped slot every companion that opens a human prompt acquires first
                // (`cyrup-ext/src/host/services.rs`'s `HumanInteractionLock`). It is reached off
                // the SAME backend Arc `load_native_with_services` clones into every native
                // (`cyrup-session-svc/src/builder.rs:1221-1224`), so the subagents extension and
                // the permission extension observe one object. That is what makes the pane say
                // `blocked`, which is the reason this feature exists.
                //
                // NOT `ctx.human_wait_gate()`. `HostCtx::event` mints a FRESH
                // `Arc<HumanWaitGate>` per native registration (`cyrup-ext/src/native.rs:170-180`,
                // one ctx per native at `cyrup-ext/src/facade.rs:541`), so this extension's gate
                // is not the one the permission dialog raises (`prompt.rs:184` holds a guard on
                // the permission extension's own ctx). Watching it left the pane at `working` for
                // the entire life of every approval dialog.
                //
                // `None` — no backend bound — arms everything but the dialog watcher; see
                // `crate::herdr::runtime::arm`.
                let human_lock = self
                    .executor
                    .host_services()
                    .and_then(|services| services.human_interaction_lock());
                if let Some(bridge) = crate::herdr::arm(ctx.has_ui, human_lock) {
                    // pi `sessionStarted({ hasUI, runs: restoredRuns })`
                    // (`herdr-status.ts:376-380`): the restore. A detached runner started by a
                    // PREVIOUS cyrup process is still going and will never deliver a "started"
                    // edge to this one, so the level has to be read off the fleet projection or
                    // the pane starts a session claiming to be idle while work is in flight.
                    let fleet = self.executor.fleet_state(&ctx.cwd, false, false).await;
                    bridge.sync_fleet(&fleet, self.executor.open_herdr_project_pane_count());
                }

                // PB-8 — pi `rpcBridge.emitReady(ctx)` (`extension/index.ts:1186`, the tail of its
                // own `session_start` handler, after the herdr bridge and before
                // `supervisorChannel.start()`): a client that attached before this process came up
                // learns the surface is live and reads the whole capability set off the ready
                // payload, instead of having to poll `ping` until one answers.
                self.emit_rpc_ready();
            }
            // The herdr status bridge's root-turn edge, and pi's `agentStarted()`
            // (`herdr-status.ts:372-375`) in one: the pane goes `working` while cyrup itself is
            // thinking, and every attention raised during the previous turn is acknowledged so a
            // notice the human has already been shown cannot re-block the pane. A run that is
            // still asking raises again on its next notice or at the next resync.
            //
            // No other subsystem in this crate handles `AgentStart`, so this arm is the
            // subscription's only consumer; outside a herdr pane it is one `Option` test.
            HostEvent::AgentStart => {
                if let Some(bridge) = crate::herdr::bridge() {
                    bridge.agent_start();
                }
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
                // pi `if (!ctx.hasUI) await drainOutstandingWork({ state, events: pi.events });`
                // (`extension/index.ts:784`) — the FIRST line of the goal-mission handler, so it
                // runs after the watchdog and before the goal scan. Headless only: an interactive
                // session has a next turn to be woken into, so blocking here would only delay a
                // prompt the user is waiting on. No session identity (pi's throw at
                // `auto-drain.ts:39`) means there is nothing scoping-safe to drain, so the block
                // is skipped — draining unscoped would drain another session's runs.
                if !ctx.has_ui
                    && let Some(session_id) = crate::identity::SessionId::parse_opt(
                        self.executor.current_session_id().as_deref(),
                    )
                {
                    let roots = self.executor.config_snapshot().await.roots;
                    let artifact_roots = crate::background::run_artifact_roots_in(&roots, &ctx.cwd);
                    let probe = crate::background::auto_drain::FsDrainProbe {
                        async_root: artifact_roots.async_root,
                        results_dir: artifact_roots.results_dir,
                    };
                    let waiter = crate::background::auto_drain::SubagentDrainWaiter {
                        // `enabled` is hard-coded TRUE: it is the `wait` TOOL's config gate, and
                        // pi's drain passes no `enabled` at all (`deps.enabled === false` is the
                        // check, `subagent-wait.ts:547`) — so the drain runs even when
                        // CYRUP_SUBAGENT_WAIT_TOOL_ENABLED=0 disables the tool. Anything else
                        // silently turns off headless result delivery with a convenience switch.
                        deps: crate::background::wait::WaitDeps::for_cwd(
                            &ctx.cwd,
                            true,
                            Some(session_id.as_str().to_string()),
                            &roots,
                        )
                        .with_completion_bus(Some(self.executor.completion_bus()))
                        // ASYNC_NOTIFY_BUG_REPORT F3.5 — the same ledger the wait tool shares,
                        // so headless drains dedup identically.
                        .with_inline_answers(Some(self.executor.inline_answers()))
                        // The drain's literal flags (`auto-drain.ts:61-63`); see
                        // [`crate::background::wait::WaitDeps::stop_on_attention`] for why the
                        // first one is what keeps this loop from spinning.
                        .with_stop_on_attention(false)
                        .with_fail_on_failed_runs(true)
                        .with_fail_on_attention(true),
                    };
                    if let Err(message) = crate::background::auto_drain::drain_outstanding_work(
                        &session_id,
                        crate::background::auto_drain::DEFAULT_AUTO_DRAIN_TIMEOUT_MS,
                        &crate::time::now_epoch_millis,
                        &probe,
                        &waiter,
                    )
                    .await
                    {
                        // [CYRUP-DELTA in mechanism, not in behaviour] upstream THROWS out of the
                        // handler (`auto-drain.ts:53,66`) — but that throw is not control flow: pi's
                        // runner catches every handler error (`runner.ts:869-878`) and emits it to
                        // the mode's `onError` listener, which headless — the only place this drain
                        // fires — is print-mode's one `console.error` line (`print-mode.ts:101-103`).
                        // Nothing fails, the handler loop continues. `on_event` returns a
                        // `HookOutcome` (whose upstream analog, the `EventResult` types, carries no
                        // error either — the error channel is the HOST'S catch), so cyrup emits the
                        // equivalent diagnostic here directly. The drain has still been AWAITED,
                        // which is the load-bearing part.
                        tracing::warn!(%message, "auto-drain at agent_end did not complete");
                    }
                }
                // pi's `agent_end` goal-mission handler (`extension/index.ts:585-601`).
                let _ = self
                    .executor
                    .raise_goal_continuation_notices(&ctx.cwd)
                    .await;
                // The fleet status widget's repaint edge (pi's 500 ms `setInterval` tick) — not a
                // registered handler, so its position here is free.
                self.refresh_fleet_status_widget(&ctx.cwd, ctx.has_ui, ctx.mode)
                    .await;
                // The herdr bridge is a NOTIFIER and goes last, after the watchdog's boundary
                // review and the goal scan, for the same reason the registration order above is
                // upstream's: those two can inject messages and can block, and a pane that said
                // `idle` before they finished would be lying for as long as they took.
                //
                // Saturating on the way down. `AgentEnd` fires without a matching `AgentStart` on
                // a session resumed mid-turn, and herdr never reclaims state from a `cyrup:`
                // source — an unsigned wrap would pin the pane at `working` for ever.
                if let Some(bridge) = crate::herdr::bridge() {
                    bridge.agent_end();
                }
            }
            HostEvent::SessionShutdown { .. } => {
                // STARTED first, JOINED last — see [`HERDR_RELEASE_JOIN_BUDGET`] for the
                // arithmetic and for what a plain `crate::herdr::shutdown().await` here cost.
                //
                // The release is the one piece of this teardown that is visible in ANOTHER
                // program's UI. Everything below it is cyrup's own in-process state, which nobody
                // outside the process can see; skipping the release leaves a `working` or
                // `blocked` row in the human's sidebar that only a herdr RESTART clears — herdr
                // never reclaims agent state from a `cyrup:` source (no TTL,
                // `tmp/herdr/src/terminal/state.rs:18-25`; a process-exit override that only fires
                // for an agent herdr can name, `:401-407`). `crate::herdr`'s module doc carries
                // the three citations. That is why it goes out FIRST and why it is still awaited.
                //
                // It is also the ONLY statement in this arm that waits on something outside the
                // process, so it is the only one that can hang. `tokio::spawn` is what keeps the
                // two facts — "first on the wire" and "cannot starve the rest" — from being in
                // tension: the task takes the global `BRIDGE` slot and starts draining on its own,
                // while the in-process disposals below run to completion regardless.
                //
                // This is also the `kill` path, not only the `/quit` path: cyrup's own signal
                // handler (`crates/cyrup/src/signals.rs:317-330`) takes SIGTERM/SIGHUP, and on an
                // interactive host it fires the cancel token rather than exiting, so `main`'s
                // `runtime.dispose()` fans `session_shutdown{quit}` out to here. The bridge
                // therefore installs NO signal handler of its own — a second one would exit the
                // process out from under this very teardown.
                let herdr_release = tokio::spawn(crate::herdr::shutdown());
                // pi `runtimeCleanup`/`session_shutdown` both call `supervisorChannel.dispose()`
                // (`extension/index.ts:412-430`): stop the poller and drop the pending map, so a
                // rebuilt session never re-surfaces the previous session's requests.
                self.supervisor_channel.dispose();
                // pi `runtimeCleanup`'s `mainWatchdog.dispose()` (`extension/index.ts:416`) and
                // `register-main.ts:434-437`'s own `session_shutdown` handler.
                self.watchdog.dispose();
                // SCOPE_11 — pi `waitSubscriptionManager.dispose()` (`extension/index.ts:1009`):
                // stop the reconcile timer and drop the in-memory map. The RECORDS stay on disk —
                // that is the entire point of the durable half, and the next session's `restore()`
                // is what picks them up.
                self.executor.dispose_wait_subscriptions();
                // SUBA-016 — pi `scheduledRunManager.stop()`. Aborts the tick and clears the slot,
                // and touches NEITHER the store nor an in-flight `active.lock`: the records are
                // the whole point of a cwd-keyed store, and releasing a live claim here would let
                // the next session double-launch a schedule whose run is still going.
                // `restore_one`'s stale-claim recovery is what clears a claim whose process died.
                self.executor.dispose_scheduled_runs();
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
                    // PB-8 — pi's cleanup block clears BOTH of this extension's widget slots:
                    // `fleetStatus?.dispose()` (`extension/index.ts:1063`) takes the fleet-status
                    // key, and `:1098`'s `ctx.ui.setWidget(WIDGET_KEY, undefined)` takes the
                    // async-jobs key. Since `refresh_fleet_status_widget` publishes the machine
                    // document into that second slot in `ExtMode::Rpc`, leaving it out here would
                    // strand a `PI_SUBAGENT_ASYNC_JSON:` line describing a session that is gone.
                    services.set_widget(
                        crate::background::async_status_snapshot::ASYNC_STATUS_SNAPSHOT_WIDGET_KEY,
                        None,
                        cyrup_ext::host::WidgetPlacement::default(),
                    );
                }
                // The join. Everything above has run; this is the only thing left to wait for, and
                // it is bounded so that a herdr which stopped answering ends this handler
                // normally instead of having it dropped at the dispatcher's deadline.
                match tokio::time::timeout(HERDR_RELEASE_JOIN_BUDGET, herdr_release).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        tracing::debug!(%err, "herdr: the release task ended abnormally");
                    }
                    Err(_elapsed) => tracing::warn!(
                        ?HERDR_RELEASE_JOIN_BUDGET,
                        "herdr: the pane release did not finish inside the join budget; the rest \
                         of the subagents teardown ran"
                    ),
                }
            }
            // pi `register-main.ts:415-418`.
            HostEvent::BeforeAgentStart {
                prompt,
                system_prompt,
                ..
            } => {
                self.watchdog.handle_before_agent_start(
                    &serde_json::json!({ "prompt": prompt, "systemPrompt": system_prompt }),
                    &ctx.cwd,
                );
            }
            // pi `register-main.ts:419-422`. The event is re-shaped into the `{type:"turn_end",
            // message, toolResults}` object `formatWatchdogTurnDelta`/`eventIndicatesRepoEdit`
            // duck-type against — the same JSON pi's own handler receives.
            HostEvent::TurnEnd {
                message,
                tool_results,
                ..
            } => {
                let event =
                    crate::watchdog::turn_delta::watchdog_turn_end_event(message, tool_results);
                self.watchdog.handle_turn_end(&event, &ctx.cwd);
                // The herdr bridge's canonical resync point (the Python client's own, at
                // `tmp/code_puppy_core_plugins/.../herdr/reporter.py:220-229`): re-read the two
                // inputs that are LEVELS rather than edges — how many detached background runs are
                // active, and which of them are asking for the human — and republish the pane's
                // label. A turn boundary is where the projection is cheapest and most likely to
                // have changed.
                if let Some(bridge) = crate::herdr::bridge() {
                    let fleet = self.executor.fleet_state(&ctx.cwd, false, false).await;
                    bridge.sync_fleet(&fleet, self.executor.open_herdr_project_pane_count());
                }
            }
            // pi `register-main.ts:423-426` — the mid-run cadence trigger.
            HostEvent::ToolResult { .. } => {
                self.watchdog.handle_tool_result(&ctx.cwd);
            }
            // pi `register-main.ts:431-432` — a switch or a fork abandons this session's review
            // state entirely, scope artifact and auto-follow counters included.
            HostEvent::SessionBeforeSwitch { .. } | HostEvent::SessionBeforeFork { .. } => {
                self.watchdog
                    .reset(crate::watchdog::runtime::WatchdogResetOptions {
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
                self.watchdog
                    .reset(crate::watchdog::runtime::WatchdogResetOptions {
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
                let skill_paths: Vec<String> =
                    crate::registration::resources::bundled_skill_files()
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

    /// PB-8 — answer one inter-extension RPC request (pi's ONE listener,
    /// `extension/rpc.ts:822-838` @v0.68.0): parse, handle, reply.
    ///
    /// **A handler fault is REPLIED, never returned.** Upstream's `try`/`catch` at `:834-837` has
    /// no path that returns silently, and returning `Err` here would be CONTAINED and logged by
    /// `BusFanout::drain_bus` (`cyrup-ext/src/facade.rs:2740-2750`) — fan-out would continue and
    /// the caller would wait forever for a reply that never comes. So `Err` is reserved for the
    /// one case where there is genuinely no reply to be had: the reply itself could not be
    /// emitted, because no capability backend is bound and therefore no bus exists to emit on.
    ///
    /// The reply travels back out through
    /// [`cyrup_ext::host::HostServices::emit_event`] → `SharedBus::emit`, which QUEUES; the
    /// in-flight `drain_bus` picks it up on its next round, so a client subscribed to the reply
    /// topic is reached inside the same `deliver_bus_events(..)` call that carried the request.
    /// See [`crate::extension::rpc`]'s module doc for the whole client contract.
    ///
    /// # `[CYRUP-DELTA, mechanism]` — the handler body runs ON the host's drain, not beside it
    ///
    /// Upstream registers `options.events.on(SUBAGENT_RPC_REQUEST_EVENT, async (raw) => {…})`
    /// (`rpc.ts:822`). pi's `EventEmitter` discards the returned promise, so `emit` returns at
    /// once and the handler's I/O costs the emitter nothing.
    ///
    /// cyrup's `on_bus_event` is `async` and is AWAITED by `BusFanout::drain_bus`
    /// (`cyrup-ext/src/facade.rs:2736-2748`), which holds the RAII `DrainLatch` acquired at
    /// `facade.rs:2724` for the whole fan-out, and is itself awaited from every host event
    /// dispatch (`cyrup-ext/src/dispatch.rs:327,352,372,388,417`) and from
    /// `run_command`/`run_shortcut` (`facade.rs:2150,2371`).
    ///
    /// **The real consequence**, stated plainly: for as long as one RPC method runs — a `status`
    /// that reconciles runs and reads a transcript, a `stop` that walks a run tree — every OTHER
    /// extension's bus event queued in the same batch waits behind it, and if the drain was
    /// entered from `run_command` the user's slash command does not return until it finishes.
    /// Upstream's equivalent request blocks nothing.
    ///
    /// This is not fixed by spawning the body onto a task. The trait hands out `&self` with no
    /// owning handle (`cyrup_ext::native::NativeExtension::on_bus_event`), and detaching would
    /// also break the guarantee this surface is built on — that a reply is emitted inside the SAME
    /// `deliver_bus_events(..)` call, which is what lets a client round-trip with no pump and
    /// nothing to sleep on (see [`crate::extension::rpc`]'s "Delivery is deferred, but not slow").
    /// The cost is paid deliberately to buy that; it is recorded here so nobody has to rediscover
    /// it from a stalled `/command`.
    async fn on_bus_event(
        &self,
        topic: &str,
        payload: &serde_json::Value,
        _ctx: &HostCtx,
    ) -> Result<(), ExtError> {
        // The herdr status bridge's two run-ended edges. Both are pure notifications with no
        // reply, so they are answered before the RPC gate below rather than inside it.
        //
        // `async-complete` is the ordinary end of a background run; `process-terminal` is the one
        // that catches a runner whose PROCESS died without ever writing a result, which is exactly
        // the shape that would otherwise leave the pane at `working` with nothing left to end it.
        // Either way the run leaves the attention set, and the next `sync_fleet` re-derives the
        // level.
        if topic == crate::background::watch::SUBAGENT_ASYNC_COMPLETE_EVENT
            || topic == crate::background::watch::SUBAGENT_PROCESS_TERMINAL_EVENT
        {
            if let Some(bridge) = crate::herdr::bridge()
                && let Some(run_id) = payload
                    .get("runId")
                    .and_then(serde_json::Value::as_str)
                    .filter(|id| !id.is_empty())
            {
                bridge.clear_attention(&crate::background::RunId::from_token(run_id.to_string()));
                let fleet = self.executor.fleet_state(&self.cwd, false, false).await;
                bridge.sync_fleet(&fleet, self.executor.open_herdr_project_pane_count());
            }
            return Ok(());
        }
        if topic != crate::extension::rpc::SUBAGENT_RPC_REQUEST_EVENT {
            return Ok(());
        }
        let (reply_topic, reply) = match self.rpc_tool.get() {
            Some(tool) => {
                self.rpc_bridge
                    .dispatch(
                        payload,
                        &crate::extension::rpc::RpcDeps {
                            tool,
                            executor: &self.executor,
                            cwd: &self.cwd,
                        },
                    )
                    .await
            }
            // Structurally unreachable: the subscription above and the tool capture are the same
            // arm of `init`. Answered anyway — see `rpc::unregistered_reply`.
            None => crate::extension::rpc::unregistered_reply(payload),
        };
        let Some(services) = self.executor.host_services() else {
            return Err(ExtError::Component(
                "subagents: no capability backend is bound, so the RPC reply cannot be emitted"
                    .to_string(),
            ));
        };
        services.emit_event(&reply_topic, &reply);
        Ok(())
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
        // pi `ctx.mode`, latched for the one handler that needs it and cannot be handed a ctx —
        // `/subagents-inspect-rpc`'s `mode === "tui"` guard (`slash/slash-commands.ts:931`). See
        // `slash_inspect_rpc`'s module doc for exactly how faithful that latch is. Written from
        // THIS invocation's ctx, immediately before dispatching it.
        crate::extension::host::slash_inspect_rpc::record_attached_mode(ctx.mode);

        // pi `register-main.ts:403-409` registers `/subagents-watchdog` as its OWN command, separate
        // from this crate's eighteen `SLASH_COMMANDS`, so it routes before the table lookup.
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

    /// VL-S11 R3 — run the chord declared at [`NativeExtension::init`] through
    /// [`cyrup_ext::native::InitApi::register_shortcut`].
    ///
    /// pi `pi.registerShortcut(options.foregroundDetachShortcut as KeyId, { description, handler:
    /// async (ctx) => detachForegroundRun("", ctx) })` (`slash/slash-commands.ts:1007-1012`): the
    /// handler is the SAME closure `/subagents-detach` is registered with, called with an empty
    /// argument. That is reproduced exactly —
    /// [`SubagentsExtension::dispatch_shortcut`] reaches
    /// [`SubagentsExtension::slash_subagents_detach`] with `""` — so the chord and the command can
    /// never resolve different targets or print different sentences.
    ///
    /// `ctx` is COMMAND tier here as it is for `execute_command`, matching pi, where a shortcut
    /// handler receives the same `ExtensionContext` a command handler does.
    ///
    /// A key this extension did not bind falls through to the trait default's own sentence rather
    /// than silently succeeding; see [`SubagentsExtension::dispatch_shortcut`].
    async fn execute_shortcut(&self, key: &str, ctx: &HostCtx) -> Result<(), ExtError> {
        ctx.require_command_tier()?;
        crate::extension::host::slash_inspect_rpc::record_attached_mode(ctx.mode);
        self.dispatch_shortcut(key).await
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
    /// EXPANDED rendering (pi's `options.expanded` arm, `:1431-1500`) is not drawn here, so this
    /// always emits pi's COMPACT tier — the tier a collapsed row shows, which is what the
    /// transcript draws by default. Since EXT-006 that is a CHOICE rather than a limit: the host
    /// does now carry `options.expanded`, and a renderer that wants the expanded arm overrides
    /// [`cyrup_ext::NativeExtension::render_result_under`] (whose default delegates to this
    /// two-argument form) and is re-invoked whenever the toggle moves.
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
    let string_field = |key: &str| {
        args.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    };
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
    let array_len = |key: &str| {
        args.get(key)
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len)
    };
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
    let payload = details.filter(|d| !d.is_null()).and_then(|d| {
        serde_json::from_value::<crate::tui::events::SubagentUpdatePayload>(d.clone()).ok()
    });

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
        let prefix = if details
            .and_then(|d| d.get("context"))
            .and_then(serde_json::Value::as_str)
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
            let Ok(bytes) = tokio::fs::read(&nested_paths.status).await else {
                continue;
            };
            let Ok(nested) = serde_json::from_slice::<crate::background::RunStatus>(&bytes) else {
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
