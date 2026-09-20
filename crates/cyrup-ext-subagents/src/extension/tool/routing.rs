//! The `subagent` tool's dispatch table: one `route_*` arm per mode and per management/control
//! action.

use std::path::{Path, PathBuf};

use cyrup_core::{
    CancelToken, ModelId, TerminateHint, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};

use crate::background::{RunId, RunMode};
use crate::discovery::discover_agents;
use crate::exec::SingleResult;
use crate::exec::acceptance::lower_acceptance_input as parse_single_acceptance;
use crate::extension::executor::paths::async_launch_details;
use crate::extension::executor::requests::{
    BackgroundSingleRequest, ForegroundRunRequest, GraphRunOutcome, SingleRunOverrides,
    StatusViewSelector,
};
use crate::extension::host::slash_render::{
    describe_chain, format_async_started_message, plan_step_agent_names, render_chain_results,
};
use crate::extension::tool::SubagentTool;
use crate::extension::tool::lane_actions;
use crate::extension::tool::params::{
    SubagentToolParams, WATCHDOG_MUTATING_ACTION, foreground_timeout_default,
    format_failed_single_run_output, lower_launch_thinking, resolve_execution_agent_scope,
    resolve_foreground_timeout, validate_execution_acceptance,
};
use crate::extension::tool::task_items::{
    expand_top_level_task_counts, find_duplicate_parallel_output, normalize_skill_input,
    parse_tool_chain_items, parse_tool_task_items, render_parallel_tool_summary, tool_task_to_spec,
};
use crate::extension::tool::text::unknown_subagent_action_message;
use crate::fork_context::{ContextMode, ContextRequest};
use crate::spawn::chain_graph::{ParallelGroupSpec, RunnerStep, SingleStepSpec, StepResult};
use crate::spawn::depth::resolve_effective_depth;
use crate::watchdog::register_main::{watchdog_config_dirs, watchdog_model_info};

/// The resolved SINGLE-mode call [`SubagentTool::route_single`]'s prologue hands to
/// [`SubagentTool::route_single_background`].
///
/// Every field is *threaded* from that prologue rather than re-derived on the background branch —
/// `task`/`context`/`model` in particular are read off the ORIGINAL params, before the
/// `applySingleAgentLaunchDefaults` rebind, and `overrides` is the very bundle the foreground
/// branch goes on to consume, so the two paths cannot disagree about what a param meant. They
/// travel as one struct because passing them positionally would put the helper over clippy's
/// argument ceiling.
struct SingleBackgroundDispatch<'a> {
    /// This call's params, already carrying [`crate::extension::SubagentExecutor::single_agent_launch_defaults`]'
    /// `async:` fallback (the rebind that decided this branch was taken at all).
    p: &'a SubagentToolParams,
    cwd: &'a Path,
    agent: &'a str,
    task: &'a str,
    context: Option<ContextRequest>,
    model: Option<ModelId>,
    /// SUBA-041's override bundle, borrowed: hop 2 clones what it needs out of it.
    overrides: &'a SingleRunOverrides,
    /// Already validated + de-aliased by `resolve_foreground_timeout`.
    timeout_ms: Option<u64>,
    /// The surface this route already resolved for `agent`, threaded rather than re-resolved so a
    /// foreground reply and an async reply publish byte-identical `resolvedTools`. `None` when the
    /// name did not resolve — the existing "unknown agent" path owns that failure.
    tool_surface: Option<&'a crate::exec::tool_surface::ResolvedToolSurface>,
}

impl SubagentTool {
    /// The tool surface `agent` would launch with, or `None` when the name does not resolve.
    ///
    /// **Deliberately ceiling-free.** A capability ceiling can only ever NARROW a surface
    /// (`spawn_plan.rs`'s SUBA-072 `declared.retain(|t| allowed.contains(t))`) and can only ever
    /// ADD to `pinned`, so what is published here is a strict SUPERSET of what the launch itself
    /// resolves with the ceiling applied — a display path may over-report a tool the ceiling later
    /// removes, never under-report one. The ceiling is not resolvable here in any case: it hangs
    /// off [`crate::exec::RunOptions::parent_session_id`], which does not exist until the executor
    /// builds the run, and reaching for it would trade a sound approximation for a duplicated
    /// resolution.
    ///
    /// An unresolvable name returns `None` rather than an error: pi's own
    /// `applySingleAgentLaunchDefaults` bails silently on an unknown agent
    /// (`subagent-executor.ts:1932`) and leaves the existing "unknown agent" path to report it, and
    /// a second name-resolution error message here would race that one.
    fn resolve_launch_surface(
        &self,
        cwd: &Path,
        roots: &crate::paths::Roots,
        agent: &str,
        step_tools: Option<&Vec<crate::discovery::types::ToolRef>>,
        depth: crate::spawn::depth::DepthEnvelope,
    ) -> Option<crate::exec::tool_surface::ResolvedToolSurface> {
        let definition = self
            .executor
            .resolve_agent(
                cwd,
                agent,
                crate::discovery::types::AgentReadScope::Both,
                roots,
            )
            .ok()?;
        let mut config = crate::exec::AgentConfig::from_agent_definition(&definition, depth);
        // A per-step `tools:` OVERRIDES the persona's own list — `background/runner_main/
        // executor.rs`'s `if step.tools.is_some() { agent.tools = step.tools.clone(); }`. Both
        // production lowerings (`tool_task_to_spec`, `chain_step_to_runner_step`) hard-code `None`
        // today, so this is a no-op; it is here because a gate that read only the persona would
        // start returning WRONG verdicts the day either of them stops, and a wrong refusal is the
        // exact failure this gate exists to prevent.
        if let Some(tools) = step_tools {
            config.tools = Some(tools.clone());
        }
        // The HOST slot stays `None` here, unlike every launch path: this is a DISPLAY
        // path with no `RunOptions` in hand, and a surface rendered for `action:"list"` is a
        // statement about the AGENT, not about one launch's host snapshot. Intersecting it against
        // this process's live registry would make a listing's verdict depend on when it was asked.
        // `structured_output: false` for the same reason the HOST slot above is `None`: a listing has
        // no launch behind it, so it has no `outputSchema` either. pi's `internalTools`
        // (`child-tool-plan.ts:456`) is a grant the RUN makes, and there is no run here.
        crate::exec::tool_surface::resolve_tool_surface(&config, false, None, None, false, cwd).ok()
    }

    /// The comma-joined discovered agent names (or `"none"`) pi's "Provide exactly one mode. Agents:
    /// …" error lists (`subagent-executor.ts:1137`: `agents.map((a) => a.name).join(", ") ||
    /// "none"`). Discovery failures degrade to an empty list rather than propagating — this string
    /// is diagnostic-only context on an already-erroring path, never itself the primary failure.
    pub(crate) async fn discovered_agent_names_joined(&self, cwd: &Path) -> String {
        let names: Vec<String> = self
            .executor
            .discovery_config(cwd, &self.executor.config_snapshot().await.roots)
            .and_then(|cfg| discover_agents(&cfg, None))
            .map(|result| result.agents.into_iter().map(|a| a.name).collect())
            .unwrap_or_default();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    }

    /// pi `canonicalizeExecutionParams` (`subagent-executor.ts:1682-1734`, driven from `:4923-4925`
    /// right after `deps.discoverAgents(...)` and before `applySingleAgentLaunchDefaults`): rewrite
    /// EVERY agent name this dispatch names — the top-level SINGLE `agent`, each `tasks[i].agent`,
    /// each chain step's `agent`, each static-parallel step's `parallel[j].agent`, and a dynamic
    /// step's `parallel.agent` — to the CANONICAL name its alias-aware resolution yields.
    ///
    /// Canonicalizing here (rather than only inside each mode's own lookup) is what makes an alias
    /// invisible downstream: persona maps, chain-step bookkeeping, run status rows and result
    /// summaries all key off the step's `agent` string, so leaving an alias in place would report a
    /// run under a name that no agent file carries.
    ///
    /// An AMBIGUOUS name/alias aborts the whole dispatch with pi's message, suffixed with the
    /// per-site location pi appends (`(task 2)`, `(step 3, task 1)`) for everything except the
    /// top-level `agent`, which carries none.
    ///
    /// SUBA-086 — pi `canonicalizeAgentName` (`subagent-executor.ts:2336-2344` @v0.64.0) checks
    /// `findBlockingAgentDiagnostic` FIRST, before the ambiguity and unknown branches: a malformed
    /// definition that outranks every candidate the name resolved to aborts the dispatch with
    /// `Agent '<name>' has invalid configuration: <error>` (plus the same location suffix). That
    /// is what stops a broken project-tier `worker.md` from silently launching the builtin
    /// `worker`, and what turns a name with ONLY a broken definition into a real error instead of
    /// "not found".
    ///
    /// [CYRUP-DELTA — deliberate, narrow] pi's `canonicalizeAgentName` ALSO turns an unresolvable
    /// name into `Unknown agent: <name>` right here. cyrup leaves an unresolvable name UNTOUCHED
    /// (when no diagnostic claims it) and lets the existing per-mode resolution fail as it
    /// already does ([`crate::error::SubagentError::AgentNotFound`] -> `agent not found: <name>`):
    /// the not-found WORDING is a pre-existing, separate difference from pi that this crate's own
    /// tests pin, and changing it here would be an unrelated behavioural edit smuggled into the
    /// alias port. The alias-resolution and ambiguity-refusal halves — the parts this port owns —
    /// are complete.
    pub(crate) async fn canonicalize_execution_params(
        &self,
        params: &SubagentToolParams,
        cwd: &Path,
    ) -> Result<Option<SubagentToolParams>, ToolError> {
        // Nothing to canonicalize on a dispatch that names no agent anywhere.
        if params.agent.is_none() && params.tasks.is_none() && params.chain.is_none() {
            return Ok(None);
        }
        // A discovery failure here is not this step's to report: the mode arm below re-runs the same
        // discovery and surfaces the real error (a malformed `settings.json` MUST abort, R-SA-009,
        // and it will — one call later, with its own message). Degrading to "no canonicalization"
        // keeps this step from turning one error into two different ones.
        let Ok(discovered) = self
            .executor
            .discovery_config(cwd, &self.executor.config_snapshot().await.roots)
            // Same scope the mode arms resolve under (pi canonicalizes against the very
            // `discoverAgents(effectiveCwd, scope)` result the executor then uses,
            // `subagent-executor.ts:4921-4923`), so an alias can never resolve here to an agent the
            // run itself would not have been allowed to see.
            .and_then(|cfg| {
                discover_agents(
                    &cfg,
                    Some(resolve_execution_agent_scope(params.agent_scope.as_deref())),
                )
            })
        else {
            return Ok(None);
        };
        let agents = &discovered.agents;

        // `canonicalizeAgentName` + the `location` suffix (`subagent-executor.ts:2336-2350`
        // @v0.64.0): `result.error && location ? { error: `${result.error} (${location})` } :
        // result`.
        let with_location = |msg: String, location: Option<String>| match location {
            Some(loc) => format!("{msg} ({loc})"),
            None => msg,
        };
        let resolve = |name: &str, location: Option<String>| -> Result<Option<String>, ToolError> {
            let resolution = crate::discovery::resolve_agent_name(name, agents);
            // SUBA-086: the blocking-diagnostic check comes BEFORE `resolved.error` /
            // `!resolved.agent` (`subagent-executor.ts:2337-2341`).
            let candidates = crate::discovery::blocking_candidates(name, agents, &resolution);
            if let Some(diagnostic) = crate::discovery::find_blocking_agent_diagnostic(
                name,
                &candidates,
                &discovered.agent_diagnostics,
            ) {
                return Err(ToolError::new(with_location(
                    format!(
                        "Agent '{name}' has invalid configuration: {}",
                        diagnostic.error
                    ),
                    location,
                )));
            }
            match resolution {
                crate::discovery::AgentNameResolution::Found(agent) => Ok(Some(agent.name.clone())),
                crate::discovery::AgentNameResolution::Ambiguous(msg) => {
                    Err(ToolError::new(with_location(msg, location)))
                }
                // See the CYRUP-DELTA above: pass through, do not manufacture a not-found here.
                crate::discovery::AgentNameResolution::NotFound => Ok(None),
            }
        };

        /// Rewrite `value["agent"]` in place when it is a string that canonicalizes to something
        /// different. Returns `true` iff anything changed.
        fn set_agent(value: &mut serde_json::Value, canonical: String) {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("agent".to_string(), serde_json::Value::String(canonical));
            }
        }

        let mut next = params.clone();
        let mut changed = false;

        if let Some(agent) = params.agent.as_deref()
            && let Some(canonical) = resolve(agent, None)?
            && canonical != agent
        {
            next.agent = Some(canonical);
            changed = true;
        }

        if let Some(tasks) = next.tasks.as_mut() {
            for (index, task) in tasks.iter_mut().enumerate() {
                let Some(name) = task.get("agent").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let name = name.to_string();
                if let Some(canonical) = resolve(&name, Some(format!("task {}", index + 1)))?
                    && canonical != name
                {
                    set_agent(task, canonical);
                    changed = true;
                }
            }
        }

