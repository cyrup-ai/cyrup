//! The slash-command dispatcher and the fleet surfaces it opens.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::{CancelToken, ModelId};

use crate::background::RunMode;
use crate::discovery::types::AgentReadScope;
use crate::error::SubagentError;
use crate::fork_context::ContextMode;
use crate::registration::prompt_workflows;
use crate::registration::slash_commands::{self, SlashCommandName};
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use crate::spawn::depth::resolve_effective_depth;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::format_slash_run_completion;
use crate::extension::executor::requests::{
    BackgroundSingleRequest, ForegroundRunRequest, GraphRunOutcome, SingleRunOverrides,
    StatusViewSelector,
};
use crate::extension::host::SubagentsExtension;
use crate::extension::host::slash_render::{render_chain_results, seed_first_step_task};
use crate::extension::models::classify::render_profile_check_report;
use crate::extension::tool::task_items::count_graph_requested_spawns;

impl SubagentsExtension {
    /// The single shared dispatch body [`cyrup_ext::NativeExtension::execute_command`] calls into
    /// (R-SA-130). Parses `args` via the real, already-built parsers in
    /// [`crate::registration::slash_commands`], then routes to [`SubagentExecutor`] exactly as
    /// the tool itself does for `/run`; the remaining commands route to their own
    /// already-implemented subsystem entry points (`registration::doctor`/`cost`/`profiles`).
    /// pi `showFleet(ctx)` (`slash/slash-commands.ts:633-649`) — the `/subagents-fleet` handler at
    /// v0.43.0, and the handler `pi.registerShortcut(Key.ctrlAlt("f"), …)` shares (`:719-722`).
    ///
    /// Three outcomes, upstream's own:
    /// 1. **No UI** → `runSlashSubagent(pi, ctx, { action: "status", view: "fleet" })` (`:635-638`),
    ///    which is exactly [`SubagentExecutor::control_status_view`]'s `view: "fleet"` form — the
    ///    same text surface this command rendered unconditionally at the v0.34.0 baseline.
    /// 2. **Already open** → `ctx.ui.notify("Subagent fleet inspector is already open.", "info")`
    ///    (`:639-642`).
    /// 3. **Open it** → `openSubagentFleet(ctx, state, { asyncDirRoot: DIRS.async, resultsDir:
    ///    DIRS.results })` (`:645`), which clears the status widget (`tui/fleet.ts:846`), raises
    ///    `state.fleetInspectorOpen` (`:844-845`), and restores both in its `finally` (`:876-878`).
    ///
    /// Upstream's third outcome AWAITS an interactive overlay
    /// (`ctx.ui.custom(factory, { overlay: true, … })`, `tui/fleet.ts:869-875`), and cyrup's
    /// counterpart is [`cyrup_ext::HostServices::open_overlay`]: the constructed component is
    /// wrapped in a [`crate::tui::fleet_overlay::FleetOverlay`] and handed to the host, which
    /// paints it, routes every keystroke into
    /// [`crate::tui::fleet::SubagentFleetComponent::handle_input`], ticks it at
    /// [`crate::tui::fleet::REFRESH_MS`], and blocks this call until the user closes it — pi's own
    /// `await ctx.ui.custom(...)`. The width and the terminal height both come from the host frame
    /// on every paint, so nothing here guesses at a terminal size.
    ///
    /// `open_overlay` answering `false` is not an error: it means the attached mode owns no
    /// terminal to drive a modal on (RPC, an embedder, a session with no UI sink). That is exactly
    /// pi's `!ctx.hasUI` situation, so the command falls back to the SAME text fleet view outcome 1
    /// renders rather than reporting a failure.
    ///
    /// `showFleet`'s first statement, `state.lastUiContext = ctx` (`:634`), has no counterpart:
    /// upstream stashes the live `ExtensionContext` so a LATER, context-less caller can still reach
    /// a UI. cyrup's equivalent already exists and is bound elsewhere — the P-1 `host_services`
    /// slot ([`SubagentExecutor::set_host_services`]), which the session builder binds once before
    /// `init` and which every surface in this file reads.
    async fn show_fleet(&self, cwd: &Path, has_ui: bool) -> Result<String, SubagentError> {
        use std::sync::atomic::Ordering;

        use crate::tui::fleet::{open_subagent_fleet, FleetOpenOutcome, FleetViewOptions};

        // pi reads `fleetOpen` BEFORE it touches anything else that could change it.
        let already_open = self.fleet_open.load(Ordering::Acquire);
        let state = self
            .executor
            .fleet_state(cwd, true, self.fleet_inspector_open.load(Ordering::Acquire))
            .await;

        match open_subagent_fleet(
            has_ui,
            already_open,
            state,
            FleetViewOptions::default(),
            None,
            // The action bundle exists (steer/stop route to `control_steer`/`control_stop`); the
            // Herdr inspector does not — see `tui/fleet.rs`'s delta 2.
            true,
            false,
        ) {
            FleetOpenOutcome::NoUiFallback => self
                .executor
                .control_status_view(
                    cwd,
                    None,
                    None,
                    false,
                    StatusViewSelector {
                        view: Some("fleet"),
                        ..StatusViewSelector::default()
                    },
                )
                .await
                .map_err(SubagentError::Management),
            FleetOpenOutcome::AlreadyOpen => {
                Ok("Subagent fleet inspector is already open.".to_string())
            }
            FleetOpenOutcome::Opened { component, clear_widget_key } => {
                self.fleet_open.store(true, Ordering::Release);
                self.fleet_inspector_open.store(true, Ordering::Release);
                // pi `ctx.ui.setWidget(FLEET_STATUS_WIDGET_KEY, undefined)` (`tui/fleet.ts:846`):
                // the status widget must be gone before the overlay paints.
                let services = self.executor.host_services();
                if let Some(services) = services.as_ref() {
                    services.set_widget(
                        clear_widget_key,
                        None,
                        cyrup_ext::host::WidgetPlacement::default(),
                    );
                }
                if let Ok(mut widget) = self.fleet_status.lock() {
                    widget.set_inspector_open(true);
                }

                let overlay = crate::tui::fleet_overlay::FleetOverlay::new(
                    *component,
                    Arc::clone(&self.executor),
                    cwd.to_path_buf(),
                    FleetViewOptions::default().refresh_ms,
                    tokio::runtime::Handle::current(),
                );
                // BLOCKS until the human closes the modal — pi's `await ctx.ui.custom(...)`.
                let driven = services
                    .as_ref()
                    .is_some_and(|services| services.open_overlay(Box::new(overlay)));

                // pi's `finally` (`slash-commands.ts:646-647` + `tui/fleet.ts:876-878`): both
                // latches are restored however the overlay ended.
                if let Ok(mut widget) = self.fleet_status.lock() {
                    widget.set_inspector_open(false);
                }
                self.fleet_inspector_open.store(false, Ordering::Release);
                self.fleet_open.store(false, Ordering::Release);

                if driven {
                    // The overlay said everything it had to say on screen; pi's
                    // `ctx.ui.custom<undefined>` likewise resolves with no value, and a non-empty
                    // return here would surface as a redundant notification
                    // (`cyrup-session-svc/src/session.rs`'s `try_execute_extension_command`).
                    return Ok(String::new());
                }
                // No terminal to drive it on — pi's `!ctx.hasUI` outcome, one level later.
                self.executor
                    .control_status_view(
                        cwd,
                        None,
                        None,
                        false,
                        StatusViewSelector {
                            view: Some("fleet"),
                            ..StatusViewSelector::default()
                        },
                    )
                    .await
                    .map_err(SubagentError::Management)
            }
        }
    }