        if let Some(chain) = next.chain.as_mut() {
            for (index, step) in chain.iter_mut().enumerate() {
                // Static-parallel step: `parallel` is an ARRAY of task objects.
                if let Some(parallel) = step
                    .get_mut("parallel")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    for (task_index, task) in parallel.iter_mut().enumerate() {
                        let Some(name) = task.get("agent").and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        let name = name.to_string();
                        if let Some(canonical) = resolve(
                            &name,
                            Some(format!("step {}, task {}", index + 1, task_index + 1)),
                        )? && canonical != name
                        {
                            set_agent(task, canonical);
                            changed = true;
                        }
                    }
                    continue;
                }
                // Dynamic-fanout step: `parallel` is a single template OBJECT carrying one `agent`.
                if let Some(template) = step.get("parallel").filter(|v| v.is_object()) {
                    let Some(name) = template.get("agent").and_then(serde_json::Value::as_str)
                    else {
                        continue;
                    };
                    let name = name.to_string();
                    if let Some(canonical) = resolve(&name, Some(format!("step {}", index + 1)))?
                        && canonical != name
                        && let Some(template) = step.get_mut("parallel")
                    {
                        set_agent(template, canonical);
                        changed = true;
                    }
                    continue;
                }
                // Ordinary sequential step.
                let Some(name) = step.get("agent").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let name = name.to_string();
                if let Some(canonical) = resolve(&name, Some(format!("step {}", index + 1)))?
                    && canonical != name
                {
                    set_agent(step, canonical);
                    changed = true;
                }
            }
        }

        Ok(if changed { Some(next) } else { None })
    }

    /// pi `resolveRequestedCwd` (`subagent-executor.ts:348-350`): an explicit `params.cwd` is
    /// resolved AGAINST this tool's runtime cwd (`path.resolve(runtimeCwd, requestedCwd)` — a
    /// relative `requestedCwd` is joined onto `runtimeCwd`; an absolute one replaces it outright,
    /// which is exactly [`Path::join`]'s own behavior for an absolute argument); an omitted `cwd`
    /// is the runtime cwd unchanged. This becomes the SINGLE `effectiveCwd`/`requestCwd` value pi
    /// threads into every dispatch arm — execution, resume, append-step, status, interrupt, doctor,
    /// models, and management CRUD alike (`subagent-executor.ts:348-350,4121,4334` @v0.43.0).
    pub(crate) fn resolve_requested_cwd(&self, requested: Option<&str>) -> PathBuf {
        match requested {
            Some(requested) if !requested.is_empty() => self.cwd.join(requested),
            _ => self.cwd.clone(),
        }
    }

    /// SINGLE mode (`{agent, task?}`) — the fully-wired shape (func-SA §5.2). Resolves the persona
    /// through real discovery and drives [`crate::extension::SubagentExecutor::run_foreground`]/[`crate::extension::SubagentExecutor::spawn_background`]
    /// (`async: true`), each a genuine child OS process. `context` selects fork/fresh (an omitted
    /// value is `Fresh` in this tier); `model` is the per-call override.
    ///
    /// SUBA-041 — the per-call override surface pi's `runSinglePath` honors
    /// (`subagent-executor.ts:3561-3564` output/outputMode/skill, `:2962` acceptance, `:2874` share,
    /// `:3387-3401` artifacts/sessionDir, `:1179` control) now reaches [`crate::exec::RunOptions`] through
    /// [`SingleRunOverrides`] instead of being rejected wholesale. `includeProgress` — the one
    /// remaining param with no subsystem behind it — is absent from the tool schema and still
    /// refused here, so the schema never promises what this dispatcher declines.
    ///
    /// SUBA-N05 moved `control` out of that refusal and into [`SingleRunOverrides::control`], on
    /// BOTH paths: the foreground run resolves it into `RunOptions::control_config` alongside the
    /// notice notifier, and the background run carries it to hop 2 on
    /// [`BackgroundSingleRequest::control`]. It had been the crate's one remaining
    /// advertised-and-silently-dropped param on the async side — parsed, not on the foreground-only
    /// refusal list, and with nowhere to go.
    pub(crate) async fn route_single(
        &self,
        p: &SubagentToolParams,
        cwd: &Path,
        on_update: ToolUpdateSink,
        cancel: CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let Some(agent) = p.agent.as_deref() else {
            return Err(ToolError::new(
                "subagent SINGLE mode requires an 'agent' name (supply 'tasks' for PARALLEL, \
                 'chain' for CHAIN, or 'action' for a management/control action instead).",
            ));
        };
        let task = p.task.clone().unwrap_or_default();
        let context = p.context_override();
        let model = p.model.clone().map(ModelId::from);

        // pi's own `validateFileOnlyOutputMode` gate (`single-output.ts:140-145`, applied at
        // `subagent-executor.ts:2883-2886`) fires AFTER the persona is resolved, because a persona's
        // own `output:` can satisfy `outputMode: "file-only"` on its own. cyrup already enforces the
        // identical invariant one layer down at the same point in the sequence — `run_sync`'s
        // R-SA-025 `validate_file_only_requires_path` fail-fast, ahead of any spawn — so it is
        // deliberately NOT duplicated here where the persona default is not yet known.

        // pi `applySingleAgentLaunchDefaults` (`subagent-executor.ts:1929-1946`): a SINGLE-agent
        // launch inherits the named agent's OWN `async:`/`timeoutMs:` frontmatter defaults —
        // strictly as a fallback. The precedence is fill-unset-only and asymmetric, and getting it
        // wrong in the other direction (an agent default silently overriding an explicit call-site
        // argument) is worse than not having the feature, so each rule is spelled out:
        //
        //  * `async` applies ONLY when the call omitted `async` entirely — an explicit
        //    `async: false` beats an agent that defaults to true;
        //  * `timeoutMs` applies ONLY when the call omitted BOTH `timeoutMs` AND its alias
        //    `maxRuntimeMs` (`:1937`);
        //  * neither applies to a chain/parallel launch (`:1930` bails on `chain`/`tasks`) — this
        //    is `route_single`, which is only ever reached for a single named agent;
        //  * an unresolvable agent name changes nothing (`:1932`), leaving the existing
        //    "unknown agent" error path to report it;
        //  * SUBA-082: `acceptance` applies ONLY when the call omitted `acceptance` entirely
        //    (`subagent-executor.ts:2690-2692` @v0.64.0) — an explicit call-site policy, `"auto"`
        //    included, beats the agent's `acceptance:` frontmatter default. It is folded into the
        //    params HERE, before `single_run_overrides` lowers `p.acceptance`, so the default
        //    reaches both the foreground and the background request through the ordinary
        //    explicit-policy path and never touches chain/parallel steps.
        //
        // G98: the RESOLUTION now lives on the executor
        // ([`crate::extension::SubagentExecutor::single_agent_launch_defaults`]) so the `/run` slash surface — an
        // independent entry point that never reaches this dispatcher — applies the same defaults.
        // Only the fill-unset-only APPLICATION rules stay here, where the "was it supplied?"
        // question can actually be asked of this call's params.
        let launch_defaults = self.executor.single_agent_launch_defaults(
            cwd,
            agent,
            &self.executor.config_snapshot().await.roots,
        );

        // pi resolves `effectiveAsync` against the live config's `asyncByDefault`/
        // `forceTopLevelAsync` and this call's own depth (`applyForceTopLevelAsyncOverride`,
        // `subagent-executor.ts:3318-3322,3382` @v0.34.0) — never a hardcoded `false` default. The agent's
        // own `async:` default is folded in FIRST (pi rewrites `params.async` before
        // `applyForceTopLevelAsyncOverride` ever sees it), so `forceTopLevelAsync` still wins over
        // an agent that declares `async: false`, exactly as upstream.
        let cfg = self.executor.config_snapshot().await;
        let envelope = resolve_effective_depth(cfg.max_subagent_depth);
        let depth = envelope.current_depth;

        // Resolved ONCE and kept: the async arm republishes this exact value as the launch reply's
        // `resolvedTools`, so the parent learns what the child it just started can actually do
        // without waiting for it to settle.
        let launch_surface = self.resolve_launch_surface(cwd, &cfg.roots, agent, None, envelope);

        let p_with_defaults;
        let fill_async = launch_defaults.0.filter(|_| p.r#async.is_none());
        let fill_acceptance = launch_defaults.3.clone().filter(|_| p.acceptance.is_none());
        let p: &SubagentToolParams = if fill_async.is_some() || fill_acceptance.is_some() {
            p_with_defaults = SubagentToolParams {
                r#async: fill_async.or(p.r#async),
                acceptance: fill_acceptance.or_else(|| p.acceptance.clone()),
                ..(*p).clone()
            };
            &p_with_defaults
        } else {
            p
        };

        let overrides = Self::single_run_overrides(p)?;
        // SUBA-077: hoisted out of the `if` below because the timeout ladder needs it — the
        // foreground backstop is gated on this launch NOT being async.
        let background = p.is_background(&cfg, depth);

        // pi `resolveForegroundTimeout` (`subagent-executor.ts:2689` @v0.57.0): `timeoutMs`/
        // `maxRuntimeMs` are aliases; validate up front (positive, and consistent when both given).
        // The default rungs are applied only when that resolution produced nothing, which is
        // exactly pi's `params.timeoutMs === undefined && params.maxRuntimeMs === undefined` guard
        // — and they run AFTER validation so an invalid explicit value still errors rather than
        // being silently replaced.
        //
        // SUBA-077 / pi `resolveSingleAgentLaunchTimeout` (`:2719-2725`): the agent's frontmatter
        // rung is now the FIRST rung of that default rather than a trailing `.or(launch_defaults.1)`
        // on the result. It cannot stay a trailing `.or()`: the default would already have filled
        // the value, leaving an agent's `timeoutMs:` permanently unreachable.
        //
        // The built-in backstop is gated on `!background`, which is upstream's own `!async` arm. An
        // async single launch already picks up `DEFAULT_ASYNC_CHILD_TIMEOUT_MS` downstream at
        // `extension/executor/background.rs`'s `timeout_ms.unwrap_or(…)`; handing that `unwrap_or`
        // a `Some` on every run would silently retire it — harmless while the two constants agree,
        // a trap the moment either moves.
        let timeout_ms = resolve_foreground_timeout(
            p,
            foreground_timeout_default(background, launch_defaults.1, cfg.timeout_ms.as_ref()),
        )
        .map_err(ToolError::new)?;

        if background {
            return self
                .route_single_background(SingleBackgroundDispatch {
                    p,
                    cwd,
                    agent,
                    task: &task,
                    context,
                    model,
                    overrides: &overrides,
                    timeout_ms,
                    tool_surface: launch_surface.as_ref(),
                })
                .await;
        }

        // C19: stream live foreground progress through the host `ToolUpdateSink` — the child's
        // NDJSON stdout is folded into `SubagentUpdatePayload` progress updates as it arrives,
        // instead of the model/UI seeing nothing until the run completes.
        let (result, run_id) = self
            .executor
            .run_foreground_streaming(
                ForegroundRunRequest {
                    // SUBA-041: the seven wired SINGLE-mode overrides.
                    overrides,
                    cwd,
                    agent_name: agent,
                    task: &task,
                    agent_scope: resolve_execution_agent_scope(p.agent_scope.as_deref()),
                    context,
                    model_override: model,
                    timeout_ms,
                    cancel,
                    // A plain SINGLE-mode dispatch, never a workflow child.
                    parent_workflow_run_id: None,
                    workflow_key: None,
                    workflow_steer: None,
                },
                on_update,
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?;

        self.render_single_result(result, run_id, agent, context, p.include_progress)
            .await
    }

    /// The WORKFLOW mode arm — refuse async, validate, run, render (WORKFLOW_2).
    ///
    /// The fourth execution mode, and the SECOND streaming one: like [`Self::route_single`] it
    /// takes `on_update`, because a workflow's children stream live progress through
    /// `run_foreground_streaming` exactly as a SINGLE run's child does.
    /// [`Self::route_parallel_mode`]/[`Self::route_chain_mode`] take no sink at all.
    pub(crate) async fn route_workflow_mode(
        &self,
        call_id: &ToolCallId,
        p: &SubagentToolParams,
        cwd: &Path,
        // WORKFLOW_20 — the mission this dispatch is bound to, ALREADY resolved by
        // `tool/mod.rs`'s single `prepare_mission_binding_for_dispatch` seam, which runs for every
        // mode before the mode gate. It is passed DOWN rather than re-resolved here because
        // `prepare_mission_launch` is side-effecting (it marks an attached mission Active, and
        // CREATES one for a `mission: {...}` object) — resolving a second time would write twice
        // per tool call. Its `location` + `mission_id` are exactly the two values a mission
        // scratchpad needs.
        mission: Option<&crate::missions::MissionLaunchBinding>,
        on_update: ToolUpdateSink,
        cancel: CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let Some(script) = p.workflow_script.as_deref() else {
            return Err(ToolError::new(
                "subagent WORKFLOW mode requires a non-empty 'workflowScript'.",
            ));
        };

        // The async refusal is permanent (§0.6): `runs.all` already provides real in-workflow
        // concurrency (fan-out width is free; only depth costs, and `RunMode::Chain` handles that).
        // Async would buy only detachment, which this build does not implement for Workflow.
        //
        // Refused BEFORE validation, not after: an `async: true` call is rejected whatever the
        // script says, and spending a V8 isolate on a compile check for a call that cannot run is
        // waste.
        if p.r#async == Some(true) {
            return Err(ToolError::new(
                "workflowScript always runs in the foreground: runs.all already provides real in-workflow \
                 concurrency, so async would buy detachment only. Omit async or pass async:false.",
            ));
        }

        // WORKFLOW_20 — pi `subagent-executor.ts:4139`, the `workflowScript` branch's ONE caller
        // of `createMissionWorkflowState`. A workflow bound to a mission gets that mission's
        // durable scratchpad (`<missionDir>/<id>/state.json`, surviving the run AND the process);
        // an unbound one gets no `state` global at all, and the analyzer says so by name.
        //
        // Built unconditionally for a bound mission, even when the script never mentions `state`:
        // `MissionWorkflowState` is lazy (`values: None` until first access), so this costs one
        // mission-id validation and zero I/O — and deferring it is not an option anyway, because
        // `state_enabled` must be known BEFORE the validation below.
        let state: Option<std::sync::Arc<dyn crate::workflows::scripted::WorkflowStateStore>> =
            match mission {
                Some(binding) => Some(std::sync::Arc::new(
                    crate::missions::MissionWorkflowStateStore::create(
                        &binding.location,
                        &binding.mission_id,
                    )
                    .map_err(|e| ToolError::new(e.to_string()))?,
                )),
                None => None,
            };
        // ONE derivation, TWO consumers that must never disagree: the analyzer below and the
        // engine's own `state_enabled` (which it derives from `options.state.is_some()` and feeds
        // to `__cyrupWorkflowInstall`, `prelude.js:578`). Two literals is how a build ends up
        // ACCEPTING a `state.get(...)` the run then refuses at the op — the worst of both — or
        // refusing one it would have served.
        let state_enabled = state.is_some();

        // Structural validation BEFORE a child is spent — this port's stated advantage over
        // upstream, which discovers a bad global as a runtime ReferenceError after the fact.
        let report = crate::workflows::scripted::validate_workflow_script(script, state_enabled)
            .map_err(ToolError::new)?;
        if !report.ok {
            // The findings are re-prompts. ONE formatter, shared with the `validate` action arm —
            // two copies is how the two surfaces drift.
            return Err(ToolError::new(render_validation_findings(&report)));
        }

        // The same foreground timeout ladder a SINGLE run is bounded by, so a configured
        // `subagents.timeoutMs` applies to workflows too. The agent rung is `None`: a workflow
        // names no single persona to read frontmatter from. `background` is `false` — the async
        // shape was refused above. `None` here would still be bounded, by
        // `WORKFLOW_DEFAULT_TIMEOUT_MS` (30 min), but then the configured value would silently not
        // apply.
        let cfg = self.executor.config_snapshot().await;
        let timeout_ms = resolve_foreground_timeout(
            p,
            foreground_timeout_default(false, None, cfg.timeout_ms.as_ref()),
        )
        .map_err(ToolError::new)?;

        // WORKFLOW_6/WORKFLOW_3 §3d + SUBA-016 — the launch itself lives in
        // [`crate::extension::executor::workflow_launch`], because a scheduled fire needs the SAME
        // block with the tool-call inputs stubbed and a second copy would diverge on the first of
        // the five behaviours (controller registration, `register_stop_child`, `on_emit`,
        // `on_host_step`, the terminal receipt write) that someone forgot. This arm supplies the
        // tool-call halves: the real `call_id`, the real progress sink, and the LIVE session read
        // that a scheduled fire replaces with a pinned snapshot.
        let prepared = crate::extension::executor::workflow_launch::prepare_workflow_run(
            crate::extension::executor::workflow_launch::PrepareWorkflowRun {
                cfg: &cfg,
                cwd,
                session_id: crate::identity::SessionId::parse_opt(
                    self.executor.current_session_id().as_deref(),
                ),
                tool_call_id: Some(call_id.as_str().to_string()),
                // A tool call delivers its own result as the `ToolResult` it returns, so nothing
                // ever consults the owner. Unchanged from the inline block.
                completion_owner_id: None,
            },
        )
        .await?;
        crate::extension::executor::workflow_launch::drive_workflow_run(
            &self.executor,
            prepared,
            crate::extension::executor::workflow_launch::DriveWorkflowRun {
                cfg: &cfg,
                cwd,
                script,
                timeout_ms,
                state,
                on_update,
                cancel,
            },
        )
        .await
    }

    /// Lower every advertised SINGLE-mode param into the one [`SingleRunOverrides`] bundle both of
    /// [`Self::route_single`]'s branches consume.
    ///
    /// SUBA-041, as amended by SUBA-N05 and then SUBA-N06: [`Self::route_single`] no longer refuses
    /// ANY advertised SINGLE-mode parameter outright.
    ///
    /// `control` came off the list in SUBA-N05. It was there for exactly one reason — this port
    /// had `registration::ControlConfig`'s SHAPE but neither `resolveControlConfig` nor the
    /// control-notice pipeline behind it — and that reason is gone: `exec::control` now ports
    /// `runs/shared/subagent-control.ts` in full, `run_sync` raises real `ControlEvent`s off the
    /// child's NDJSON stream, and `SubagentExecutor::foreground_control_notifier` feeds them to
    /// the (previously producer-less) `tui::notices::ControlNoticeState`.
    ///
    /// `includeProgress` came off in SUBA-N06, and for the same shape of reason. It was refused
    /// because `SingleResult` carried no progress object to include or omit — R-SA-043
    /// compaction with no opt-out. It now has one: `exec::AgentProgress::snapshot` projects the
    /// winning attempt's fold into pi's `AgentProgress` wire shape and `run_sync` publishes it
    /// on `SingleResult::progress` when — and only when — this flag is `Some(true)`, matching
    /// pi's `progress: params.includeProgress ? allProgress : undefined`
    /// (`subagent-executor.ts:3819` @v0.43.0). It is advertised in the tool schema again and
    /// honoured on BOTH the foreground path (`SingleRunOverrides::include_progress` →
    /// `RunOptions::include_progress`) and the async one (`BackgroundSingleRequest::
    /// include_progress` → `RunnerConfig::include_progress` → every hop-2 step's `RunOptions`).
    ///
    /// `chainDir` is CHAIN-mode-only in pi (it resolves `{chain_dir}` for chain steps) so it is
    /// not gated here for SINGLE mode.
    ///
    /// SUBA-041: the SINGLE-mode overrides pi's `runSinglePath` honors, resolved here and
    /// carried into `run_foreground_impl` as one bundle. `acceptance` is validated up front
    /// through pi's own `validateAcceptanceInput` (`subagent-executor.ts:1418`) so a malformed
    /// policy is refused BEFORE agent resolution and before any child spawns.
    /// `pub(crate)` since WORKFLOW_2: [`crate::extension::executor::workflow::WorkflowRunHost`]
    /// lowers a workflow child's launch params through this SAME function, so a workflow child
    /// accepts exactly the surface a `tasks[]` child does. It lives in a sibling module subtree,
    /// which a private inherent `fn` is not reachable from.
    pub(crate) fn single_run_overrides(
        p: &SubagentToolParams,
    ) -> Result<SingleRunOverrides, ToolError> {
        Ok(SingleRunOverrides {
            // SUBA-008 / pi `resolveTurnBudgetConfig(effectiveParams.turnBudget …)`
            // (`subagent-executor.ts:4928`): a malformed budget is a hard refusal at the tool
            // boundary with upstream's own message, not a silent downgrade to "unbudgeted". The
            // frontmatter and config rungs below it are applied by `run_foreground_impl`.
            turn_budget: crate::exec::turn_budget::resolve_turn_budget_config(
                p.turn_budget.as_ref(),
                "turnBudget",
            )
            .map_err(ToolError::new)?,
            // SUBA-021 / pi `validateUsageBudgetConfig(params.usageBudget)`: same seam, same
            // fail-closed rule — a malformed budget is a hard refusal carrying upstream's own
            // sentence, never a silent downgrade to an unbudgeted run.
            usage_budget: crate::exec::usage_budget::validate_usage_budget_config(
                p.usage_budget.as_ref(),
                "usageBudget",
            )
            .map_err(ToolError::new)?,
            output: p.output.clone(),
            output_mode: p.output_mode.clone(),
            skills: normalize_skill_input(p.skill.as_ref()),
            acceptance: match p.acceptance.as_ref() {
                Some(raw) => parse_single_acceptance(raw).map_err(ToolError::new)?,
                None => None,
            },
            // SCOPE_19/A1 — the caller's reasoning level for the child (rung 2 of the launch
            // resolution ladder; `run_foreground_impl` owns the fold). Validated HERE so an
            // unrecognized level is refused at the tool boundary — this param used to be accepted
            // on launches and silently discarded, and a named refusal is the contract that
            // replaces that.
            thinking: lower_launch_thinking(p.thinking.as_deref()).map_err(ToolError::new)?,
            share: p.share,
            session_dir: p.session_dir.clone(),
            artifacts: p.artifacts,
            // SUBA-N05 / pi `effectiveParams.control` (`subagent-executor.ts:1179`). Lowered
            // tolerantly (a wrong-typed threshold or an unknown `notifyOn` string degrades to
            // "that field was not supplied", exactly as `parsePositiveInt`/`parseControlList` do)
            // rather than hard-failing the call — see `parse_control_overrides`.
            control: p
                .control
                .as_ref()
                .map(crate::exec::control::parse_control_overrides),
            // SUBA-N06 / pi `params.includeProgress`. Passed through untouched: `run_sync` applies
            // pi's own `? :` truthiness gate, so an explicit `false` behaves exactly like an
            // omitted flag.
            include_progress: p.include_progress,
            // WORKFLOW_2 — empty for every tool-driven dispatch: no advertised tool parameter sets
            // the child's environment, and none should. The one production producer is
            // `WorkflowRunHost`, which inserts `WORKFLOW_CHILD_ENV` AFTER calling this function.
            child_env: std::collections::HashMap::new(),
            // SUBA-043 / pi `params.outputSchema` (`subagent-executor.ts:3651,3671` @v0.43.0).
            output_schema: p.output_schema.clone(),
            // SUBA-047 / pi `validateToolBudgetConfig(params.toolBudget, "toolBudget")`
            // (`async-execution.ts:1299`): a malformed budget is a hard refusal at the tool
            // boundary, with pi's own message text, not a silent downgrade to "unbudgeted".
            tool_budget: crate::exec::tool_budget::validate_tool_budget_config(
                p.tool_budget.as_ref(),
                "toolBudget",
            )
            .map_err(ToolError::new)?,
        })
    }

    /// [`Self::route_single`]'s background branch: hand the call to
    /// [`crate::extension::SubagentExecutor::spawn_background`] and return pi's async-started receipt.
    ///
    /// SUBA-N03 — there is NO foreground-only refusal on this branch any more, because
    /// every one of the nine advertised SINGLE-mode params now genuinely reaches hop 2.
    ///
    /// **The refusal that stood here cited a precedent that does not exist.** Its comment
    /// read "mirrors pi's own precedent of erroring on timeoutMs + async
    /// (subagent-executor.ts:3022)". At v0.34.0, `subagent-executor.ts:3015-3030` is
    /// FOREGROUND intercom-receipt construction (`maybeBuildForegroundIntercomReceipt`) and
    /// has nothing to do with timeouts or async; `git grep` over the whole of v0.34.0 `src/`
    /// finds no timeout-vs-async refusal anywhere in the package. Upstream does the exact
    /// OPPOSITE — `extension/schemas.ts:265-266` and `extension/tool-description.ts:25,:73`
    /// each state that `timeoutMs`/`maxRuntimeMs` apply to "foreground and async/background
    /// runs", and `runs/background/async-execution.ts:924` arms
    /// `deadlineAt = Date.now() + params.timeoutMs` for `executeAsyncSingle`, which
    /// `subagent-runner.ts:2078-2081` turns into a live run-level timer.
    ///
    /// The other eight were refused for one honest reason, now removed: the second-hop
    /// `RunnerConfig` boundary was strictly NARROWER than the foreground `RunOptions`, so a
    /// param accepted here would have been dropped on the floor by the detached runner —
    /// the advertised-and-silently-dropped defect SUBA-041 exists to prevent. Widening that
    /// boundary to upstream's own field set (`spawnRunner({ …, share, sessionDir,
    /// artifactsDir, artifactConfig, timeoutMs, deadlineAt, … })` plus the step's own
    /// `outputPath`/`outputMode`/`skills`, `async-execution.ts:930-996`) was always the work
    /// the refusal was standing in for, and it is now done:
    ///
    /// * `output`/`outputMode` -> resolved parent-side against the run-scoped output base
    ///   dir (`resolve_single_run_output_base_dir`, pi `:2203-2207`) onto the step's
    ///   `output_path`/`output_mode`;
    /// * `skill` -> the step's new `skills` field;
    /// * `sessionDir` -> resolved parent-side onto the step's new `session_dir`;
    /// * `share`/`artifacts`/`timeoutMs` -> new `RunnerConfig` fields;
    /// * `acceptance` (SUBA-N04), `control` (SUBA-N05) and `includeProgress` (SUBA-N06) were
    ///   wired by their own units and are unchanged here.
    ///
    /// This mattered beyond the explicit `async: true` call: `asyncByDefault` /
    /// `forceTopLevelAsync` route EVERY top-level `subagent` call down this branch, so the
    /// refusal made nine schema-advertised params unusable for whole configurations.
    async fn route_single_background(
        &self,
        dispatch: SingleBackgroundDispatch<'_>,
    ) -> Result<ToolResult, ToolError> {
        let SingleBackgroundDispatch {
            p,
            cwd,
            agent,
            task,
            context,
            model,
            overrides,
            timeout_ms,
            tool_surface,
        } = dispatch;
        let run_id = self
            .executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021 / pi `usageBudget: params.usageBudget` on the async SINGLE step builder
                // (`async-execution.ts:1471`): the same validated caller rung the foreground path uses.
                usage_budget: crate::exec::usage_budget::validate_usage_budget_config(
                    p.usage_budget.as_ref(),
                    "usageBudget",
                )
                .map_err(ToolError::new)?,
                // SUBA-008 / pi `resolveTurnBudgetConfig(effectiveParams.turnBudget …)`
                // (`subagent-executor.ts:4928`): the async SINGLE branch validates the caller's
                // budget exactly as the foreground branch does, so `{…, turnBudget, async:true}`
                // is neither silently unbudgeted nor silently lenient.
                turn_budget: crate::exec::turn_budget::resolve_turn_budget_config(
                    p.turn_budget.as_ref(),
                    "turnBudget",
                )
                .map_err(ToolError::new)?,
                // SUBA-043 / pi `params.outputSchema` (`extension/schemas.ts:351` @v0.43.0):
                // the async SINGLE branch forwards the top-level schema exactly as the
                // foreground branch does, so `{agent, task, outputSchema, async:true}` is not a
                // silently weaker call than the same request without `async`.
                structured_output_schema: p.output_schema.clone(),
                // SUBA-047 / pi `validateToolBudgetConfig(params.toolBudget, "toolBudget")`
                // (`async-execution.ts:1299` @v0.43.0). Validated here for the same reason the
                // foreground branch validates it: a malformed budget must refuse the call, not
                // silently downgrade to "unbudgeted".
                tool_budget: crate::exec::tool_budget::validate_tool_budget_config(
                    p.tool_budget.as_ref(),
                    "toolBudget",
                )
                .map_err(ToolError::new)?,
                cwd,
                agent_name: agent,
                task,
                context,
                model_override: model,
                agent_scope: resolve_execution_agent_scope(p.agent_scope.as_deref()),
                // SUBA-N04: the raw policy, already validated by `validate_execution_acceptance`
                // at the tool boundary; hop 2 lowers it per step.
                acceptance: p.acceptance.clone(),
                // SUBA-N05: the raw per-call `control` override. `spawn_background` folds it
                // against `subagents.control` and carries the RESOLVED value to hop 2 — pi
                // `executeAsyncSingle(id, { ..., controlConfig, ... })`,
                // `subagent-executor.ts:2845,2868-2870` @v0.34.0. Before this it was parsed,
                // absent from the foreground-only refusal list, and dropped on the floor.
                control: overrides.control.clone(),
                // SUBA-N03: the six params this branch used to refuse, taken from the SAME
                // `overrides` bundle the foreground path consumes — so the two paths cannot
                // disagree about what a given param meant (`skills` in particular is already
                // `normalize_skill_input`-normalized, so `skill: false` reaches hop 2 as the
                // explicit empty list rather than as "omitted").
                output: overrides.output.clone(),
                output_mode: overrides.output_mode.clone(),
                skills: overrides.skills.clone(),
                // SCOPE_19/A1: the caller's explicit reasoning level, from the SAME lowered
                // `overrides` bundle the foreground path consumes — so `{thinking, async:true}` is
                // not a silently weaker call than the same request without `async`.
                thinking: overrides.thinking.clone(),
                share: overrides.share,
                session_dir: overrides.session_dir.clone(),
                artifacts: overrides.artifacts,
                // SUBA-N03: already validated + de-aliased by `resolve_foreground_timeout`
                // (pi `resolveForegroundTimeout`, `:1327-1341`), whose positivity and
                // both-given-must-agree checks are mode-independent.
                timeout_ms,
                include_progress: overrides.include_progress,
            })
            .await
            .map_err(|e| ToolError::new(e.to_string()))?;
        // R-SA-074: return immediately after confirmed spawn; instruct against busy-polling.
        // pi `executeAsyncSingle` (`async-execution.ts:1515-1518`): the headline is `Async: {agent}
        // [{id}]`, followed by `formatAsyncStartedMessage`'s fixed guidance, and `details` is
        // `{ mode: "single", runId, results: [], asyncId, asyncDir }` (`async-execution.ts:1563`;
        // `asyncId` === `runId` for a SINGLE run, pi's own async-run identity convention).
        // `asyncDir` is what binds this run to its mission — see [`async_dir_for_run`].
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(format_async_started_message(
                &format!("Async: {agent} [{run_id}]"),
            ))],
            details: Some(async_launch_details(
                "single",
                &run_id,
                cwd,
                &self.executor.config_snapshot().await.roots,
                // [CYRUP-DELTA] `resolvedTools` — an async launch returns before any child settles,
                // so this reply is the parent's only synchronous chance to learn what the child it
                // just started can actually do. Threaded from `route_single`'s gate, not
                // re-resolved.
                tool_surface,
            )),
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }

    /// Turn a settled foreground [`SingleResult`] into the tool's [`ToolResult`] — out-of-band
    /// intercom delivery first, then the detached / interrupted / failed / clean ladder, IN THAT
    /// ORDER (each rung returns, so a later rung only ever sees a run the earlier ones declined).
    ///
    /// Single-run result surfacing (pi `subagent-executor.ts:2738-2761`). `final_output` is the
    /// finalized delivered output (`run_sync` already folded in the timeout preamble and any
    /// saved-output reference), i.e. pi's `finalizedOutput.displayOutput`.
    async fn render_single_result(
        &self,
        result: SingleResult,
        run_id: RunId,
        agent: &str,
        context: Option<ContextRequest>,
        include_progress: Option<bool>,
    ) -> Result<ToolResult, ToolError> {
        let display_output = result.final_output.clone().unwrap_or_default();
        // pi `runSinglePath`'s `details` (`subagent-executor.ts:3811-3823` @v0.43.0) is
        // `compactForegroundDetails({ mode: "single", runId, results: [r], progress, … })` — the
        // `SingleResult` is WRAPPED under `results`, and `mode`/`runId`/`context` sit beside it.
        // This used to be `serde_json::to_value(&result)`: the bare `SingleResult` at the details
        // ROOT, with no `mode`, no `runId` and no `results` array at all. That is a port bug at the
        // ported baseline, and it is what left `renderSubagentResult`'s only settled branch
        // (`tui/render.ts:1709`, keyed on `d.mode === "single" && d.results.length === 1`)
        // permanently unreachable — a `details` shape no renderer could read.
        //
        // `SubagentUpdatePayload` IS that shape (`{mode, context, progress, results, …}`, its own
        // doc already says it is attached "to every streamed `ToolUpdate` AND to its final result"),
        // and `single_final` is the constructor written for exactly this settle, so the live C19
        // stream and the terminal result now speak ONE wire shape instead of two.
        //
        // `progress` follows pi's own gate — `params.includeProgress ? allProgress : undefined`
        // (`:3008`) — so an unasked-for progress array is not smuggled into the model's context.
        let details = {
            // pi `resolveExplicitContextPolicy` (`subagent-executor.ts:1893-1900` @v0.43.0) stamps
            // the context from the CALL-SITE `params.context`, and only for `"fork"` — never from
            // the resolved per-persona default: `const context = params.context === "fork" ?
            // "fork" : "fresh";` (`:1894`). Mirror that exactly.
            let details_context = if context == Some(ContextRequest::Fork) {
                ContextMode::Fork
            } else {
                ContextMode::Fresh
            };
            let mut payload = crate::tui::events::SubagentUpdatePayload::single_final(
                details_context,
                result.clone(),
                crate::tui::events::LiveProgressSnapshot::from_settled_result(&result),
            );
            if include_progress != Some(true) {
                payload.progress.clear();
            }
            payload.run_id = Some(run_id.clone());
            Some(
                serde_json::to_value(&payload)
                    .unwrap_or_else(|_| serde_json::Value::String("subagent result".to_string())),
            )
        };

        if let Some(delivered) = self
            .deliver_single_result_out_of_band(&result, &run_id, agent)
            .await
        {
            return delivered;
        }

        // A detached (intercom) run is a coordination hand-off, not a failure (pi 2738-2743). No
        // live trigger sets `detached` in this crate today, but the branch is kept for fidelity.
        if result.detached {
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Detached for intercom coordination: {agent}. Reply to the supervisor request \
                     first. After the child exits, start a fresh follow-up if needed."
                ))],
                details,
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            });
        }

        // A soft interrupt is a paused SUCCESS, not an error (pi 2745-2750): exit 0, cleared error.
        if result.interrupted {
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Run paused after interrupt ({agent}). Waiting for explicit next action."
                ))],
                details,
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            });
        }

        // A FAILED run (non-zero exit) sets the error flag and surfaces the error text in the
        // model-facing content via `formatFailedSingleRunOutput` (pi 2752-2757) — cyrup's error
        // channel is `Err(ToolError)` (its `ToolResult` has no `isError` flag), which the runtime
        // renders as an `is_error` tool result carrying this text. The error is thus surfaced in
        // CONTENT, not buried in `details` JSON the model never sees.
        if result.exit_code != 0 {
            return Err(ToolError::new(format_failed_single_run_output(
                &result,
                &display_output,
            )));
        }

        // A clean run delivers its output (pi 2758-2761: `displayOutput || "(no output)"`).
        let text = if display_output.is_empty() {
            "(no output)".to_string()
        } else {
            display_output
        };
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(text)],
            details,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }

    /// The SINGLE path's out-of-band result-intercom hand-off, offered BEFORE
    /// [`Self::render_single_result`]'s own detached / interrupted / failed / clean ladder is
    /// consulted. `Some` is a confirmed delivery and IS the tool's result; `None` means the run
    /// was ineligible or the delivery was not confirmed, and that ladder runs as normal.
    ///
    /// R-SA-123/124/125 (pi `runSinglePath`, `subagent-executor.ts:3515-3873`): pi attempts
    /// out-of-band result-intercom delivery for a SINGLE run too, gated on `!detached &&
    /// !interrupted` (a detached/paused run has no terminal result to hand off yet) — this mirrors
    /// `route_parallel_mode`/`route_chain_mode`'s identical wiring. On a confirmed delivery, pi
    /// returns `formatSubagentResultReceipt`'s text for BOTH a clean run and a failed one (still
    /// surfacing failure — cyrup's analog is `Err(ToolError)` carrying that same receipt text,
    /// matching the "error surfaced in CONTENT" convention [`Self::render_single_result`]'s own
    /// non-zero-exit rung follows).
    async fn deliver_single_result_out_of_band(
        &self,
        result: &SingleResult,
        run_id: &RunId,
        agent: &str,
    ) -> Option<Result<ToolResult, ToolError>> {
        if result.detached || result.interrupted {
            return None;
        }
        // G104 — the SINGLE path resolves its one child through
        // `foregroundResultIntercomStatus` (`subagent-executor.ts:1594-1605`, applied per child
        // at `:1626`), i.e. the full `resolveSubagentResultStatus` ladder over the REAL
        // `SingleResult`. It deliberately does NOT go through the grouped
        // `StepResult`-projection constructor: a `StepResult` carries no `process_signal`, no
        // `detached`, no `timed_out` and no acceptance ledger, so projecting through it made the
        // unexplained-signal → `"stopped"` branch (`result-intercom.ts:35`) unreachable and
        // reported a rejected-but-exit-0 child as `"completed"`.
        let payload = crate::tui::intercom::IntercomPayload::from_single_result(
            run_id.clone(),
            agent.to_string(),
            result.exit_code == 0,
            result,
        );
        if let crate::tui::intercom::DeliveryOutcome::Delivered = self
            .executor
            .deliver_group_out_of_band(payload.clone())
            .await
        {
            let reduced = crate::tui::intercom::ReducedInlinePayload::from(&payload);
            let receipt = crate::tui::intercom::format_subagent_result_receipt(
                "single",
                run_id,
                &payload.child_statuses,
            );
            let reduced_details = Some(serde_json::json!({
                "mode": "single", "outOfBandDelivered": true, "reduced": reduced,
            }));
            return Some(if result.exit_code != 0 {
                Err(ToolError::new(receipt))
            } else {
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(receipt)],
                    details: reduced_details,
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            });
        }
        None
    }

    /// Management/control action dispatch (pi: a present `action` puts the tool in management mode).
    /// `doctor`/`models` (read-only) are wired to [`crate::extension::SubagentExecutor::run_doctor`]/`run_models_report`;
    /// the CRUD (`list`/`get`/`create`/`update`/`delete`, C3) routes to [`Self::route_management_action`]
    /// (the real [`crate::discovery::management`] handlers) and the background-control
    /// (`status`/`interrupt`/`resume`/`append-step`, C5) routes to [`Self::route_control_action`]
    /// (the real [`crate::background::control`]/[`crate::background::run_status`] primitives).
    pub(crate) async fn route_action(
        &self,
        action: &str,
        p: &SubagentToolParams,
        cwd: &Path,
        // VL-S13 — pi threads `signal` into `RefinementActionContext` (`subagent-executor.ts:6369`)
        // and on into the proposal-child launch (`:6370`). `Tool::execute` already holds the
        // host's own token for this call; passing a fresh `CancelToken::new()` here instead would
        // be a silent divergence — an interrupted turn would leave the proposal child running.
        cancel: &CancelToken,
    ) -> Result<ToolResult, ToolError> {
        match action {
            // pi `subagent-executor.ts:6467-6473` @v0.68.0, the arm immediately before `doctor`:
            // `listRetainedChildren(DIRS.async, resolveCurrentSessionId(ctx.sessionManager))`
            // rendered by `formatRetainedChildren`. A READ — absent from
            // `MUTATING_MANAGEMENT_ACTIONS` (`:213`), so it is its own arm rather than a member of
            // the control band below, for the reason the `inspect` arm states: it must not pass
            // through `route_control_action`'s authority consult. The session is
            // `current_session_id()` (pi's `resolveCurrentSessionId`); `None` means no filter,
            // exactly pi's falsy `sessionId`.
            "children.list" => {
                let cfg = self.executor.config_snapshot().await;
                let async_root =
                    crate::extension::executor::paths::default_async_root_in(&cfg.roots, cwd);
                let results_dir =
                    crate::extension::executor::paths::default_results_dir_in(&cfg.roots, cwd);
                let session = self.executor.current_session_id();
                let children = crate::background::list_retained_children(
                    &async_root,
                    &results_dir,
                    session.as_deref(),
                )
                .await
                .map_err(ToolError::new)?;
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(
                        crate::background::format_retained_children(&children),
                    )],
                    // pi `:6471` — `details: { mode: "management", results: [] }`, the shape
                    // every management reply here carries.
                    details: Some(serde_json::json!({ "mode": "management", "results": [] })),
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
            // Read-only diagnostics — already faithfully implemented (`run_doctor`), so wired here.
            // pi threads the call's own `sessionDir` override into the report (`buildDoctorReport`'s
            // `requestedSessionDir: paramsWithResolvedCwd.sessionDir`, `subagent-executor.ts:2828`).
            "doctor" => {
                let report = self
                    .executor
                    .run_doctor(cwd, p.session_dir.as_deref())
                    .await;
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(report)],
                    details: None,
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
            // SUBA-055 — pi `subagent-executor.ts:4979-4992` @v0.47.1. Upstream wraps the read in a
            // try/catch because its `readSubagentGuide` does filesystem I/O and throws on a missing
            // packaged file; cyrup's is `include_str!`-backed and cannot fail, so there is no error
            // arm to port — see `registration::guide`'s `[CYRUP-DELTA]`. The unknown-TOPIC branch is
            // NOT an error either, upstream or here: it returns the valid list as ordinary text so a
            // model recovers within the same turn.
            "guide" => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(
                    crate::registration::guide::read_subagent_guide(p.topic.as_deref()),
                )],
                details: None,
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            }),
            // `models` is the runtime builtin-agent -> model mapping (pi `handleModels`), the SAME
            // renderer the `/subagents-models` slash command uses — so the tool and slash surfaces
            // report one consistent mapping, exactly as pi routes both through `handleModels`.
            "models" => {
                let report = self.executor.run_models_report(cwd, p.agent.as_deref());
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(report)],
                    details: None,
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
            // SUBA-005: `eject`/`disable`/`enable`/`reset` join the CRUD arm — they are
            // `handle_management_action` cases exactly as `create`/`update`/`delete` are, and go
            // through the same child-safe denylist below.
            "list" | "get" | "create" | "update" | "delete" | "eject" | "disable" | "enable"
            | "reset" => self.route_management_action(action, p, cwd).await,
            // G90: `steer` joins the control arm (it is `route_control_action`'s fifth case, in
            // pi's own dispatch position between `resume` and `append-step`,
            // `subagent-executor.ts:3194` @v0.34.0).
            // G77: `stop` joins the control arm, in pi's own dispatch position immediately after
            // `interrupt` (`extension/rpc.ts:568`, `subagent-executor.ts:4776-4810` @v0.43.0).
            // SUBA-057: `dismiss` joins the control arm, in pi's own dispatch position immediately
            // after `stop` (`subagent-executor.ts:5872`, the `if (action === "dismiss")` block that
            // sits between the `append-step`/`schedule.*` blocks and the `stop` block).
            // `debug.run` shares `status`'s dispatch arm upstream (`if (action === "status" ||
            // action === "debug.run")`, `subagent-executor.ts:6515` @v0.68.0), so it joins the
            // control band here; it is absent from `MUTATING_MANAGEMENT_ACTIONS` (`:213`) and the
            // authority consult maps it to `None` (`registration/authority.rs`), so the band
            // gates nothing for it — exactly as upstream leaves it.
            "status" | "debug.run" | "interrupt" | "stop" | "dismiss" | "resume" | "steer"
            | "append-step" => self.route_control_action(action, p, cwd).await,
            // SCOPE_12 — [CYRUP-DELTA]: `inspect` is cyrup's own verb (see `text.rs`'s
            // `SUBAGENT_ACTIONS` entry for why upstream has none), and it is its OWN arm rather
            // than a member of the control band above. That placement is the point: inspect is a
            // READ, so it must NOT go through `route_control_action`, whose first act is the
            // authority consult (`:2044-2082`). `AuthorityAction::for_tool_action`
            // (`registration/authority.rs`) returns `None` for read verbs and must keep doing so —
            // routing a read through the consult would make reading a child's output a
            // confirm-gated operation.
            //
            // Params: the `status` arm's own `id`-first precedence (`:2090`, pi
            // `params.id ?? params.runId`), and `childId`/`lines` exactly as `stop` and the
            // transcript view already read them. No new schema property is needed.
            "inspect" => {
                let Some(target) = p.id.as_deref().or(p.run_id.as_deref()) else {
                    return Err(ToolError::new(
                        "action='inspect' requires id (or runId) naming the async run to inspect.",
                    ));
                };
                let reply = self
                    .executor
                    .control_inspect(cwd, target, p.child_id.as_deref(), p.lines)
                    .await;
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(
                        crate::background::inspect_rpc::respond::render_inspect_reply(&reply),
                    )],
                    // The full wire reply rides along, so a host that DOES render the widget shape
                    // reads the same object upstream's `encodeInspectReply` would have emitted.
                    details: serde_json::to_value(&reply).ok(),
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
            // SUBA-046 — pi `subagent-executor.ts:4457-4527` @v0.43.0, in upstream's own dispatch
            // position (after the management CRUD, before `children.list`/`doctor`).
            "grant-spawn-budget" => self.route_grant_spawn_budget(p).await,
            // The four `watchdog.*` actions (pi `WATCHDOG_TOOL_ACTIONS` /
            // `handleWatchdogToolAction`, dispatched at `subagent-executor.ts:4432`).
            watchdog_action
                if crate::watchdog::tool_actions::WATCHDOG_TOOL_ACTIONS
                    .contains(&watchdog_action) =>
            {
                self.route_watchdog_action(watchdog_action, p, cwd)
            }
            // The seven `mission.*` actions (pi `subagent-executor.ts:5723-5732` @v0.64.0;
            // `:4397-4407` @v0.43.0 before SUBA-085's `mission.resolve-decision`), in the dispatch
            // position upstream gives them: after the management/control arms, before the
            // authority-policy arm. `MUTATING_MANAGEMENT_ACTIONS` (`:197` @v0.64.0) lists five of
            // the seven — `mission.list`/`mission.show` are read-only — so a child-safe fanout
            // tool refuses exactly those five, with upstream's own child-safe text.
            mission_action
                if crate::missions::MissionAction::from_wire(mission_action).is_some() =>
            {
                let Some(mission_action) =
                    crate::missions::MissionAction::from_wire(mission_action)
                else {
                    // Unreachable: the guard above already resolved it.
                    return Err(ToolError::new(format!(
                        "unknown subagent action '{action}'"
                    )));
                };
                if !self.allow_mutating_management && mission_action.is_mutating() {
                    return Err(ToolError::new(format!(
                        "Action '{action}' is not available from child-safe subagent fanout mode."
                    )));
                }
                let outcome = crate::missions::handle_mission_action(
                    mission_action,
                    &p.mission_action_params(),
                    &crate::missions::MissionActionContext {
                        cwd: cwd.to_path_buf(),
                        current_session_id: self
                            .executor
                            .host_services()
                            .and_then(|s| s.session_id()),
                        config: self.executor.config_snapshot().await.missions.clone(),
                        agent_dir: None,
                    },
                )
                .map_err(|e| ToolError::new(e.to_string()))?;
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(outcome.text)],
                    details: Some(outcome.details),
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
            // LANES_2 — the FIVE convergence verbs, in the dispatch position upstream gives
            // them: `subagent-executor.ts:6213-6293` @v0.68.0 sits between the `mission.*` block
            // and the policy-action chain that reaches `schedule.create`.
            //
            // ONE guard arm for the family, through `LaneAction::from_wire` — the same shape the
            // `schedule.*` arm below uses. The three properties that differ between the verbs
            // (`is_mutating`, `authority_action`, `requires_handoff_path`) are decided by
            // exhaustive matches on the enum rather than by five independent string comparisons
            // that can drift; see `extension/tool/lane_actions.rs`.
            lane_action if lane_actions::LaneAction::from_wire(lane_action).is_some() => {
                use crate::extension::tool::lane_actions::{
                    LaneAction, LaneActionRefusal, is_lane_evidence_verb,
                    missing_handoff_path_refusal, resolve_against_request_cwd, trimmed_lane_id,
                };

                let Some(verb) = LaneAction::from_wire(lane_action) else {
                    // Unreachable: the guard above already resolved it.
                    return Err(ToolError::new(format!(
                        "unknown subagent action '{action}'"
                    )));
                };

                // THE ORDER IS UPSTREAM'S AND IS LOAD-BEARING. The child-safe gate is checked
                // FIRST (`:6214`, `:6243`, `:6271`) — above `mode`, above `planId`, above
                // `laneId`, above `handoffPath`. Reordering would leak the shape of the refusal,
                // and through it the fact that this repository holds preserved worktrees, to a
                // caller not permitted to invoke the verb at all.
                //
                // `lane.status` is deliberately exempt: `:6271` is
                // `action !== "lane.status" && allowMutatingManagementActions === false`, so a
                // delegated child CAN read its own lane graph. That exemption lives in exactly
                // one place, `LaneAction::is_mutating`, and is tested directly.
                if !self.allow_mutating_management && verb.is_mutating() {
                    return Err(ToolError::new(
                        LaneActionRefusal::ChildSafe(verb.as_str()).to_string(),
                    ));
                }

                let params = p.lane_action_params();
                // pi `:6274-6275` — `laneId`, for the three `lane.*` verbs only, and checked
                // BEFORE `handoffPath` (`:6276`). The order is upstream's: a `lane.recordMerge`
                // missing both parameters is told about `laneId`, not about `handoffPath`.
                let lane_id = trimmed_lane_id(params.lane_id.as_deref()).map(str::to_string);
                if is_lane_evidence_verb(verb) && lane_id.is_none() {
                    return Err(ToolError::new(
                        LaneActionRefusal::LaneRequiresLaneId(verb.as_str()).to_string(),
                    ));
                }
                // pi `:6226`, `:6260`, `:6278` — resolved against the REQUEST cwd, never the
                // process cwd. `worktree.cleanup` is the ONE verb that tolerates its absence
                // (`:6228` is a spread, so absent means "discover every manifest under repo"),
                // which is exactly what `requires_handoff_path()` says.
                let handoff_path = resolve_against_request_cwd(params.handoff_path.as_deref(), cwd);
                if handoff_path.is_none() && verb.requires_handoff_path() {
                    return Err(ToolError::new(
                        missing_handoff_path_refusal(verb).to_string(),
                    ));
                }

                match verb {
                    LaneAction::WorktreeCleanup => {
                        let params = p.cleanup_plan_params();
                        // The ONE verb `requires_handoff_path()` is false for, so `None` here
                        // reaches the plan builder as "discover every manifest under `repo`"
                        // rather than as a refusal (pi `:6228` is a spread).
                        let handoff_path = handoff_path.as_deref();
                        // pi `:6217-6219`, verbatim. This refusal IS the "a stale plan must never
                        // be executed" guard: upstream has no apply phase, no plan reader and no
                        // TTL enforcement anywhere at v0.68.0, so the only thing standing between
                        // a saved plan and an unchecked removal is that `apply` is refused at the
                        // boundary.
                        if params.mode.as_deref() != Some("plan") {
                            return Err(ToolError::new(
                                LaneActionRefusal::CleanupPlanOnly.to_string(),
                            ));
                        }
                        // pi `:6220-6222`, verbatim.
                        if params.plan_id.is_some() {
                            return Err(ToolError::new(
                                LaneActionRefusal::CleanupNoPlanId.to_string(),
                            ));
                        }
                        // pi `:6225-6228`: both paths resolve against the REQUEST cwd, never the
                        // process cwd.
                        let repo = resolve_against_request_cwd(params.repo.as_deref(), cwd)
                            .unwrap_or_else(|| cwd.to_path_buf());
                        let config = self.executor.config_snapshot().await;
                        let worktree_base_dir = config
                            .worktree_base_dir
                            .as_deref()
                            .map(|path| path.to_string_lossy().into_owned());
                        // pi `:6230-6235`. Required, not optional — see
                        // `ForegroundOwnershipProbe`.
                        let executor = std::sync::Arc::clone(&self.executor);
                        let ownership =
                            move |run_id: &str| executor.foreground_run_ownership(run_id);

                        let created = crate::spawn::cleanup_plan::create_worktree_cleanup_plan(
                            crate::spawn::cleanup_plan::BuildCleanupPlanInput {
                                repo: &repo,
                                handoff_path,
                                handoff_paths: &[],
                                worktree_base_dir: worktree_base_dir.as_deref(),
                                now: crate::time::now_epoch_millis(),
                                plan_id: None,
                                foreground_ownership: &ownership,
                            },
                        )
                        .await
                        // pi `:6238-6240` renders `error.message` with `isError: true`; cyrup's
                        // error channel is `Err(ToolError)`, which the runtime renders the same
                        // way. The text is `CleanupPlanError`'s own `Display`, unprefixed.
                        .map_err(|error| ToolError::new(error.to_string()))?;

                        Ok(ToolResult {
                            content: vec![cyrup_core::Content::text(
                                crate::spawn::cleanup_plan::format_worktree_cleanup_plan(&created),
                            )],
                            // pi `:6237` — the shape every management arm returns. `results` is a
                            // present empty array, not an omitted key.
                            details: Some(
                                serde_json::json!({ "mode": "management", "results": [] }),
                            ),
                            terminate: TerminateHint::Unspecified,
                            ..Default::default()
                        })
                    }
                    LaneAction::WorktreeDiscard => {
                        // `handoff_path` is `Some` for this verb (the guard above).
                        let manifest_path = handoff_path.unwrap_or_default();
                        self.route_worktree_discard(verb, &manifest_path).await
                    }
                    LaneAction::LaneStatus
                    | LaneAction::LaneRecordMerge
                    | LaneAction::LaneRecordSupersession => {
                        // Both `Some` for these verbs (the two guards above).
                        let lane_id = lane_id.unwrap_or_default();
                        let manifest_path = handoff_path.unwrap_or_default();
                        self.route_lane_evidence(verb, &lane_id, &manifest_path, params)
                            .await
                    }
                }
            }
            // VL-S13 — the three `refine*` verbs. ONE guard arm through
            // `RefinementAction::from_wire`, the same shape the `lane.*` and `schedule.*` arms use.
            //
            // Upstream's own dispatch block for them is `subagent-executor.ts:6358`, which sits
            // after the `lane.*` block (`:6213-6293`) and before `grant-spawn-budget` (`:6381`).
            // cyrup does NOT reproduce that relative layout and does not need to: this file's
            // `"grant-spawn-budget"` arm is at `:1189`, roughly two hundred lines ABOVE this one.
            // The arms of a `match` on `action` are disjoint string patterns, so their order is
            // inert here in a way upstream's chain of `if (action === …)` returns is not.
            //
            // Authority is deliberately NOT consulted, and that is a finding rather than an
            // omission: upstream's whole dispatch block's only gate is the child-safe one at
            // `:6359`. cyrup's `AUTHORITY_ACTIONS` (`registration/authority.rs:47-54`) is a closed
            // SIX-entry list and upstream's (`policy/authority.ts:1-10` @v0.68.0) a closed EIGHT
            // — cyrup omits `inspectorOpen`/`projectOpen`, whose verbs it has not ported — and
            // NEITHER list has a member for any `refine*` verb.
            // `AuthorityAction::for_tool_action` therefore keeps returning `None`
            // for all three, and the read/mutate split is carried entirely by
            // `RefinementAction::is_mutating` at the child-safe gate below — the same inline shape
            // the `mission.*`, `lane.*` and `schedule.*` arms use, rather than growing
            // `discovery::management::MUTATING_MANAGEMENT_ACTIONS` (a 7-entry set scoped to
            // `route_management_action`'s CRUD, which these verbs do not route through).
            refinement_action
                if crate::exec::agent_refinements::action::RefinementAction::from_wire(
                    refinement_action,
                )
                .is_some() =>
            {
                let Some(verb) =
                    crate::exec::agent_refinements::action::RefinementAction::from_wire(
                        refinement_action,
                    )
                else {
                    // Unreachable: the guard above already resolved it.
                    return Err(ToolError::new(format!(
                        "unknown subagent action '{action}'"
                    )));
                };
                // pi `:6359`, FIRST — above agent resolution and above the file read. The order is
                // upstream's: a child-safe caller is told the verb is unavailable, not told which
                // agents exist. `refine.show` is exempt because upstream's
                // `MUTATING_MANAGEMENT_ACTIONS` does not carry it.
                if !self.allow_mutating_management && verb.is_mutating() {
                    return Err(ToolError::new(format!(
                        "Action '{action}' is not available from child-safe subagent fanout mode."
                    )));
                }
                self.route_refinement_action(verb, p, cwd, cancel).await
            }
            // SUBA-016 — the nine `schedule.*` actions, in the dispatch position upstream gives
            // them: after the `mission.*` arm, before `validate`. `subagent-executor.ts` dispatches
            // `schedule.*` between `append-step` and `dismiss`, which is the neighbourhood
            // `route_control_action`'s own doc already names.
            //
            // `schedule.delete` is in `DESTRUCTIVE_MANAGEMENT_ACTIONS` (`text.rs:299-312`), which
            // has carried it since SUBA-065 — so the stricter did-you-mean rule applies to it
            // automatically from the first call.
            schedule_action
                if crate::background::scheduled_runs::ScheduledRunAction::from_wire(
                    schedule_action,
                )
                .is_some() =>
            {
                let Some(schedule_action) =
                    crate::background::scheduled_runs::ScheduledRunAction::from_wire(
                        schedule_action,
                    )
                else {
                    // Unreachable: the guard above already resolved it.
                    return Err(ToolError::new(format!(
                        "unknown subagent action '{action}'"
                    )));
                };
                // The six mutating verbs are refused from a child-safe fanout tool, with the same
                // sentence the mutating `mission.*` verbs already get: a fanout child may read
                // what the project has scheduled and may not schedule, resume or fire anything.
                if !self.allow_mutating_management && schedule_action.is_mutating() {
                    return Err(ToolError::new(format!(
                        "Action '{action}' is not available from child-safe subagent fanout mode."
                    )));
                }
                // SUBA-064 — `registration/authority.rs:64-74` has mapped `schedule.create` to
                // `AuthorityAction::ScheduleCreate` since before this verb dispatched, with a note
                // saying exactly that. This is the dispatch that finally consults it, through the
                // SAME three-arm gate `route_control_action` applies to `stop`/`steer` — one
                // policy surface, not a second one that could drift. `for_tool_action` returns
                // `None` for the other eight verbs, so only `schedule.create` is gated, which is
                // upstream's own mapping.
                if let Some(policy_action) =
                    crate::registration::authority::AuthorityAction::for_tool_action(action)
                {
                    use crate::registration::authority as auth;
                    let policy = self.executor.config_snapshot().await.authority_policy;
                    match auth::resolve_authority_decision(policy_action, policy.as_ref()) {
                        auth::AuthorityDecision::Auto => {}
                        auth::AuthorityDecision::Forbid => {
                            return Err(ToolError::new(auth::forbidden_message(action)));
                        }
                        auth::AuthorityDecision::Confirm => {
                            let Some(services) = self.executor.host_services() else {
                                return Err(ToolError::new(auth::no_ui_message(action)));
                            };
                            if !services.confirm(
                                &auth::confirm_prompt(action),
                                &auth::confirm_message(action),
                                &cyrup_ext::host::DialogOptions::default(),
                            ) {
                                // A user declining is a CHOICE, not a failure — `Ok`, not `Err`,
                                // exactly as the control arm's own decline branch is.
                                return Ok(ToolResult {
                                    content: vec![cyrup_core::Content::text(
                                        auth::declined_message(action),
                                    )],
                                    details: Some(serde_json::json!({ "mode": "management" })),
                                    terminate: TerminateHint::Unspecified,
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
                let Some(manager) = self.executor.scheduled_runs() else {
                    // pi `:544` — the gate answers BEFORE the switch. cyrup reaches the same
                    // sentence from the absent manager, because `install_scheduled_runs` is what
                    // reads the config and it declines to install when the key says `false`.
                    return Err(ToolError::new(
                        crate::background::scheduled_runs::SCHEDULED_RUNS_DISABLED,
                    ));
                };
                // pi `handleToolCall`'s outer `try/catch` (`:558-560`) turns EVERY thrown error
                // into `textResult(message, …, isError = true)`. cyrup's `ToolResult` carries no
                // `isError` flag — its error channel is `Err(ToolError)`, which the runtime renders
                // as an `is_error` tool result carrying the text — so upstream's error-flagged
                // result maps onto `Err` here. That is the SAME mapping
                // `route_management_action` already performs for `outcome.is_error`, and the
                // details survive it: `ToolError::with_details` carries the `schedules` payload
                // through, so a refusal is not stripped of its evidence.
                //
                // The refusals really are refusals, not failures: a `schedule.create` that names a
                // calendar trigger, a store that already holds twenty pending schedules, or a
                // `schedule.delete` on a live run all reach the model as text it can act on within
                // the same turn.
                match manager
                    .handle_action(schedule_action, &p.schedule_action_params())
                    .await
                {
                    Ok(outcome) if !outcome.is_error => Ok(ToolResult {
                        content: vec![cyrup_core::Content::text(outcome.text)],
                        details: Some(outcome.details),
                        terminate: TerminateHint::Unspecified,
                        ..Default::default()
                    }),
                    Ok(outcome) => Err(ToolError::new(outcome.text).with_details(outcome.details)),
                    Err(error) => Err(ToolError::new(error.to_string())),
                }
            }
            // WORKFLOW_2 / pi `action: "validate"` with `workflowScript`
            // (`extension/schemas.ts:288`). Synchronous, because validation authority is already
            // split — V8 owns syntax via `compile_check`'s throwaway isolate, the Rust analyzer
            // owns structure — so there is no run, no child and no budget to spend.
            //
            // `route_action` is dispatched at `tool/mod.rs:222-224`, ABOVE the mode gate, the
            // depth guard and the session spawn reserve, so "validation costs no child" is
            // guaranteed by CONTROL FLOW, not merely by this function being sync. Note also that
            // this signature carries no `on_update` and no `cancel`: the arm cannot stream or
            // spawn even by accident.
            //
            // `state_enabled` is `false` HERE, and deliberately so even though WORKFLOW_20 made
            // the WORKFLOW arm's flag conditional. The two surfaces differ because this one is
            // required to be side-effect-free: deriving a real flag would mean resolving the
            // mission, and `prepare_mission_launch` marks an attached mission Active and CREATES
            // one for a `mission: {...}` object — a validation call must do neither. So a preflight
            // `action: "validate"` reports the STRICTEST verdict: a script it accepts runs
            // anywhere, while one using `state` is reported unavailable here and still runs when
            // dispatched with a `missionId`. Widening this is a separate decision (WORKFLOW_20 §4),
            // not a drive-by.
            //
            // A script that merely FAILS validation is `Ok(report)` with `ok: false` — the
            // function's own doc says so — so the findings are rendered as ordinary text, exactly
            // as the `guide` arm renders an unknown topic. Only an engine/instantiation failure is
            // a `ToolError`.
            "validate" => {
                let Some(script) = p.workflow_script.as_deref() else {
                    return Err(ToolError::new("action='validate' requires workflowScript."));
                };
                let report = crate::workflows::scripted::validate_workflow_script(script, false)
                    .map_err(ToolError::new)?;
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(render_validation_findings(
                        &report,
                    ))],
                    details: Some(serde_json::json!({
                        "action": "validate",
                        "ok": report.ok,
                        "findings": report.errors.len(),
                    })),
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
            // SUBA-038/SUBA-065 / pi `unknownSubagentActionMessage`
            // (`subagent-executor.ts:195-208` @v0.47.1, reached from `:4861`'s successor). This was
            // cyrup's own `unknown subagent action '{other}'; valid actions are …` with a
            // HAND-WRITTEN list that omitted the four `watchdog.*` verbs that do dispatch — so a
            // model recovering from the error was steered away from verbs that exist.
            other => Err(ToolError::new(unknown_subagent_action_message(other))),
        }
    }

    /// SUBA-046 — `action: "grant-spawn-budget"`: add launches to an exhausted per-session spawn
    /// cap, behind an explicit user confirmation (pi `subagent-executor.ts:4457-4527` @v0.43.0).
    ///
    /// THE USER ACTION: a session hits `maxSubagentSpawnsPerSession` mid-task. Before this, that
    /// was terminal — cyrup had no grant path, so the only escape was restarting the session and
    /// losing the conversation. Upstream's design is that the cap is a speed bump with a confirmed
    /// grant behind it, which is why its own refusal text says "Grant budget explicitly from the
    /// root interactive session"; cyrup printed that sentence while implementing no such thing,
    /// and additionally ADVERTISED the verb in the child-safe tool description.
    ///
    /// Every refusal below is upstream's verbatim text, in upstream's order — the order is
    /// load-bearing, because each gate assumes the previous one held (the session-id check runs
    /// before the active-children check, which runs before the amount is validated, which runs
    /// before the authority policy is consulted, which runs before the user is asked).
    ///
    /// The authority consult is deliberately NOT
    /// [`crate::registration::authority::AuthorityAction::for_tool_action`]'s generic
    /// control-arm gate: upstream maps only `stop`/`steer`/`schedule.create` there and gives the
    /// grant path its OWN `resolveAuthorityDecision({action:"spawnBudgetGrant"})` with its own
    /// messages and its own confirmation body (`:4493-4500`), which is what is reproduced here.
    ///
    /// Where upstream attaches `details.spawnBudget` to a refusal, cyrup surfaces the same numbers
    /// in the message text instead: a cyrup [`ToolError`] carries a message and nothing else
    /// (`cyrup-core/src/tool.rs:78-80`), and upstream itself prefixes result text with
    /// `formatSpawnBudget(...)` on the same surface (`withSpawnBudget`, `:425-430`). The SUCCESS
    /// result carries the snapshot in `details.spawnBudget`, where the type does allow it.
    async fn route_grant_spawn_budget(
        &self,
        p: &SubagentToolParams,
    ) -> Result<ToolResult, ToolError> {
        use crate::exec::spawn_budget as budget_ops;
        use crate::registration::authority as auth;

        // pi `if (deps.allowMutatingManagementActions === false || !ctx.hasUI)` (`:4458`). cyrup's
        // `hasUI` is "a live host-services surface is bound", the same equivalence the SUBA-064
        // authority gate already draws.
        let Some(services) = self
            .executor
            .host_services()
            .filter(|_| self.allow_mutating_management)
        else {
            return Err(ToolError::new(
                "Action 'grant-spawn-budget' is available only from the root interactive parent \
                 session.",
            ));
        };
        // pi `deps.state.currentSessionId = resolveCurrentSessionId(...); if (!...)` (`:4465-4471`).
        let Some(session_id) = self.executor.current_session_id() else {
            return Err(ToolError::new(
                "Action 'grant-spawn-budget' requires an active parent session id.",
            ));
        };
        let cfg = self.executor.config_snapshot().await;
        let max_spawns = cfg.max_subagent_spawns_per_session;

        // pi `if (hasActiveSubagentChildren(deps.state))` (`:4472-4479`) — the preview the user
        // confirms must be measured against a `used` count that is not still moving.
        if self.executor.has_active_subagent_children() {
            let snapshot = self.executor.spawn_budget_snapshot(max_spawns);
            return Err(ToolError::new(format!(
                "Spawn budget grants are rejected while current-session children are queued or \
                 running. Wait for them to settle, then retry the explicit grant. {}",
                budget_ops::format_spawn_budget(&snapshot)
            )));
        }

        // pi `const additional = paramsWithResolvedCwd.additional ?? Number.NaN;` (`:4482`) — an
        // OMITTED `additional` is not a distinct case upstream: it flows into the same validator
        // and comes back with the "requires additional to be a positive integer" message.
        let requested = p.additional.unwrap_or(0);
        let preview = self.executor.spawn_budget_snapshot(max_spawns);
        let additional =
            budget_ops::preflight_spawn_budget_grant(&preview, requested).map_err(|error| {
                ToolError::new(format!(
                    "{error} {}",
                    budget_ops::format_spawn_budget(&preview)
                ))
            })?;

        // pi `resolveAuthorityDecision({ action: "spawnBudgetGrant", … })` (`:4491`), whose DEFAULT
        // for this action is `confirm` (`policy/authority.ts:14-21`).
        let decision = auth::resolve_authority_decision(
            auth::AuthorityAction::SpawnBudgetGrant,
            cfg.authority_policy.as_ref(),
        );
        if decision == auth::AuthorityDecision::Forbid {
            return Err(ToolError::new(format!(
                "Authority policy forbids spawn budget grants. {}",
                budget_ops::format_spawn_budget(&preview)
            )));
        }
        // pi `authority === "auto" || await ctx.ui.confirm(title, body)` (`:4497-4500`), with
        // upstream's title and body verbatim.
        let confirmed = decision == auth::AuthorityDecision::Auto
            || services.confirm(
                "Grant subagent spawn budget?",
                &format!(
                    "Add {additional} launches to this logical session?\n\n{}\n\nUsage is not \
                     reset. Compaction keeps the same budget; a new parent session starts a fresh \
                     one.",
                    budget_ops::format_spawn_budget(&preview)
                ),
                &cyrup_ext::host::DialogOptions::default(),
            );
        if !confirmed {
            // pi returns the cancel text WITHOUT `isError: true` (`:4502-4506`) — declining is a
            // choice, not a failure.
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(
                    "Spawn budget grant canceled; no capacity was added.",
                )],
                details: Some(serde_json::json!({
                    "mode": "management",
                    "results": [],
                    "spawnBudget": preview,
                })),
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            });
        }

        // pi's post-confirmation re-check (`:4508-4520`): the dialog was open for an unbounded
        // amount of wall clock, so the session, the usage and the active-child state are all
        // re-read and the grant is abandoned if ANY of them moved. This is the JS→Rust hazard in
        // reverse — upstream needs the check because `await ctx.ui.confirm` yields, and so does
        // this `.await`.
        let current = self.executor.spawn_budget_snapshot(max_spawns);
        if self.executor.current_session_id().as_deref() != Some(session_id.as_str())
            || self.executor.has_active_subagent_children()
            || current.used != preview.used
            || current.granted != preview.granted
        {
            return Err(ToolError::new(format!(
                "Spawn budget grant was not applied because the session, budget, or active-child \
                 state changed while confirmation was open. {}",
                budget_ops::format_spawn_budget(&current)
            )));
        }

        let granted = self
            .executor
            .grant_subagent_spawn_budget(i64::from(additional), max_spawns)
            .map_err(ToolError::new)?;
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(format!(
                "Spawn budget grant applied: +{additional}. {}",
                budget_ops::format_spawn_budget(&granted)
            ))],
            details: Some(serde_json::json!({
                "mode": "management",
                "results": [],
                "spawnBudget": granted,
            })),
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }

    /// `handleWatchdogToolAction(action, paramsWithResolvedCwd, ctx, deps.watchdog)`
    /// (`subagent-executor.ts:4432`) — the four `watchdog.*` actions.
    ///
    /// Upstream returns an `AgentToolResult` whose `isError` flag distinguishes a failed action;
    /// cyrup surfaces a failed tool as `Err` (R-02-024), so that flag becomes a [`ToolError`] here,
    /// exactly as [`Self::route_management_action`] already maps pi's `isError: true`.
    pub(crate) fn route_watchdog_action(
        &self,
        action: &str,
        p: &SubagentToolParams,
        cwd: &Path,
    ) -> Result<ToolResult, ToolError> {
        // `deps.allowMutatingManagementActions === false && MUTATING_MANAGEMENT_ACTIONS.has(action)`
        // (`subagent-executor.ts:4425-4431`): of the four watchdog verbs, only `watchdog.configure`
        // is in that set (`:151`) — the other three are read-only reports a fanout child may run.
        // The refusal text is upstream's own, verbatim.
        if !self.allow_mutating_management && action == WATCHDOG_MUTATING_ACTION {
            return Err(ToolError::new(format!(
                "Action '{action}' is not available from child-safe subagent fanout mode."
            )));
        }
        let services = self.executor.host_services();
        let registry = crate::watchdog::model_selection::BuiltinWatchdogModelRegistry::new(
            watchdog_config_dirs().as_ref(),
        );
        let ctx = crate::watchdog::register_main::WatchdogCommandContext {
            cwd: cwd.to_path_buf(),
            registry: &registry,
            current_model: services
                .as_ref()
                .and_then(|s| s.current_model())
                .as_deref()
                .and_then(watchdog_model_info),
            thinking_level: services.as_ref().and_then(|s| s.thinking_level()),
        };
        let params = crate::watchdog::tool_actions::WatchdogToolParams {
            scope: p.scope.clone(),
            target: p.target.clone(),
            agent: p.agent.clone(),
            model: p.model.clone(),
            thinking: p.thinking.clone(),
        };
        let result = crate::watchdog::tool_actions::handle_watchdog_tool_action(
            action,
            &params,
            &ctx,
            self.watchdog.as_deref(),
        );
        if result.is_error {
            return Err(ToolError::new(result.text));
        }
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(result.text)],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }

    /// Management-action dispatch (Tier 1, C3): route `list`/`get`/`models`/`create`/`update`/
    /// `delete` to the now-wired [`crate::discovery::management`] CRUD + `agent-management.ts`
    /// renderers via [`crate::discovery::management::handle_management_action`]. Discovery is scoped
    /// to this tool's captured `cwd` and re-run per call inside each handler (R-SA-019). A pi
    /// `isError: true` outcome (not-found, read-only, validation) maps to a [`ToolError`] carrying
    /// pi's exact text (cyrup surfaces tool failures as `Err`, R-02-024); a genuine discovery/IO
    /// failure propagates as a [`ToolError`] too.
    async fn route_management_action(
        &self,
        action: &str,
        p: &SubagentToolParams,
        cwd: &Path,
    ) -> Result<ToolResult, ToolError> {
        // T6 child-safe restriction (pi `fanout-child.ts` `allowMutatingManagementActions: false`,
        // over `MUTATING_MANAGEMENT_ACTIONS`, `subagent-executor.ts:151`): a fanout child may
        // inspect/delegate but must not rewrite the parent's agent config on disk — which since
        // SUBA-005 also means it must not eject a builtin into the parent's user scope, nor
        // disable/enable/reset an agent via the parent's `settings.json`.
        if !self.allow_mutating_management
            && crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS.contains(&action)
        {
            // SUBA-038 / pi `Action '${action}' is not available from child-safe subagent fanout
            // mode.` (`subagent-executor.ts:4867` @v0.43.0, and the identical text at `:4380`
            // /`:4385` for the herdr arms). This was cyrup's own "subagent management action '…' is
            // blocked in child-safe fanout mode; <list> are not permitted here." — a different
            // sentence AND a different amount of information than pi gives.
            return Err(ToolError::new(format!(
                "Action '{action}' is not available from child-safe subagent fanout mode."
            )));
        }
        let cfg = self
            .executor
            .discovery_config(cwd, &self.executor.config_snapshot().await.roots)
            .map_err(|e| ToolError::new(e.to_string()))?;
        // The live parent session model (pi `ctx.model`), so a `models` action routed through the
        // management layer renders the real inherited model rather than `(unavailable)`. Bound to a
        // local so the borrowed `&str` in `ManagementRequest` outlives the call.
        let current_session_model = self
            .executor
            .inherited_session_model()
            .map(|m| m.as_str().to_string());

        // pi `handleList` reads `ctx.config?.proactiveSkillSubagents` and passes a LAZY
        // `discoverAvailableSkills: () => discoverAvailableSkills(ctx.cwd)` closure
        // (`agent-management.ts:765-770` @v0.43.0). cyrup's skill scan is `async`, so the
        // laziness lives here rather than inside the handler: the config is
        // resolved first and the scan is awaited ONLY when the feature is enabled, which is the
        // observable behaviour upstream's closure gives (a disabled feature touches no filesystem).
        // Every other action ignores the field, so the scan is also skipped for them.
        let proactive_setting = if action == "list" {
            self.executor
                .config_snapshot()
                .await
                .proactive_skill_subagents
                .as_ref()
                .map(
                    crate::discovery::skills::ProactiveSkillSubagentsSetting::from_extension_config,
                )
        } else {
            None
        };
        let available_skills: Vec<crate::discovery::skills::AvailableSkill> = if action == "list"
            && crate::discovery::skills::resolve_proactive_skill_subagents_config(
                proactive_setting.as_ref(),
            )
            .enabled
        {
            crate::discovery::skills::discover_available_skills(cwd).await
        } else {
            Vec::new()
        };

        let req = crate::discovery::management::ManagementRequest {
            agent: p.agent.as_deref(),
            chain_name: p.chain_name.as_deref(),
            agent_scope: p.agent_scope.as_deref(),
            config: p.config.as_ref(),
            current_session_model: current_session_model.as_deref(),
            proactive_skills: (action == "list").then(|| {
                crate::discovery::management::ProactiveSkillsInput {
                    setting: proactive_setting.as_ref(),
                    available_skills: &available_skills,
                }
            }),
        };
        match crate::discovery::management::handle_management_action(&cfg, action, &req).await {
            Ok(outcome) if !outcome.is_error => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(outcome.text)],
                details: Some(serde_json::json!({ "mode": "management", "results": [] })),
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            }),
            Ok(outcome) => Err(ToolError::new(outcome.text)),
            Err(e) => Err(ToolError::new(e.to_string())),
        }
    }

    /// LANES_2 — `worktree.discard` (pi `subagent-executor.ts:6242-6268` @v0.68.0).
    ///
    /// The only verb in this family that consults the authority policy, and the only one that
    /// removes anything from disk. Four things here are load-bearing and easy to lose:
    ///
    /// 1. **The authority consult is not optional and its absence is not `auto`.**
    ///    [`crate::registration::authority::resolve_authority_decision`] falls back to
    ///    `default_decision`, which is `Confirm` for `discardWorktree`, so a default install
    ///    prompts. A `Confirm` with no UI is a REFUSAL (`:6255`), never an implicit yes.
    /// 2. **A decline is `Ok`, not `Err`** (`:6258` carries no `isError`). A deliberate "no" that
    ///    reads as a failure to an orchestrator retry loop gets the user re-prompted.
    /// 3. **How it was authorized is DATA.** pi passes `kind: "confirmed" | "policy"` down
    ///    (`:6263`), and `cleanup_worktrees` consults it again for any worktree that still holds
    ///    uncommitted work (`worktree.ts:1232-1234`) — under a `confirm` policy, only an
    ///    authorization that actually came from a human clears such a worktree.
    /// 4. **It runs inside the worktree-mutation turn** (`:6261`'s `withWorktreeTransaction`),
    ///    so a discard cannot interleave with a `git worktree add` in flight.
    async fn route_worktree_discard(
        &self,
        verb: crate::extension::tool::lane_actions::LaneAction,
        manifest_path: &Path,
    ) -> Result<ToolResult, ToolError> {
        use crate::extension::tool::lane_actions::{
            DISCARD_CONFIRM_PROMPT, DISCARD_DECLINED, LaneActionRefusal, discard_confirm_message,
        };
        use crate::registration::authority as auth;

        let Some(policy_action) = verb.authority_action() else {
            // Unreachable: `authority_action()` is `Some` for exactly this variant, and the test
            // `only_worktree_discard_consults_the_authority_policy` pins that.
            return Err(ToolError::new(
                LaneActionRefusal::DiscardForbidden.to_string(),
            ));
        };
        let policy = self.executor.config_snapshot().await.authority_policy;
        let decision = auth::resolve_authority_decision(policy_action, policy.as_ref());
        let confirmed = match decision {
            auth::AuthorityDecision::Auto => true,
            // pi `:6250-6252`. Verb-specific prose, NOT `auth::forbidden_message`'s generic
            // `Authority policy forbids action '<x>'.`
            auth::AuthorityDecision::Forbid => {
                return Err(ToolError::new(
                    LaneActionRefusal::DiscardForbidden.to_string(),
                ));
            }
            auth::AuthorityDecision::Confirm => {
                // pi `:6254-6256`.
                //
                // [CYRUP-DELTA] the confirm BODY names the RESOLVED manifest path, where pi
                // interpolates the raw `handoffPath` the caller sent (`:6256` uses
                // `paramsWithResolvedCwd.handoffPath`, while the path it then acts on is
                // absolutized at `:6260`). A relative path in a destructive prompt is ambiguous —
                // the person answering cannot tell which repository's worktrees are about to go —
                // and showing something other than what will be touched is the wrong half of the
                // pair to show. The prompt TITLE and every other sentence stay verbatim.
                let Some(services) = self.executor.host_services() else {
                    return Err(ToolError::new(LaneActionRefusal::DiscardNoUi.to_string()));
                };
                services.confirm(
                    DISCARD_CONFIRM_PROMPT,
                    &discard_confirm_message(manifest_path),
                    &cyrup_ext::host::DialogOptions::default(),
                )
            }
        };
        if !confirmed {
            // pi `:6258` — `Ok`, not `Err`.
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(DISCARD_DECLINED)],
                details: Some(serde_json::json!({ "mode": "management", "results": [] })),
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            });
        }

        // pi `:6261` — `withWorktreeTransaction`. Held across the whole read-modify-write, not
        // just the `git` calls: two discards that each read the manifest, remove, and write it
        // back would otherwise lose one of the two removal ledgers.
        let _turn = crate::spawn::worktree::worktree_turn().await;
        let discarded = crate::handoff::discard_preserved(
            manifest_path,
            crate::handoff::DiscardAuthorization {
                // pi `:6263` — `decision === "confirm" ? "confirmed" : "policy"`.
                kind: if decision == auth::AuthorityDecision::Confirm {
                    crate::handoff::DiscardAuthorizationKind::Confirmed
                } else {
                    crate::handoff::DiscardAuthorizationKind::Policy
                },
                policy,
            },
        )
        .await
        // pi `:6266-6268` renders `error.message` with `isError: true`.
        .map_err(|error| ToolError::new(error.to_string()))?;

        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(discarded.text)],
            details: Some(serde_json::json!({ "mode": "management", "results": [] })),
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }

    /// LANES_2 — `lane.status` / `lane.recordMerge` / `lane.recordSupersession` (pi
    /// `subagent-executor.ts:6280-6292` @v0.68.0), after the shared `laneId`/`handoffPath`
    /// validation the caller has already applied.
    ///
    /// Two asymmetries are upstream's and are deliberate:
    ///
    /// * **`lane.status` swallows a read error** (`:6282`'s bare `catch { manifest = undefined; }`),
    ///   so a missing or corrupt manifest renders as "removal is not safe" prose rather than an
    ///   error. That swallow is scoped to this verb ALONE — the run-id mismatch at `:6283` and
    ///   every `lane.record*` failure are errors. It is the one place in this feature where
    ///   discarding an error is correct.
    /// * **The two recorders emit `details.parallelHandoff`** (`:6289`), which is not decoration:
    ///   `missions/lifecycle.rs`'s `artifacts_for_result` and `sync_mission_from_async_completion`
    ///   both read `parallelHandoff.path` and file a `MissionArtifactKind::Manifest`. Dropping
    ///   the key as "unused" would re-deaden two landed production readers.
    async fn route_lane_evidence(
        &self,
        verb: crate::extension::tool::lane_actions::LaneAction,
        lane_id: &str,
        manifest_path: &Path,
        params: crate::extension::tool::lane_actions::LaneActionParams,
    ) -> Result<ToolResult, ToolError> {
        use crate::extension::tool::lane_actions::{LaneAction, parse_evidence};

        let management_details = || serde_json::json!({ "mode": "management", "results": [] });

        if verb == LaneAction::LaneStatus {
            // pi `:6282`. See this function's doc for why the error is dropped HERE and nowhere
            // else.
            let manifest = crate::handoff::read_manifest(manifest_path)
                .await
                .unwrap_or(None);
            // pi `:6283` — thrown, so an error.
            if let Some(stored) = manifest.as_ref()
                && stored.run_id.as_str() != lane_id
            {
                return Err(ToolError::new(
                    crate::handoff::HandoffError::LaneRunMismatch {
                        lane: lane_id.to_string(),
                        run: stored.run_id.as_str().to_string(),
                    }
                    .to_string(),
                ));
            }
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(
                    crate::handoff::format_stored_cleanup(manifest_path, manifest.as_ref()),
                )],
                details: Some(management_details()),
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            });
        }

        let lane = crate::handoff::LaneId::parse(lane_id)
            .map_err(|error| ToolError::new(error.to_string()))?;
        let now = crate::time::now_epoch_millis();
        // pi `:6286-6288`. The attestation object reaches the recorder byte-identical to what the
        // caller sent — its `manifestDigest` is overwritten there (`:442`/`:461`) and every other
        // field is evidence, so normalizing on the way in would change what it attests to.
        let recorded = match verb {
            LaneAction::LaneRecordMerge => {
                let merge = parse_evidence::<crate::handoff::MergeEvidence>("merge", params.merge)
                    .map_err(|error| ToolError::new(error.to_string()))?;
                crate::handoff::record_merge(crate::handoff::RecordMerge {
                    manifest_path,
                    lane_id: &lane,
                    merge,
                    now,
                })
                .await
            }
            _ => {
                let supersession = parse_evidence::<crate::handoff::SupersessionEvidence>(
                    "supersession",
                    params.supersession,
                )
                .map_err(|error| ToolError::new(error.to_string()))?;
                crate::handoff::record_supersession(crate::handoff::RecordSupersession {
                    manifest_path,
                    lane_id: &lane,
                    supersession,
                    now,
                })
                .await
            }
        }
        // pi `:6290-6292`.
        .map_err(|error| ToolError::new(error.to_string()))?;

        let reference = serde_json::to_value(&recorded.reference)
            .map_err(|error| ToolError::new(error.to_string()))?;
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(recorded.text)],
            details: Some(serde_json::json!({
                "mode": "management",
                "results": [],
                // pi `:6289`. Read today by `missions/lifecycle.rs:431` and `:893`.
                "parallelHandoff": reference,
            })),
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }

    /// Tier-1 dispatch arm (C5): route `status`/`interrupt`/`resume`/`append-step` to the
    /// [`crate::background::control`] primitives + the [`crate::background::run_status`] report shape
    /// (including the no-id "list active runs" form) — pi `subagent-executor.ts:2845-2912` +
    /// `run-status.ts:101-273`. Each arm delegates to the matching [`crate::extension::SubagentExecutor`] method (the
    /// SAME shared executor the slash commands route through, R-SA-130); a rendered report/list is
    /// returned as tool content, a user-facing failure (not-found, wrong-mode, no-transcript, …) as
    /// a [`ToolError`] (cyrup's error-result channel, since [`ToolResult`] carries no `isError`
    /// flag).
    async fn route_control_action(
        &self,
        action: &str,
        p: &SubagentToolParams,
        cwd: &Path,
    ) -> Result<ToolResult, ToolError> {
        // SUBA-064 / pi `subagent-executor.ts:4412-4423` @v0.43.0. The authority consult sits
        // BEFORE dispatch and covers `stop`→`stopRun`, `steer`→`steerRun` (and `schedule.create`
        // →`scheduleCreate`, which SUBA-016 landed: `route_action`'s `schedule.*` arm runs this
        // same three-arm gate, so the mapping has one meaning on both surfaces). Every other
        // control verb is ungated upstream and must stay ungated here, which is why the mapping
        // returns `None` for them rather than this arm defaulting to "gated".
        if let Some(policy_action) =
            crate::registration::authority::AuthorityAction::for_tool_action(action)
        {
            use crate::registration::authority as auth;
            let policy = self.executor.config_snapshot().await.authority_policy;
            match auth::resolve_authority_decision(policy_action, policy.as_ref()) {
                auth::AuthorityDecision::Auto => {}
                auth::AuthorityDecision::Forbid => {
                    return Err(ToolError::new(auth::forbidden_message(action)));
                }
                auth::AuthorityDecision::Confirm => {
                    // pi `if (!ctx.hasUI) return …` (`:4419`): with no interactive UI there is
                    // nobody to grant the authority, so the action is refused rather than
                    // silently auto-granted. cyrup's `hasUI` is "a live host-services surface
                    // exists"; without one `HostServices::confirm`'s own default is `false`
                    // (deny), so the two agree even if this branch is ever bypassed.
                    let Some(services) = self.executor.host_services() else {
                        return Err(ToolError::new(auth::no_ui_message(action)));
                    };
                    if !services.confirm(
                        &auth::confirm_prompt(action),
                        &auth::confirm_message(action),
                        &cyrup_ext::host::DialogOptions::default(),
                    ) {
                        // pi returns the DECLINED text WITHOUT `isError: true` (`:4421`) — a user
                        // declining is a choice, not a failure — so this is an Ok result, not an
                        // `Err`, and it is the one refusal on this path that is not a `ToolError`.
                        return Ok(ToolResult {
                            content: vec![cyrup_core::Content::text(auth::declined_message(
                                action,
                            ))],
                            details: Some(serde_json::json!({ "mode": "management" })),
                            terminate: TerminateHint::Unspecified,
                            ..Default::default()
                        });
                    }
                }
            }
        }

        let index = p.index.and_then(|value| usize::try_from(value).ok());
        let outcome = match action {
            // pi `subagent-executor.ts:6517-6538` @v0.68.0: the target is `id ?? runId` (`:6517`),
            // and the two refusals come in upstream's order — no target (`:6532`) BEFORE a view
            // (`:6535`) — so `{ view }` alone is refused for the missing target, not the view.
            "debug.run" => {
                use crate::background::run_lifecycle_debug::{
                    DEBUG_RUN_NO_VIEWS, DEBUG_RUN_REQUIRES_TARGET,
                };
                let target = p.id.as_deref().or(p.run_id.as_deref());
                if target.is_none() && p.dir.is_none() {
                    Err(DEBUG_RUN_REQUIRES_TARGET.to_string())
                } else if p.view.is_some() {
                    Err(DEBUG_RUN_NO_VIEWS.to_string())
                } else {
                    self.executor
                        .control_debug_run(cwd, target, p.dir.as_deref())
                        .await
                }
            }
            "status" => {
                // pi `params.id ?? params.runId` (`subagent-executor.ts:2846`): `id` takes priority,
                // but a caller using `runId` alone must still resolve to that run's report instead of
                // falling through to the no-id "list active runs" view.
                let target = p.id.as_deref().or(p.run_id.as_deref());
                // G92: `view`/`lines`/`index` are threaded through here — this is the dispatch arm
                // the two new schema properties are advertised against.
                self.executor
                    .control_status_view(
                        cwd,
                        target,
                        p.dir.as_deref(),
                        !self.allow_mutating_management,
                        StatusViewSelector {
                            view: p.view.as_deref(),
                            lines: p.lines,
                            index,
                        },
                    )
                    .await
            }
            "interrupt" => {
                // pi interrupt prefers `runId` over `id` (`subagent-executor.ts:2872`).
                let target = p.run_id.as_deref().or(p.id.as_deref());
                self.executor.control_interrupt(cwd, target).await
            }
            // G77: the `stop` dispatch arm the schema enum value is advertised against (the
            // advertise-vs-dispatch invariant — both land in this same change). pi reads
            // `targetRunId = params.runId ?? params.id` (`subagent-executor.ts:4772`), the SAME
            // `runId`-first precedence `interrupt` uses, and also accepts `dir` (`:4779-4787`).
            // SUBA-087: `childId` (`subagent-executor.ts:6163,6184` @v0.64.0) rides along on both
            // the `dir` and the id forms, so a child-scoped stop reaches `control_stop`'s resolver
            // instead of being silently dropped by serde.
            "stop" => {
                let target = p.run_id.as_deref().or(p.id.as_deref());
                self.executor
                    .control_stop(cwd, target, p.dir.as_deref(), p.child_id.as_deref())
                    .await
            }
            // SUBA-057: the `dismiss` dispatch arm the schema enum value is advertised against
            // (the advertise-vs-dispatch invariant — both land in this same change). pi reads
            // `targetRunId = paramsWithResolvedCwd.runId ?? paramsWithResolvedCwd.id`
            // (`subagent-executor.ts:5873`), the SAME `runId`-first precedence `stop` and
            // `interrupt` use. Unlike `stop`, upstream accepts NO `dir` form here — `dismiss` is
            // id-addressed only — so `p.dir` is deliberately not threaded through.
            "dismiss" => {
                // SUBA-057 — upstream's child-safe gate for this verb. `dismiss` is a member of
                // pi's `MUTATING_MANAGEMENT_ACTIONS` (`subagent-executor.ts:175` @v0.47.1), and
                // the `if (deps.allowMutatingManagementActions === false && …)` block at `:5865`
                // runs immediately BEFORE the `if (action === "dismiss")` block at `:5872` — so a
                // fanout child gets the refusal, never the handler.
                //
                // The check is spelled here rather than by extending
                // [`crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS`] for the reason that
                // slice's own doc gives: it gates [`SubagentTool::route_management_action`], which
                // `dismiss` does not route through, so a name added there would be enforced nowhere
                // and merely widen a list used for a different purpose. The sentence is upstream's,
                // byte for byte, and is the same one every other child-safe refusal in this file
                // emits.
                if !self.allow_mutating_management {
                    return Err(ToolError::new(format!(
                        "Action '{action}' is not available from child-safe subagent fanout mode."
                    )));
                }
                let target = p.run_id.as_deref().or(p.id.as_deref());
                self.executor.control_dismiss(cwd, target).await
            }
            "resume" => {
                // pi `resumeAsyncRun` (`subagent-executor.ts:1456-1469`): a resume may carry an
                // attach-chain, whose steps' `acceptance` is validated with pi's own prefix before
                // anything is enqueued (SUBA-N04 — those policies are now really honoured).
                let acceptance_errors = validate_execution_acceptance(p);
                if !acceptance_errors.is_empty() {
                    return Err(ToolError::new(format!(
                        "Cannot resume: {}",
                        acceptance_errors.join(" ")
                    )));
                }
                let target = p.id.as_deref().or(p.run_id.as_deref());
                self.executor
                    .control_resume(cwd, target, p.message.as_deref(), p.task.as_deref(), index)
                    .await
            }
            // G90: the `steer` dispatch arm the schema enum value above is advertised against.
            "steer" => {
                self.executor
                    .control_steer(
                        cwd,
                        p.run_id.as_deref().or(p.id.as_deref()),
                        p.dir.as_deref(),
                        p.message.as_deref(),
                        p.task.as_deref(),
                        index,
                        // SUBA-049: the advertised `mode` parameter, now with a consumer.
                        p.mode.as_deref(),
                    )
                    .await
            }
            "append-step" => {
                // pi `appendStepToRun` (`subagent-executor.ts:791-798`), same rule, pi's own prefix.
                let acceptance_errors = validate_execution_acceptance(p);
                if !acceptance_errors.is_empty() {
                    return Err(ToolError::new(format!(
                        "Cannot append step: {}",
                        acceptance_errors.join(" ")
                    )));
                }
                let target = p.id.as_deref().or(p.run_id.as_deref());
                self.executor
                    .control_append_step(cwd, target, p.chain.as_deref().unwrap_or(&[]))
                    .await
            }
            // SUBA-038 residual 3: this arm is defensively unreachable (only the control band's
            // verbs in `route_action` reach `route_control_action`), but it listed "status, interrupt, resume, steer,
            // append-step" and omitted `stop` — which IS advertised and IS dispatched two arms up.
            // Upstream has no separate control-arm message at all: every unknown action lands on
            // the one `unknownSubagentActionMessage` (`subagent-executor.ts:195`), so routing here
            // through the same function is the port, and it cannot drift again by construction.
            other => Err(unknown_subagent_action_message(other)),
        };
        match outcome {
            Ok(text) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(text)],
                details: Some(serde_json::json!({ "mode": "management" })),
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            }),
            Err(message) => Err(ToolError::new(message)),
        }
    }

    /// Tier-1 dispatch arm (parallel major, `parallel-execution.test.ts:174-376`): translate the
    /// tool's top-level PARALLEL shape (`tasks[]` + `concurrency`/`worktree`, per-task
    /// `count`/`output`/`outputMode`/`reads`/`model`) into a single [`RunnerStep::ParallelGroup`]
    /// and route it through the SAME shared plan-execution path
    /// ([`crate::extension::SubagentExecutor::run_or_background_graph`]) the slash commands use — so each task's REAL
    /// persona (T0.1/C13) is resolved and dispatched through the faithful
    /// [`crate::spawn::parallel::run_bounded`] worker pool over real child processes.
    ///
    /// Faithful pi behaviors reproduced here: per-task `count` fan-out multiplication
    /// (`expandTopLevelTaskCounts`, `subagent-executor.ts:1986`); duplicate-output-path rejection
    /// BEFORE any spawn (`findDuplicateParallelOutputPath`, `subagent-executor.ts:2921-2944`); and the
    /// `N/M succeeded` result summary (`subagent-executor.ts:2921-2944`).
    pub(crate) async fn route_parallel_mode(
        &self,
        p: &SubagentToolParams,
        cwd: &Path,
        cancel: CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let raw = p.tasks.as_deref().unwrap_or(&[]);
        let items = parse_tool_task_items(raw, true)?;
        // Expand `count` FIRST (matching pi's `normalizeRepeatedParallelCounts` -> later
        // `findDuplicateParallelOutputPath`), so a `count`-multiplied task with a fixed output path
        // is itself caught as a duplicate rather than slipping through the pre-expansion check.
        let expanded = expand_top_level_task_counts(items).map_err(ToolError::new)?;
        if let Some(dup) = find_duplicate_parallel_output(&expanded) {
            return Err(ToolError::new(dup));
        }
        let specs: Vec<SingleStepSpec> = expanded.iter().map(tool_task_to_spec).collect();
        let agents: Vec<String> = specs.iter().map(|spec| spec.agent.clone()).collect();

        let cfg = self.executor.config_snapshot().await;
        // pi: `resolveTopLevelParallelConcurrency(params.concurrency, config.parallel.concurrency)`
        // — an explicit positive `concurrency` wins; otherwise the config default (4).
        let concurrency = p
            .concurrency
            .and_then(|c| u32::try_from(c).ok())
            .filter(|c| *c > 0)
            .unwrap_or(cfg.parallel_concurrency());
        // LANES_2 — pi `:3805-3808`: the lane is normalized at the dispatch boundary and an
        // invalid one is answered with the normalizer's own sentence, not a schema rejection. The
        // normalizer is landed (`crate::workflows::normalize_workflow_lane_metadata`, an exact
        // port of `lane-metadata.ts:49-70`) and already carries upstream's eight messages
        // verbatim, which is why this is one line and not a second validator.
        let lane = crate::workflows::normalize_workflow_lane_metadata(p.lane.as_ref(), "lane")
            .map_err(|error| ToolError::new(error.message().to_string()))?;
        let group = RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: specs,
            concurrency,
            fail_fast: false,
            worktree: p.worktree.unwrap_or(false),
            // Recorded onto every child row of the handoff manifest by `run_parallel_group`, so
            // the lane's `claims` and `outputPaths` survive the fan-out as durable evidence.
            lane,
        });

        let context = p.context_override();
        let depth = resolve_effective_depth(cfg.max_subagent_depth).current_depth;
        // SUBA-077: this site used to hard-code `None`, which dropped an EXPLICIT call-site
        // `timeoutMs` on the floor as well as skipping the default. Resolved on the same terms as
        // SINGLE and CHAIN — upstream's `!async` arm covers `tasks: []` launches too, since
        // `isComposite` suppresses only the async default (`subagent-executor.ts:2724` @v0.57.0).
        // A top-level parallel call names many agents, so there is no agent-frontmatter rung.
        let background = p.is_background(&cfg, depth);
        let timeout_ms = resolve_foreground_timeout(
            p,
            foreground_timeout_default(background, Option::None, cfg.timeout_ms.as_ref()),
        )
        .map_err(ToolError::new)?;
        match self
            .executor
            .run_or_background_graph(
                cwd,
                vec![group],
                RunMode::Parallel,
                context,
                background,
                p.task.clone(),
                cancel,
                timeout_ms,
                // SUBA-N05: `control` is a top-level param on pi's `SubagentParams`, so it applies
                // to a PARALLEL invocation exactly as it does to a SINGLE one — `runParallelPath`
                // shares `ExecutionContextData.controlConfig` with every other mode
                // (`subagent-executor.ts:3385` @v0.34.0 — one resolution at the shared `execute`
                // entry, read by every path off `ExecutionContextData.controlConfig`).
                p.control
                    .as_ref()
                    .map(crate::exec::control::parse_control_overrides),
                // SUBA-N06: `includeProgress` is likewise a top-level `SubagentParams` field, so it
                // applies to PARALLEL/CHAIN exactly as it does to SINGLE — pi gates
                // `details.progress` on it in `runParallelPath` (`subagent-executor.ts:3444`) and
                // threads it into `executeChain` (`:2012`), both @v0.34.0.
                p.include_progress,
                // ...but `chainDir` is NOT such a field: pi resolves it only in `runChainPath`
                // (`subagent-executor.ts:2623`), never in `runParallelPath`, so a bare PARALLEL run
                // keeps the default scratch dir even when the caller sent one.
                None,
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
        {
            // pi `executeAsyncChain` (`async-execution.ts:1152-1161`): a bare PARALLEL call is a
            // length-1 chain of one parallel step, so `chainDesc` is just that group's own
            // `[a+b+c]` descriptor; the headline is `Async parallel: {chainDesc} [{id}]`.
            GraphRunOutcome::Background(run_id) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format_async_started_message(
                    &format!("Async parallel: [{}] [{run_id}]", agents.join("+")),
                ))],
                // `asyncDir` per `async-execution.ts:1191` — see [`async_dir_for_run`].
                details: Some(async_launch_details(
                    "parallel",
                    &run_id,
                    cwd,
                    &self.executor.config_snapshot().await.roots,
                    // A top-level PARALLEL launch names many agents; one flattened surface would be
                    // a claim about none of them, so the key is omitted.
                    None,
                )),
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            }),
            GraphRunOutcome::Foreground { run_id, groups, .. } => {
                let (summary, details) = match groups.first() {
                    Some(group) => {
                        let total = group.children.len();
                        let ok = group
                            .children
                            .iter()
                            .filter(|c| matches!(c, Some(r) if r.success))
                            .count();
                        // R-SA-123/124/125: attempt out-of-band delivery of the FULL grouped result
                        // through the intercom `DeliveryChannel`. On a confirmed delivery, the inline
                        // tool payload is REDUCED — the heavy per-task `final_output` block that
                        // `render_parallel_tool_summary` inlines is dropped in favor of pi's own
                        // `formatSubagentResultReceipt` text (`result-intercom.ts:376-421`) — else the
                        // full inline summary is preserved (never delivered instead-of, always
                        // in-addition-to). Uses the `NoTransportChannel` default (→ NotDelivered, full
                        // inline kept) until `with_channels` wires the real broker channel.
                        let success = ok == total && total > 0;
                        let top_agent = agents
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "subagent".to_string());
                        // pi always cites the run's OWN real id in the payload/receipt
                        // (`result-intercom.ts:256,347` @v0.34.0) — never a fresh id minted only for this
                        // message, so a follow-up status/resume action can correlate on it.
                        let payload = crate::tui::intercom::IntercomPayload::from_group_children(
                            run_id.clone(),
                            top_agent,
                            success,
                            &group.children,
                        );
                        match self
                            .executor
                            .deliver_group_out_of_band(payload.clone())
                            .await
                        {
                            crate::tui::intercom::DeliveryOutcome::Delivered => {
                                let reduced =
                                    crate::tui::intercom::ReducedInlinePayload::from(&payload);
                                // pi's `formatSubagentResultReceipt` (`result-intercom.ts:376-421`):
                                // mode label + "Run: …" + "Children: {status counts}" + closing line.
                                let receipt = crate::tui::intercom::format_subagent_result_receipt(
                                    "parallel",
                                    &run_id,
                                    &payload.child_statuses,
                                );
                                (
                                    receipt,
                                    serde_json::json!({
                                        "mode": "parallel", "total": total, "succeeded": ok,
                                        "outOfBandDelivered": true, "reduced": reduced,
                                    }),
                                )
                            }
                            crate::tui::intercom::DeliveryOutcome::NotDelivered => (
                                render_parallel_tool_summary(group, &agents),
                                serde_json::json!({
                                    "mode": "parallel", "total": total, "succeeded": ok,
                                    "outOfBandDelivered": false,
                                }),
                            ),
                        }
                    }
                    None => (
                        "0/0 succeeded".to_string(),
                        serde_json::json!({ "mode": "parallel", "total": 0, "succeeded": 0 }),
                    ),
                };
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(summary)],
                    details: Some(details),
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
        }
    }

    /// Tier-1 dispatch arm (chain via tool): translate `chain[]` into a `Vec<RunnerStep>`
    /// (sequential steps + inline static parallel groups, each group's per-task `count` expanded via
    /// pi's `expandChainParallelCounts`) and route it through the SAME
    /// [`crate::extension::SubagentExecutor::run_or_background_graph`] path the slash commands use. Dynamic fanout
    /// (`expand`/`collect`) is Tier-4 territory (C16) and is rejected with a clear message rather
    /// than silently mis-parsed.
    pub(crate) async fn route_chain_mode(
        &self,
        p: &SubagentToolParams,
        cwd: &Path,
        cancel: CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let raw = p.chain.as_deref().unwrap_or(&[]);
        let cfg = self.executor.config_snapshot().await;
        let graph = parse_tool_chain_items(raw, cfg.parallel_concurrency())?;
        let context = p.context_override();
        let depth = resolve_effective_depth(cfg.max_subagent_depth).current_depth;
        // pi `resolveForegroundTimeout` (`subagent-executor.ts:2689` @v0.57.0): `timeoutMs`/
        // `maxRuntimeMs` are aliases, resolved once up front here exactly as SINGLE mode does.
        //
        // SUBA-077: upstream's `!async` arm applies the foreground backstop to composite launches
        // too — `resolveSingleAgentLaunchTimeout`'s `isComposite` test only suppresses the ASYNC
        // default (`:2724`). A chain names many agents, so there is no single agent-frontmatter
        // rung to pass.
        let background = p.is_background(&cfg, depth);
        let timeout_ms = resolve_foreground_timeout(
            p,
            foreground_timeout_default(background, Option::None, cfg.timeout_ms.as_ref()),
        )
        .map_err(ToolError::new)?;
        // SUBA-N03: no timeout-vs-async refusal here any more. The one that stood here cited
        // `subagent-executor.ts:3022-3023` as "pi's own precedent" and mirrored `route_single`'s
        // identically-cited SINGLE-mode refusal. The citation is false in both places — at v0.34.0
        // `:3015-3030` is foreground intercom-receipt construction — and upstream does the
        // opposite for CHAIN/PARALLEL too: `executeAsyncChain(id, { …, timeoutMs: data.timeoutMs,
        // … })` (`subagent-executor.ts:2568`) arms `deadlineAt = Date.now() + params.timeoutMs`
        // (`async-execution.ts:677`) and hands both to the runner (`:723,798`). `RunnerConfig` now
        // carries the same pair, so the timeout is HONOURED on this path rather than refused.
        // Captured before `graph` moves into `run_or_background_graph` below — only needed for the
        // out-of-band intercom payload's top-level `agent` label (R-SA-123/124).
        let top_agent = plan_step_agent_names(&graph)
            .into_iter()
            .next()
            .unwrap_or_else(|| "subagent".to_string());
        // Captured before `graph` moves into `run_or_background_graph` below — pi's `chainDesc`
        // (`async-execution.ts:1183-1197`), needed only for the async-start headline.
        let chain_desc = describe_chain(&graph);
        match self
            .executor
            .run_or_background_graph(
                cwd,
                graph,
                RunMode::Chain,
                context,
                background,
                p.task.clone(),
                cancel,
                timeout_ms,
                // SUBA-N05 (pi `runChainPath` -> `resolveControlConfig(deps.config.control,
                // params.control)`, `subagent-executor.ts:1133` @v0.34.0).
                p.control
                    .as_ref()
                    .map(crate::exec::control::parse_control_overrides),
                // SUBA-N06: `includeProgress` is likewise a top-level `SubagentParams` field, so it
                // applies to PARALLEL/CHAIN exactly as it does to SINGLE — pi gates
                // `details.progress` on it in `runParallelPath` (`subagent-executor.ts:3444`) and
                // threads it into `executeChain` (`:2012`), both @v0.34.0.
                p.include_progress,
                // pi `chainDir: params.chainDir ?? getProjectChainRunsDir(effectiveCwd)`
                // (`subagent-executor.ts:2623` @v0.43.0). THE one caller that forwards it: `:2022`
                // sits in `runChainPath`, and this is cyrup's CHAIN arm.
                p.chain_dir.clone().map(PathBuf::from),
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
        {
            // pi `executeAsyncChain` (`async-execution.ts:1152-1161`): headline `Async chain: {chainDesc}
            // [{id}]` followed by `formatAsyncStartedMessage`'s fixed guidance; `details` is
            // `{ mode: "chain", runId, results: [], asyncId }`.
            GraphRunOutcome::Background(run_id) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format_async_started_message(
                    &format!("Async chain: {chain_desc} [{run_id}]"),
                ))],
                // `asyncDir` per `async-execution.ts:1191` — see [`async_dir_for_run`].
                details: Some(async_launch_details(
                    "chain",
                    &run_id,
                    cwd,
                    &self.executor.config_snapshot().await.roots,
                    // As PARALLEL: a chain names many agents.
                    None,
                )),
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            }),
            GraphRunOutcome::Foreground {
                run_id,
                results,
                is_group,
                groups,
            } => {
                let text = render_chain_results(&results, &is_group, &groups);
                let steps = results.len();

                // R-SA-123/124/125: pi attempts out-of-band result-intercom delivery for EVERY
                // foreground mode (single/parallel/chain), not parallel alone
                // (`result-intercom.ts:245-281` as consumed by every `subagent-executor.ts`
                // foreground path) — this mirrors `route_parallel_mode`'s identical wiring. Flatten
                // each step's real child(ren) into one position-ordered list, exactly as
                // `render_chain_results` above zips `is_group`/`groups` back together: a plain step
                // contributes its own result, a parallel-group step contributes each of its
                // fanned-out children.
                let mut children: Vec<Option<StepResult>> = Vec::with_capacity(steps);
                let mut group_cursor = 0usize;
                for (i, result) in results.iter().enumerate() {
                    if is_group.get(i).copied().unwrap_or(false) {
                        if let Some(group) = groups.get(group_cursor) {
                            children.extend(group.children.iter().cloned());
                        }
                        group_cursor += 1;
                    } else {
                        children.push(Some(result.clone()));
                    }
                }
                let success = !results.is_empty() && results.iter().all(|r| r.success);
                // pi always cites the run's OWN real id in the payload/receipt
                // (`result-intercom.ts:256,347` @v0.34.0) — never a fresh id minted only for this message.
                let payload = crate::tui::intercom::IntercomPayload::from_group_children(
                    run_id.clone(),
                    top_agent,
                    success,
                    &children,
                );
                let (text, details) = match self
                    .executor
                    .deliver_group_out_of_band(payload.clone())
                    .await
                {
                    crate::tui::intercom::DeliveryOutcome::Delivered => {
                        let reduced = crate::tui::intercom::ReducedInlinePayload::from(&payload);
                        // pi's `formatSubagentResultReceipt` (`result-intercom.ts:376-421`).
                        let receipt = crate::tui::intercom::format_subagent_result_receipt(
                            "chain",
                            &run_id,
                            &payload.child_statuses,
                        );
                        (
                            receipt,
                            serde_json::json!({
                                "mode": "chain", "steps": steps,
                                "outOfBandDelivered": true, "reduced": reduced,
                            }),
                        )
                    }
                    crate::tui::intercom::DeliveryOutcome::NotDelivered => (
                        text,
                        serde_json::json!({
                            "mode": "chain", "steps": steps, "outOfBandDelivered": false,
                        }),
                    ),
                };
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(text)],
                    details: Some(details),
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
        }
    }
}

/// Render a [`crate::workflows::scripted::WorkflowScriptValidationResult`] as the model-facing
/// re-prompt both validation
/// surfaces use — `action: "validate"` ([`SubagentTool::route_action`]) and the pre-run check
/// inside [`SubagentTool::route_workflow_mode`].
///
/// ONE formatter deliberately: two copies is how the two surfaces drift.
///
/// `WorkflowScriptValidationResult { ok: bool, errors: Vec<WorkflowScriptValidationError> }` —
/// `ok` is a plain FIELD here, unlike `AnalysisReport::ok` which is a method;
/// `validate_workflow_script` bridges the two, which is why callers must use its result and never
/// `analyze_workflow_script`'s.
///
/// BOTH arms are reachable, so neither is dead code:
///
/// * LOCATED — the structural analyzer fills `line`/`column` from its `position()` helper for
///   nested-async and unknown-global findings (`analyzer.rs:297`, `:328`, helper `:359-363`),
///   reporting 1-based positions in the SCRIPT's own coordinates rather than the `(async () => {`
///   wrapper's (`analyzer.rs`'s own test asserts exactly that).
/// * UNLOCATED — `compile_check` hardcodes both to `None` (`engine.rs:2517`), as do the
///   empty-script finding and the nested-workflow finding.
///
/// So a V8 syntax error prints bare and a structural finding prints `line:column message`.
fn render_validation_findings(
    report: &crate::workflows::scripted::WorkflowScriptValidationResult,
) -> String {
    let mut lines = Vec::with_capacity(report.errors.len() + 1);
    lines.push(if report.ok {
        "workflowScript is valid.".to_string()
    } else {
        format!("workflowScript has {} finding(s):", report.errors.len())
    });
    for finding in &report.errors {
        lines.push(match (finding.line, finding.column) {
            (Some(line), Some(column)) => format!("  {line}:{column} {}", finding.message),
            _ => format!("  {}", finding.message),
        });
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;

/// SUBA-016 part B's end-to-end suite — a `#[path]` sibling for the same reason
/// [`tests`](self::tests) is one: it drives `Tool::execute` and the installed
/// `ScheduledRunManager`, both of which live inside this private module tree.
#[cfg(test)]
#[path = "scheduled_runs_tests.rs"]
mod scheduled_runs_tests;

/// LANES_2's end-to-end suite — a `#[path]` sibling for the same reason the two above are: it
/// drives `Tool::execute` against the `worktree.cleanup` arm in this file.
#[cfg(test)]
#[path = "worktree_cleanup_tests.rs"]
mod worktree_cleanup_tests;

/// LANES_2's second end-to-end suite — `worktree.discard` and the three `lane.*` verbs, against a
/// real git worktree and a real manifest. A `#[path]` sibling for the same reason the three above
/// are: every row drives `Tool::execute` against the arms in this file.
#[cfg(test)]
#[path = "lane_actions_tests.rs"]
mod lane_actions_tests;

// `children.list` — the reachability tests for the retained-children listing, a `#[path]`
// sibling like the two above so `routing_tests.rs` does not grow past its already-largest size.
#[cfg(test)]
#[path = "children_list_tests.rs"]
mod children_list_tests;