    /// pi's `SubagentFleetStatus.refresh()` tick (`tui/fleet-status.ts:285,301-350`) — recollect
    /// the active-agent roster and republish (or clear) the status widget.
    ///
    /// \[CYRUP-DELTA] Upstream drives this from a 500 ms `setInterval` armed in `setContext`
    /// (`:285`) and registers a widget FACTORY the TUI re-invokes. cyrup's
    /// [`cyrup_ext::HostServices::set_widget`] is a fire-and-forget payload
    /// (`cyrup-ext/src/host/services.rs:241`) and the extension surface exposes no timer, so the
    /// tick rides the host's own event edges instead: `SessionStart` (arm + first paint),
    /// `AgentEnd` (repaint after every turn) and `SessionShutdown` (clear). The change-detector
    /// ([`crate::tui::fleet_status::SubagentFleetStatus::render_key`]) still suppresses redundant
    /// publishes exactly as upstream's does, so a no-op edge costs one fold and no host call.
    pub(crate) async fn refresh_fleet_status_widget(&self, cwd: &Path, has_ui: bool) {
        use std::sync::atomic::Ordering;

        // pi's `fleetViewEnabled` gate (`extension/index.ts:378`): with the fleet view off there is
        // no `SubagentFleetStatus` at all upstream, so nothing is ever published.
        if !self.fleet_view_enabled {
            return;
        }
        let Some(services) = self.executor.host_services() else { return };
        let state = self
            .executor
            .fleet_state(cwd, false, self.fleet_inspector_open.load(Ordering::Acquire))
            .await;
        let now = crate::time::now_epoch_millis();
        let payload = {
            let Ok(mut widget) = self.fleet_status.lock() else { return };
            widget.set_ui_available(has_ui);
            if !widget.refresh(&state, now) {
                return;
            }
            // Same 100-column fallback, and for the same reason, as `show_fleet` above.
            // EXT-047: pi's `setWidget(key, content, { placement })` — three arguments, so the
            // placement the fleet-status widget resolved is no longer lost inside an opaque blob.
            (widget.widget_lines(100, now), widget.placement())
        };
        let placement = match payload.1 {
            crate::tui::fleet_status::FleetViewPlacement::BelowEditor => {
                cyrup_ext::host::WidgetPlacement::BelowEditor
            }
            crate::tui::fleet_status::FleetViewPlacement::AboveEditor => {
                cyrup_ext::host::WidgetPlacement::AboveEditor
            }
        };
        match payload.0 {
            Some(lines) => services.set_widget(
                crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY,
                Some(&lines),
                placement,
            ),
            // pi `ctx.ui.setWidget(FLEET_STATUS_WIDGET_KEY, undefined)` (`:309,320`) — a removal.
            None => services.set_widget(
                crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY,
                None,
                placement,
            ),
        }
    }

    pub(crate) async fn dispatch_slash(
        &self,
        command: SlashCommandName,
        args: &str,
        cwd: &Path,
        has_ui: bool,
    ) -> Result<String, SubagentError> {
        match command {
            SlashCommandName::Run => self.slash_run(args, cwd).await,
            // pi's `/subagents-doctor` handler calls `runSlashSubagent(pi, ctx, { action: "doctor"
            // })` — no `sessionDir` override on the slash-command surface
            // (`slash-commands.ts:694-699` @v0.43.0), so `formatConfiguredSessionDir` falls through
            // to the configured default.
            SlashCommandName::SubagentsDoctor => Ok(self.executor.run_doctor(cwd, None).await),
            // G92: `/subagents-fleet`'s handler at v0.43.0 is exactly `showFleet(ctx)`
            // (`slash-commands.ts:714-717`) — see [`Self::show_fleet`] for the three-outcome
            // control flow it ports. Its no-UI branch still lands on the SAME
            // `control_status_view(view: "fleet")` entry point the
            // `subagent({ action: "status", view: "fleet" })` tool call uses, so the two surfaces
            // can never render different fleets (R-SA-130).
            SlashCommandName::SubagentsFleet => self.show_fleet(cwd, has_ui).await,
            SlashCommandName::SubagentsStop => self.slash_subagents_stop(args, cwd).await,
            SlashCommandName::SubagentsGuide => self.slash_subagents_guide(args),
            SlashCommandName::SubagentsProfiles => self.slash_subagents_profiles(),
            SlashCommandName::SubagentsLoadProfile => self.slash_load_profile(args).await,
            SlashCommandName::SubagentCost => Ok(self.executor.run_cost_report(cwd).await),
            SlashCommandName::Chain => self.slash_chain(args, cwd).await,
            SlashCommandName::Parallel => self.slash_parallel(args, cwd).await,
            SlashCommandName::RunChain => self.slash_run_chain(args, cwd).await,
            SlashCommandName::SubagentsModels => self.slash_models(args, cwd),
            SlashCommandName::SubagentsRefreshProviderModels => {
                self.slash_refresh_provider_models(args, cwd).await
            }
            SlashCommandName::SubagentsGenerateProfiles => self.slash_generate_profiles(args).await,
            SlashCommandName::SubagentsCheckProfile => self.slash_check_profile(args).await,
            SlashCommandName::PromptWorkflow => self.slash_prompt_workflow(args, cwd).await,
            SlashCommandName::ChainPrompts => self.slash_chain_prompts(args, cwd).await,
        }
    }

    /// Slash-live-state (T8, partial — pi `slash/slash-live-state.ts`): pi posts an IMMEDIATE
    /// in-transcript placeholder message the moment `/run` is invoked, then UPDATES IT IN
    /// PLACE as the run streams and finally renders the completed result over the same
    /// transcript entry. The crate cannot post that immediate placeholder or update it in
    /// place today: `NativeExtension::execute_command` returns a single `Option<String>` (its
    /// one final transcript entry) and its `HostCtx` exposes no transcript-message sink and no
    /// update-in-place handle. So the crate-side minimum here is to make the SINGLE returned
    /// entry read as the placeholder RESOLVED to completion — a completion summary (status +
    /// agent + tool/token stats) over the delivered output, exactly what pi's placeholder
    /// renders once the run settles (`renderSubagentResult`). The immediate placeholder + live
    /// in-place update is the remaining outer-layer step, gated on a host transcript-update
    /// channel `cyrup-tui`/`HostCtx` must expose (the tool path already streams live progress
    /// via `ToolUpdateSink`, C19 — the slash path has no equivalent sink yet).
    async fn slash_run(&self, args: &str, cwd: &Path) -> Result<String, SubagentError> {
        let parsed = slash_commands::parse_run_command(args)
            .map_err(|e| SubagentError::MalformedSettings(e.message))?;
        let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
        let model = parsed.config.model.clone().map(ModelId::from);
        // SUBA-002 — charge the per-SESSION spawn budget on the SLASH surface too. Upstream
        // gets this for free: `/run`'s handler calls `runSlashSubagent` -> `requestSlashRun`
        // -> the bridge wired at `extension/index.ts:512-517` -> `executeSubagentCollapsed`
        // -> the SAME `executor.execute` the tool uses, whose `reserveSubagentSpawns`
        // (`subagent-executor.ts:266-282`, called at `:3434-3441`) therefore covers both
        // surfaces. Here `dispatch_slash` is an independent entry point into
        // `SubagentExecutor`, so without this the budget would be enforced on the tool path
        // ONLY and a session that had exhausted it could keep fanning out via `/run`.
        //
        // `/run` is pi's SINGLE shape (`params.agent` set, no `tasks`/`chain`), so
        // `countRequestedSubagentSpawns` bills it exactly `1` — the same `1` whether the run
        // goes foreground or background, which is why the charge sits after parsing and
        // ahead of the mode branch below rather than inside either arm (charging once, never
        // twice, and never a count that differs from what actually gets spawned).
        let run_cfg = self.executor.config_snapshot().await;
        // SUBA-002 follow-up — the DEPTH guard must precede the charge (pi checks the
        // ceiling at `subagent-executor.ts:3297-3312`, ahead of `reserveSubagentSpawns` at
        // `:3434-3441`). `/run`'s own R-SA-055 guard lives inside `run_foreground`/
        // `spawn_background`, i.e. strictly after this charge, so a depth-blocked `/run`
        // was billed and then refused. `/run-chain` already checks here for its own
        // (discovery-ordering) reason; this is the same rung for the `/run` surface.
        let depth = resolve_effective_depth(run_cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }
        self.executor
            .reserve_subagent_spawns(1, run_cfg.max_subagent_spawns_per_session)
            .map_err(SubagentError::SpawnLimitExceeded)?;

        // G98 / pi `applySingleAgentLaunchDefaults` (`subagent-executor.ts:1930-1947`,
        // applied at `:4927` @v0.43.0). `/run` is pi's SINGLE shape, and upstream reaches it through the SAME
        // `executor.execute` the tool does, so the agent's own `async:`/`timeoutMs:`
        // frontmatter defaults apply here identically. cyrup's `/run` is an independent
        // entry point, so it has to apply them itself — see
        // [`SubagentExecutor::single_agent_launch_defaults`] for why the resolution is
        // shared rather than duplicated.
        //
        // Fill-unset-only, and `/run`'s "unset" is precise: the surface parses no
        // `timeoutMs=`/`maxRuntimeMs=` token at all (`slash-commands.ts:678-681`
        // forwards only `output`/`outputMode`/`skill`/`model`), so a timeout
        // default is ALWAYS eligible; `--bg` is the only async signal, and an explicit
        // `--bg` must beat an agent declaring `async: false`, so the default only decides
        // the case where `--bg` was NOT typed.
        let (default_async, default_timeout_ms, _default_turn_budget) =
            SubagentExecutor::single_agent_launch_defaults(cwd, &parsed.agent);
        let background = parsed.flags.background || default_async.unwrap_or(false);
        if background {
            let run_id = self
                .executor
                .spawn_background(BackgroundSingleRequest {
                    // SUBA-021: the slash surfaces advertise no `usageBudget` param upstream either.
                    usage_budget: None,
                    // SUBA-008: `/run` parses no `turnBudget=` token (upstream's
                    // `slash-commands.ts:678-681` forwards only output/outputMode/skill/
                    // model), so there is no CALLER rung here — but the agent's own
                    // `turnBudget:` frontmatter and `subagents.turnBudget` still apply,
                    // because `spawn_background` resolves those two itself.
                    turn_budget: None,
                    structured_output_schema: None,
                    tool_budget: None,
                    cwd,
                    agent_name: &parsed.agent,
                    task: &parsed.task,
                    context,
                    model_override: model,
                    agent_scope: AgentReadScope::Both,
                    // pi's own `/run` handler forwards `output`/`outputMode`/`skill`/`model`
                    // and NOTHING else (`slash/slash-commands.ts:678-681`) — the
                    // inline `acceptance=` token it parses is only ever consumed by the
                    // `/chain`//`/parallel` step builders. Faithful parity: `None` here.
                    acceptance: None,
                    // SUBA-N06: nor an `includeProgress=` token.
                    include_progress: None,
                    // Same parity rule: the `/run` surface parses no `control=` token, so
                    // there is no per-call override to forward. `spawn_background` still
                    // folds in the extension-level `subagents.control` block.
                    control: None,
                    // SUBA-N03: `None` here is SYMMETRY with the foreground `/run` branch
                    // directly below, not a background-specific drop — that branch calls
                    // `run_foreground(…)`'s flat legacy signature, which likewise carries
                    // no override bundle. `/run`'s own parser does not yet surface pi's
                    // `output`/`outputMode`/`skill` tokens (`slash-commands.ts:678-681`)
                    // on EITHER path; wiring that surface is a separate unit, and
                    // until it lands both `/run` paths behave identically. `share`/
                    // `sessionDir`/`artifacts`/`timeoutMs` have no `/run` token upstream at
                    // all.
                    output: None,
                    output_mode: None,
                    skills: None,
                    share: None,
                    session_dir: None,
                    artifacts: None,
                    // G98: `/run` parses no timeout token, so this is purely the agent's
                    // own `timeoutMs:` default — never an override of a call-site value.
                    timeout_ms: default_timeout_ms,
                })
                .await?;
            Ok(format!("Background subagent run started: {run_id}"))
        } else {
            let result = self
                .executor
                .run_foreground(
                    cwd,
                    &parsed.agent,
                    &parsed.task,
                    context,
                    model,
                    // G98: same agent-level default on the foreground branch — pi applies
                    // it before the async/foreground fork, not inside one arm of it.
                    default_timeout_ms,
                )
                .await?;
            Ok(format_slash_run_completion(&result))
        }
    }

    /// G77: pi's `/subagents-stop` handler with an explicit id is exactly
    /// `runSlashSubagent(pi, ctx, { action: "stop", id })` (`slash-commands.ts:754-757`
    /// @v0.43.0) — routed here to the SAME `control_stop` entry point the
    /// `subagent({ action: "stop" })` tool call lands on, so the two surfaces can never
    /// diverge (R-SA-130).
    ///
    /// With NO id upstream opens a TUI selector over the discovered stop targets and then
    /// issues the identical call for the chosen one (`:775-791`); this crate's slash layer
    /// has no overlay seam (see `SLASH_COMMANDS`' own note that `CommandDescriptor` carries
    /// a static completion list, not a closure), so the empty-argument case takes upstream's
    /// OWN documented no-UI fallback path instead — `if (!ctx.hasUI) sendSlashText(pi,
    /// stopFallbackText(targets))` (`:774`) — rather than guessing a target. A stop is
    /// unrecoverable, so guessing is the one thing this command must not do.
    async fn slash_subagents_stop(&self, args: &str, cwd: &Path) -> Result<String, SubagentError> {
        let id = args.trim();
        if id.is_empty() {
            self.executor
                .format_stop_targets(cwd)
                .await
                .map_err(SubagentError::Management)
        } else {
            self.executor
                .control_stop(cwd, Some(id), None)
                .await
                .map_err(SubagentError::Management)
        }
    }

    /// SUBA-066 — pi `slash-commands.ts:706-719` @v0.47.1. The command routes to the SAME
    /// `read_subagent_guide` the `guide` action does, which is the whole point of landing the
    /// two together: two readers over one packaged document set cannot disagree about what
    /// the current contract says.
    ///
    /// Upstream refuses a multi-word argument with `ctx.ui.notify("Usage: /subagents-guide
    /// [topic]", "error")` (`:712-715`) BEFORE dispatching, rather than letting it fall
    /// through to the unknown-topic message — because a two-word argument is a usage mistake,
    /// not a wrong topic, and the topic list would be the wrong answer to it.
    fn slash_subagents_guide(&self, args: &str) -> Result<String, SubagentError> {
        let topic = args.trim();
        if topic.contains(char::is_whitespace) {
            return Err(SubagentError::Management(
                "Usage: /subagents-guide [topic]".to_string(),
            ));
        }
        Ok(crate::registration::guide::read_subagent_guide(Some(topic)))
    }

    /// /subagents-profiles — list the names of every saved profile in the profiles directory.
    fn slash_subagents_profiles(&self) -> Result<String, SubagentError> {
        let profiles_dir = self.profiles_dir();
        let profiles = crate::registration::profiles::describe_profiles(&profiles_dir)?;
        if profiles.is_empty() {
            Ok("No saved subagent profiles.".to_string())
        } else {
            Ok(profiles
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }

    /// /subagents-load-profile — load one named profile's overrides into the live settings.
    async fn slash_load_profile(&self, args: &str) -> Result<String, SubagentError> {
        let name = slash_commands::parse_subagents_load_profile_command(args)
            .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
        self.load_profile_into_settings(&name).await
    }

    /// /chain — linear sequence (with optional inline parallel groups), R-SA-129/§5.1/§5.3.
    /// Routes into the SAME chain-graph walker (`spawn::chain_graph::walk_chain`) and the
    /// SAME `ExecSingleStepExecutor` subprocess-spawning adapter the hop-2 background
    /// runner uses for a saved/async chain (R-SA-130: one execution code path, never a
    /// second divergent implementation for the foreground slash-command shape).
    async fn slash_chain(&self, args: &str, cwd: &Path) -> Result<String, SubagentError> {
        let parsed = slash_commands::parse_chain_command(args)
            .map_err(|e| SubagentError::MalformedSettings(e.message))?;
        let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
        // `/chain` carries no separate top-level task arg — the first step's task seeds the
        // chain, so `{task}` falls back to it (`first_step_task`).
        self.run_or_background_chain(cwd, parsed.chain, RunMode::Chain, context, parsed.flags.background, None)
            .await
    }

    /// /parallel — a single static-width fan-out group (R-SA-129/§5.3). Represented as a
    /// ONE-element `ChainGraph` whose sole element is a `RunnerStep::ParallelGroup`, so it
    /// is dispatched by the identical `walk_chain`/`run_bounded` machinery a parallel GROUP
    /// inside a longer `/chain` uses — never a second, parallel-only dispatch path.
    async fn slash_parallel(&self, args: &str, cwd: &Path) -> Result<String, SubagentError> {
        let parsed = slash_commands::parse_parallel_command(args)
            .map_err(|e| SubagentError::MalformedSettings(e.message))?;
        let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
        let cfg = self.executor.config_snapshot().await;
        let group = RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
            steps: parsed.tasks,
            concurrency: cfg.parallel_concurrency(),
            fail_fast: false,
            worktree: false,
        });
        // `/parallel` carries no separate top-level task arg — `{task}` falls back to the
        // group's first task (`first_step_task`).
        self.run_or_background_chain(cwd, vec![group], RunMode::Parallel, context, parsed.flags.background, None)
            .await
    }

    /// /run-chain — invoke a saved chain (`.chain.md`/`.chain.json`) by name (R-SA-129).
    /// Resolves the chain through the REAL discovery pipeline (R-SA-019/020), then routes
    /// into the identical `walk_chain` machinery `/chain` itself uses.
    async fn slash_run_chain(&self, args: &str, cwd: &Path) -> Result<String, SubagentError> {
        let parsed = slash_commands::parse_run_chain_command(args)
            .map_err(|e| SubagentError::MalformedSettings(e.message))?;
        let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before `resolve_chain`
        // below, which is a real discovery filesystem scan (R-SA-019/020) — so a blocked
        // call never touches discovery at all, not even for the saved-chain lookup this
        // command performs ahead of `run_or_background_chain`'s own (correct, but
        // necessarily later) independent re-check.
        let cfg = self.executor.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }
        let chain = self.executor.resolve_chain(cwd, &parsed.chain_name)?;
        // The functionality spec's own usage grammar (`/run-chain <chainName> -- <task>`)
        // gives no further detail on how the supplied task text combines with a saved
        // chain's own per-step task text beyond pi-subagents' `mapSavedChainSteps`
        // reference (`registration/slash_commands.rs`'s own module doc). The most complete
        // honest reading: the supplied task text seeds the FIRST step only (mirroring
        // `/chain`'s own "first element's task is what starts the chain" convention,
        // R-SA-053's own "cross-step data flows via named outputs from here forward"
        // model) — every later step keeps its saved, fixed task text verbatim.
        // A saved chain parses into `ChainStepConfig` authoring shapes (T0.2); lower each
        // to the runtime `RunnerStep` union here via the structural bridge — it carries the
        // real agent NAME (never a placeholder persona; name resolution stays the
        // executor's job) and defers plan-time model/acceptance enrichment. A group step's
        // omitted `concurrency` falls back to `cfg.parallel_concurrency()`, mirroring
        // `/parallel`'s own default above.
        let graph: Vec<RunnerStep> = chain
            .steps
            .iter()
            .map(|step| {
                crate::discovery::chains::chain_step_to_runner_step(
                    step,
                    cfg.parallel_concurrency(),
                )
            })
            .collect();
        let steps = seed_first_step_task(graph, &parsed.task);
        // `/run-chain <name> -- <task>`: the supplied task seeds the first step AND is the
        // run-wide `{task}` value (pi `originalTask = params.task`).
        let task = (!parsed.task.trim().is_empty()).then(|| parsed.task.clone());
        self.run_or_background_chain(cwd, steps, RunMode::Chain, context, parsed.flags.background, task)
            .await
    }

    /// /subagents-models — report the RUNTIME builtin-agent -> model mapping (pi
    /// `handleModels`, slash-commands.ts:802-823), NOT a dump of the static provider
    /// catalog: each discovered builtin persona's effective model + provenance, optionally
    /// filtered to one builtin.
    fn slash_models(&self, args: &str, cwd: &Path) -> Result<String, SubagentError> {
        let parsed = slash_commands::parse_subagents_models_command(args)
            .map_err(|e| SubagentError::MalformedSettings(e.message))?;
        Ok(self.executor.run_models_report(cwd, parsed.agent.as_deref()))
    }

    /// /subagents-refresh-provider-models — R-SA-129/142. The catalog-refresh ALGORITHM
    /// (probe scheduling, catalog diffing, observed/derived classification) is explicitly
    /// deferred (func-SA §9 item 31) — this crate has no provider-catalog CACHE FILE writer
    /// anywhere yet, only `registration/doctor.rs`'s freshness-checking READER
    /// (`provider_catalog_path`). The honest, most-complete implementation available today:
    /// validate the provider name (R-SA-142's path-traversal guard, since this name feeds
    /// the SAME cache-file path `doctor.rs` stats), confirm it resolves against the real
    /// built-in model registry, and write/refresh a minimal, genuinely-real freshness-cache
    /// marker file at the exact path `doctor.rs`'s own `check_provider_catalog_freshness`
    /// reads — so `/subagents-doctor`'s freshness check (R-SA-131 item f) observes a REAL
    /// effect of running this command, not a no-op. What remains explicitly OUT OF SCOPE
    /// (per the same deferred item): actually spawning a probe subprocess against the named
    /// provider's live API to discover/diff its real-time model list.
    async fn slash_refresh_provider_models(
        &self,
        args: &str,
        cwd: &Path,
    ) -> Result<String, SubagentError> {
        let parsed = slash_commands::parse_subagents_refresh_provider_models_command(args)
            .map_err(|e| SubagentError::MalformedSettings(e.message))?;
        self.refresh_provider_catalog_cache(cwd, &parsed.provider, parsed.force)
            .await
    }

    /// /subagents-generate-profiles — R-SA-129/140/141/142. Profile *authoring* (writing a
    /// NEW named-profile JSON file) is explicitly out of `registration/profiles.rs`'s
    /// documented scope (that module is read-only over an already-authored profiles
    /// directory — see its own module doc's "Deferred to a later phase" section) — full
    /// provider-catalog-driven profile GENERATION is the same deferred item as
    /// `/subagents-refresh-provider-models` ([`Self::slash_refresh_provider_models`], func-SA §9
    /// item 31). The honest, most-complete implementation available today: validate the provider
    /// name (R-SA-142), confirm it
    /// resolves against the real built-in model registry, and WRITE the two named profiles
    /// (`<provider>.quota`/`<provider>.quality`) this command's own usage string promises,
    /// selecting the catalog's cheapest/highest-capability model for that provider as the
    /// profile's `defaultModel` — a genuine, on-disk, load-through-`/subagents-load-profile`
    /// artifact, not a placeholder acknowledgement.
    async fn slash_generate_profiles(&self, args: &str) -> Result<String, SubagentError> {
        let provider = slash_commands::parse_subagents_generate_profiles_command(args)
            .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
        self.generate_provider_profiles(&provider).await
    }

    /// /subagents-check-profile — R-SA-129/140/141/142. Loads the named profile through the
    /// real `registration::profiles::load_profile` primitive and checks every
    /// `overrides.<agent>.model`/`defaultModel` value it declares against the real
    /// built-in model registry, reporting which model references are genuinely known vs.
    /// unresolvable — the honest, catalog-backed half of "still points to usable models" this command's
    /// own usage string promises; a genuine LIVE reachability probe against the provider's
    /// API is the same explicitly deferred item as the two commands directly above
    /// ([`Self::slash_refresh_provider_models`] and [`Self::slash_generate_profiles`]).
    async fn slash_check_profile(&self, args: &str) -> Result<String, SubagentError> {
        let name = slash_commands::parse_subagents_check_profile_command(args)
            .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
        let profiles_dir = self.profiles_dir();
        let profile = crate::registration::profiles::load_profile(&profiles_dir, &name)?;
        Ok(render_profile_check_report(&name, &profile).await)
    }

    /// /prompt-workflow — run one bundled/user/project `prompts/*.md` recipe (pi
    /// `prompt-workflows.ts:269-301` @v0.34.0). This is the reader that finally makes
    /// `registration::resources::bundled_prompt_files` reachable: the seven recipes this
    /// crate ships were discovered, unit-tested, and invocable by nothing.
    async fn slash_prompt_workflow(&self, args: &str, cwd: &Path) -> Result<String, SubagentError> {
        let mut words = prompt_workflows::shell_words(args);
        let name = if words.is_empty() { None } else { Some(words.remove(0)) };
        let workflows = prompt_workflows::discover_prompt_workflows(cwd);
        // pi `:275-278`: a bare `/prompt-workflow`, or the literal `list`, prints the list.
        let Some(name) = name.filter(|n| n != "list") else {
            return Ok(prompt_workflows::format_workflow_list(&workflows));
        };
        let Some(workflow) = prompt_workflows::find_workflow(&workflows, &name) else {
            // pi `:281` notifies `Unknown prompt workflow: {name}` as an error.
            return Err(SubagentError::MalformedSettings(format!(
                "Unknown prompt workflow: {name}"
            )));
        };
        let runtime = prompt_workflows::parse_runtime_options(&words);
        // pi `:286-295`: a recipe carrying `chain:` expands to a chain of OTHER recipes and
        // never runs its own body.
        if let Some(chain) = workflow.chain.as_deref() {
            let names = prompt_workflows::split_prompt_chain(chain);
            let steps = prompt_workflows::build_chain_steps(
                &workflows,
                &names,
                &runtime.args,
                &runtime,
                Some(&workflow.name),
            )
            .map_err(SubagentError::MalformedSettings)?;
            return self.run_prompt_workflow_chain(cwd, steps, &runtime).await;
        }
        let run = prompt_workflows::workflow_params(workflow, &runtime.args, &runtime);
        self.run_prompt_workflow_single(cwd, &run).await
    }

    /// /chain-prompts — the same recipes [`Self::slash_prompt_workflow`] runs, chained by an
    /// inline ` -> ` declaration (pi `prompt-workflows.ts:303-329` @v0.34.0).
    async fn slash_chain_prompts(&self, args: &str, cwd: &Path) -> Result<String, SubagentError> {
        let (declaration, args_text) = prompt_workflows::split_chain_declaration(args);
        let workflows = prompt_workflows::discover_prompt_workflows(cwd);
        // pi `:308-311`: an empty declaration, or the literal `list`, prints the list.
        if declaration.is_empty() || declaration == "list" {
            return Ok(prompt_workflows::format_workflow_list(&workflows));
        }
        let runtime =
            prompt_workflows::parse_runtime_options(&prompt_workflows::shell_words(&args_text));
        let names = prompt_workflows::split_prompt_chain(&declaration);
        if names.is_empty() {
            // pi `:315` — the usage line, verbatim.
            return Err(SubagentError::MalformedSettings(
                "Usage: /chain-prompts prompt-a -> prompt-b -- args".to_string(),
            ));
        }
        let steps = prompt_workflows::build_chain_steps(
            &workflows,
            &names,
            &runtime.args,
            &runtime,
            None,
        )
        .map_err(SubagentError::MalformedSettings)?;
        self.run_prompt_workflow_chain(cwd, steps, &runtime).await
    }

    // ---------------------------------------------------------------------------------------
    // /prompt-workflow + /chain-prompts execution (pi's `run:` callback, which both handlers
    // share — `slash-commands.ts:795-800` binds it to the SAME `runSlashSubagent` every other slash
    // command uses, so R-SA-130's single-executor rule holds for these two exactly as it does for
    // `/run` and `/chain`).
    // ---------------------------------------------------------------------------------------

    /// Run one non-chain recipe. Routes into the identical foreground/background entry points
    /// `/run` uses, carrying the recipe's own `model`/`skill`/`cwd` overrides.
    async fn run_prompt_workflow_single(
        &self,
        cwd: &Path,
        run: &prompt_workflows::WorkflowRun,
    ) -> Result<String, SubagentError> {
        // pi `resolveChildCwd(baseCwd, params.cwd)` (`shared/utils.ts:85-88`): a relative `cwd:`
        // resolves against the session cwd; an absolute one is taken as-is.
        let effective_cwd = match run.cwd.as_deref() {
            None => cwd.to_path_buf(),
            Some(child) => {
                let child = Path::new(child);
                if child.is_absolute() { child.to_path_buf() } else { cwd.join(child) }
            }
        };
        let model = run.model.clone().map(ModelId::from);

        // Same ordering as the `/run` arm above (SUBA-002): depth guard, then the per-session
        // spawn charge, then dispatch — a depth-blocked run must not be billed.
        let cfg = self.executor.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }
        self.executor
            .reserve_subagent_spawns(1, cfg.max_subagent_spawns_per_session)
            .map_err(SubagentError::SpawnLimitExceeded)?;

        if run.background {
            let run_id = self
                .executor
                .spawn_background(BackgroundSingleRequest {
                    // SUBA-021: the slash surfaces advertise no `usageBudget` param upstream either.
                    usage_budget: None,
                    // SUBA-008: same as the `/run` surface — no caller rung on this path; the
                    // frontmatter and config rungs are applied inside `spawn_background`.
                    turn_budget: None,
                    structured_output_schema: None,
                    tool_budget: None,
                    cwd: &effective_cwd,
                    agent_name: &run.agent,
                    task: &run.task,
                    context: run.context,
                    model_override: model,
                    agent_scope: AgentReadScope::Both,
                    acceptance: None,
                    include_progress: None,
                    control: None,
                    output: None,
                    output_mode: None,
                    // A recipe's `skill:` IS forwarded — unlike `/run`, whose upstream handler
                    // parses no `skill=` token (`slash-commands.ts:678-681`), `workflowParams`
                    // sets `skill` from the recipe's frontmatter (`prompt-workflows.ts:233`).
                    skills: run.skills.clone(),
                    share: None,
                    session_dir: None,
                    artifacts: None,
                    timeout_ms: None,
                })
                .await?;
            return Ok(format!("Background subagent run started: {run_id}"));
        }

        let result = self
            .executor
            // `run_foreground_impl` rather than the flat `run_foreground`: this surface DOES carry
            // per-call overrides (a recipe's `skill:`), which the flat entry point cannot express.
            // `on_update: None` matches `/run` — no host `ToolUpdateSink` reaches slash dispatch.
            .run_foreground_impl(
                ForegroundRunRequest {
                    overrides: SingleRunOverrides {
                        skills: run.skills.clone(),
                        ..SingleRunOverrides::default()
                    },
                    cwd: &effective_cwd,
                    agent_name: &run.agent,
                    task: &run.task,
                    agent_scope: AgentReadScope::Both,
                    context: run.context,
                    model_override: model,
                    timeout_ms: None,
                    cancel: CancelToken::new(),
                },
                None,
            )
            .await
            .map(|(result, _run_id)| result)?;
        Ok(format_slash_run_completion(&result))
    }

    /// Run an expanded recipe chain through the SAME `run_or_background_chain` walker `/chain`
    /// uses (pi lowers each recipe to a `ChainStep` and hands the whole `chain` array to the one
    /// executor, `prompt-workflows.ts:288-293`).
    async fn run_prompt_workflow_chain(
        &self,
        cwd: &Path,
        steps: Vec<prompt_workflows::WorkflowRun>,
        runtime: &prompt_workflows::RuntimeOptions,
    ) -> Result<String, SubagentError> {
        // pi `:293`/`:324`: the run-wide task is the joined positional args, and `clarify: false`/
        // `agentScope: "both"` are fixed — the chain's own context/async come from the runtime
        // flags, not from any single step.
        let task = runtime.args.join(" ");
        let context = if runtime.fork {
            Some(ContextMode::Fork)
        } else if runtime.fresh {
            Some(ContextMode::Fresh)
        } else {
            None
        };
        let graph: Vec<RunnerStep> = steps
            .iter()
            .map(|step| {
                RunnerStep::SingleStep(SingleStepSpec {
                    agent: step.agent.clone(),
                    task: step.task.clone(),
                    cwd: step.cwd.as_deref().map(PathBuf::from),
                    model: step.model.clone().map(ModelId::from),
                    // pi's `ChainStep` carries `skill` (`prompt-workflows.ts:246`), and cyrup's
                    // step spec has the same tri-state field, so a recipe's `skill:` survives into
                    // a chained step exactly as it does into a single run.
                    skills: step.skills.clone(),
                    session_dir: None,
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
                })
            })
            .collect();
        self.run_or_background_chain(
            cwd,
            graph,
            RunMode::Chain,
            context,
            runtime.bg,
            (!task.trim().is_empty()).then_some(task),
        )
        .await
    }

    // ---------------------------------------------------------------------------------------
    // /chain, /parallel, /run-chain shared foreground-vs-background dispatch (R-SA-129/130)
    // ---------------------------------------------------------------------------------------

    /// Shared tail for `/chain`, `/parallel`, and `/run-chain`: resolve every step's effective
    /// fork-context (R-SA-137's eager whole-batch rule) — an omitted call-site `context` defers to
    /// each step's agent's own `default_context`, and each forking step gets its OWN per-index branch
    /// (R-SA-138: a sibling step's own explicit choice is never overridden) — then either walk the
    /// graph to completion in the foreground or hand it to [`SubagentExecutor::spawn_background_steps`].
    pub(crate) async fn run_or_background_chain(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        mode: RunMode,
        context: Option<ContextMode>,
        background: bool,
        task: Option<String>,
    ) -> Result<String, SubagentError> {
        if graph.is_empty() {
            return Ok("chain has no steps to run".to_string());
        }

        // SUBA-002 — charge the per-SESSION spawn budget for the chain-shaped SLASH surfaces
        // (`/chain`, `/parallel`, `/run-chain`), which all funnel through this one wrapper. Upstream
        // needs no charge here because those handlers call `runSlashSubagent` -> `requestSlashRun`
        // -> the bridge at `extension/index.ts:512-517` -> `executeSubagentCollapsed` -> the SAME
        // `executor.execute` the tool uses, so its `reserveSubagentSpawns`
        // (`subagent-executor.ts:266-282`, called at `:3434-3441`) already covers them; this crate
        // reaches `run_or_background_graph` directly from here, so the reserve has to be repeated.
        //
        // Placed AFTER the empty-graph short-circuit and after the caller has fully resolved the
        // mode into a concrete `RunnerStep` list, so the number billed is exactly the number of
        // children this run can spawn (pi's own "count the settled mode, not the request shape"
        // ordering). It is NOT double-charged with the tool path: the `subagent` tool's own
        // `chain[]`/`tasks[]` shapes reserve once in `SubagentTool::execute` and then reach
        // `run_or_background_graph` via `route_chain_mode`/`route_parallel_mode`, never through this
        // slash-only wrapper.
        let budget_cfg = self.executor.config_snapshot().await;
        // SUBA-002 follow-up — the DEPTH guard must precede the charge. pi refuses on depth at
        // `subagent-executor.ts:3297-3312`, well ahead of `reserveSubagentSpawns` (`:3434-3441`),
        // so a dispatch the ceiling will reject never spends a spawn. `run_or_background_graph`'s
        // own R-SA-055 guard (the SAFETY-CRITICAL one, ahead of persona/fork IO) runs strictly
        // AFTER this reserve, so `/chain` and `/parallel` were billed and then refused — the
        // budget drained while nothing could ever be spawned. `/run-chain` was already immune: it
        // checks depth itself before `resolve_chain`. Re-checking here (a pure env+config read)
        // leaves the downstream guard untouched and changes no charge count.
        let depth = resolve_effective_depth(budget_cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }
        self.executor
            .reserve_subagent_spawns(
                count_graph_requested_spawns(&graph, &budget_cfg),
                budget_cfg.max_subagent_spawns_per_session,
            )
            .map_err(SubagentError::SpawnLimitExceeded)?;

        // R-SA-130: delegate to the ONE shared plan-execution path `SubagentExecutor` exposes (the
        // identical method the `subagent` tool's `chain[]`/`tasks[]` shapes route through), then
        // render the sequential/per-step text this slash surface presents. Depth guard, plan-time
        // persona resolution (T0.1/C13), fork-context resolution (R-SA-137), and the foreground-vs-
        // background fork all live inside `run_or_background_graph` now, so both call sites share
        // them verbatim rather than each re-implementing the tail.
        // The slash-command surface (`/chain`, `/parallel`, `/run-chain`) has no host
        // `ToolCallId`/cancellation seam of its own (`NativeExtension::execute_command` takes no
        // cancel token) — a fresh, never-cancelled token here preserves this path's pre-existing
        // behavior exactly; only the `subagent` TOOL's `execute` threads the live host token
        // (`SubagentTool::execute` -> `route_parallel_mode`/`route_chain_mode`).
        match self
            .executor
            .run_or_background_graph(
                cwd,
                graph,
                mode,
                context,
                background,
                task,
                CancelToken::new(),
                // The slash-command surface (`/chain`/`/parallel`/`/run-chain`) exposes no timeout
                // param at all (pi's `timeoutMs`/`maxRuntimeMs` are tool-only) — always `None`.
                None,
                // ...and no `control` token either (same rule, same reason): the extension-level
                // `subagents.control` block is still folded in one layer down.
                None,
                // ...and no `includeProgress` (SUBA-N06, same rule): the slash surface renders
                // text, never a `details` payload a progress snapshot could ride on.
                None,
                // ...and no `chainDir` (same rule again): pi reads it off the TOOL params in
                // `runChainPath`, and this surface has no such param to read.
                None,
            )
            .await?
        {
            GraphRunOutcome::Background(run_id) => {
                Ok(format!("Background subagent run started: {run_id}"))
            }
            GraphRunOutcome::Foreground {
                run_id: _,
                results,
                is_group,
                groups,
            } => Ok(render_chain_results(&results, &is_group, &groups)),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::extension::testsupport::seed_running_run;
    use crate::registration::slash_commands::SLASH_COMMANDS;

    #[test]
    fn slash_command_name_round_trips_every_registered_descriptor() {
        for descriptor in SLASH_COMMANDS {
            let parsed = SlashCommandName::from_str_exact(descriptor.name.as_str());
            assert_eq!(parsed, Some(descriptor.name));
        }
    }

    /// G77 — `/subagents-stop` is registered on the slash surface (pi
    /// `pi.registerCommand("subagents-stop", …)`, `slash-commands.ts:751-753` @v0.43.0) and its
    /// dispatch lands on the SAME `control_stop` entry point the tool action does, so the two
    /// surfaces can never diverge (R-SA-130).
    #[tokio::test]
    async fn the_subagents_stop_slash_command_is_registered_and_drives_the_same_stop() {
        assert_eq!(
            SlashCommandName::from_str_exact("subagents-stop"),
            Some(SlashCommandName::SubagentsStop),
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let paths = seed_running_run(dir.path(), "stopslash001", &["scout"]);
        let extension = SubagentsExtension::new();
        let rendered = extension
            .dispatch_slash(SlashCommandName::SubagentsStop, "stopslash001", dir.path(), false)
            .await
            .expect("/subagents-stop must dispatch");
        assert_eq!(rendered, "Stop requested for async run stopslash001.");
        assert!(
            crate::background::control::stop_request_path(&paths.run_dir).exists(),
            "the slash command must write the same real stop request the tool action does"
        );
    }

}
