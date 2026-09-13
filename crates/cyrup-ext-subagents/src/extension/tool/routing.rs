//! The `subagent` tool's dispatch table: one `route_*` arm per mode and per management/control
//! action.

use std::path::{Path, PathBuf};

use cyrup_core::{CancelToken, ModelId, TerminateHint, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

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

        // Structural validation BEFORE a child is spent — this port's stated advantage over
        // upstream, which discovers a bad global as a runtime ReferenceError after the fact.
        // `state_enabled: false` matches this build's `state: None` below; passing `true` would
        // ACCEPT a `state.get(...)` the run then refuses at the op, which is the worst of both.
        let report = crate::workflows::scripted::validate_workflow_script(script, false)
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

        // WORKFLOW_6/WORKFLOW_3 §3d — ONE workflow run id for the whole call, minted BEFORE the
        // engine runs. It is three things at once and all three need it up front:
        //   1. the `workflow_controllers` key for the lifetime of this call (pi `:5095-5098`),
        //   2. every child's `parent_workflow_run_id` (pi `prepareWorkflowChildLaunchParams`,
        //      `:5633`/`:5900`, whose `parentWorkflowRunId: workflowRunId` reaches the child's
        //      control entry at `:7019`),
        //   3. the receipt's directory name and the id the receipt cross-checks itself against.
        // Pre-WORKFLOW_6 it was minted after `run_workflow_script` returned, which made (1) and (2)
        // impossible.
        let workflow_run_id = crate::background::RunId::new();
        let run_dir_name = crate::identity::RunDirName::for_run(&workflow_run_id);

        // The run dir the receipt was already being written into (`attach_workflow_receipt`), created up
        // front now, because it is a real run directory from this point on — not a drop box.
        // `ensure_accessible_dir` is REQUIRED, not defensive: `write_atomic_json` has no implicit
        // `mkdir -p` (`atomic.rs:105-107`).
        let async_root = crate::extension::executor::paths::default_async_root_in(&cfg.roots, cwd);
        let run_dir = run_dir_name.resolve_in(&async_root);
        crate::background::ensure_accessible_dir(&run_dir)
            .await
            .map_err(|e| ToolError::new(format!("workflow run directory could not be created: {e}")))?;

        // THE FIRST PRODUCTION `RunMode::Workflow` IN THE CRATE.
        let mut status = crate::background::RunStatus::queued(
            workflow_run_id.clone(),
            crate::background::RunMode::Workflow,
            // §2.1 — this process drives it; reconcile.rs step 4 must be able to probe us.
            Some(std::process::id()),
        );
        status.session_id = crate::identity::SessionId::parse_opt(
            self.executor.current_session_id().as_deref(),
        );
        status.cwd = Some(cwd.to_path_buf());
        // §2.3 — the REAL parent tool call id. `with_workflow_children` (settlement.rs:102-105) falls
        // back to the run id when this is None; the real value is strictly better and is in hand.
        status.tool_call_id = Some(call_id.as_str().to_string());
        // A workflow's step count is DISCOVERED, not declared — exactly what `state.rs:31`'s own doc says
        // distinguishes it from a Chain. Leave it None; §3.3 fills `steps` as children settle.
        status.chain_step_count = None;
        // ⚠ §2.5 — MUST happen before the engine runs. `Queued -> Complete` is not a legal transition,
        // so an instantly-succeeding workflow would fail its terminal write from `Queued`.
        status
            .advance_state(crate::background::RunState::Running)
            .map_err(|e| ToolError::new(e.to_string()))?;
        crate::background::atomic::write_atomic_json(&run_dir.join("status.json"), &status)
            .await
            .map_err(|e| ToolError::new(format!("workflow status could not be written: {e}")))?;

        // ONE record, shared. The host refreshes `steps` in place as children settle (§3.3) and
        // §3.2's terminal write starts from whatever the host last published — never from a
        // second, divergent copy that would silently drop the `last_update` the host advanced.
        // `std::sync::Mutex`: every access here is a short synchronous clone with no `.await`
        // inside the critical section (the host's own `publish_steps` is written to guarantee it).
        let status = std::sync::Arc::new(std::sync::Mutex::new(status));

        // pi `const controller = new AbortController(); state.workflowControllers.set(workflowRunId,
        // controller)` — with cyrup's ONE abort mechanism, which is the token already threaded into
        // `RunWorkflowScriptOptions::cancel` below, so an abort through the registry aborts the real
        // engine run.
        let _controller = self
            .executor
            .register_workflow_controller(&workflow_run_id, cancel.clone());

        let host = std::sync::Arc::new(crate::extension::executor::workflow::WorkflowRunHost::new(
            std::sync::Arc::clone(&self.executor),
            cwd.to_path_buf(),
            // Moved, not cloned — `WorkflowRunHost::new` does the `Arc<Mutex<…>>` wrap that lets
            // each child borrow a forwarding share of this one sink.
            on_update,
            workflow_run_id.clone(),
            std::sync::Arc::clone(&status),
            run_dir.clone(),
        ));

        // Field order mirrors `RunWorkflowScriptOptions`' own declaration order, so this block can
        // be diffed against the struct line by line. The struct has no `Default`, so all thirteen
        // are named.
        let outcome = crate::workflows::scripted::run_workflow_script(
            crate::workflows::scripted::RunWorkflowScriptOptions {
                script: script.to_string(),
                // No resource provenance is wired in this build.
                one_use_permit: None,
                timeout_ms,
                // The workspace's ONE abort flag — no subsystem invents its own.
                cancel: Some(cancel),
                continue_after_abort_when_children_settled: None,
                // -> `DEFAULT_GLOBAL_CONCURRENCY_LIMIT` (`spawn/parallel.rs:36`, 20), which is
                // exactly what `count_requested_subagent_spawns` bills the session for.
                global_concurrency_limit: None,
                host: std::sync::Arc::clone(&host)
                    as std::sync::Arc<dyn crate::workflows::scripted::WorkflowScriptHost>,
                // No mission state -> the verbatim "Workflow state is unavailable without a
                // mission." refusal at the ops, AND `globalThis.state` is never installed in the
                // guest realm (`prelude.js:578`, via the engine's `state_enabled` derivation).
                state: None,
                // No external stop channel in a foreground tool call.
                register_stop_child: None,
                // `None`, NOT a caller-side journal. The engine OWNS the trace and returns it
                // COMPLETE on both arms (`WorkflowScriptResult::trace`, and
                // `WorkflowScriptError::partial.trace` on failure). `on_trace` is pure telemetry —
                // its own doc says it "never decides workflow outcomes" — so accumulating from it
                // would be a second copy of data that is already handed back.
                on_trace: None,
                // `runs.lanes` still WORKS; only the advisory plan callback is undelivered.
                on_lane_plan: None,
                // `None`, for TWO reasons. (a) `emit` hands the callback the WHOLE accumulated
                // snapshot every time, so a per-value forwarder re-forwards everything on every
                // emit, quadratically — and `WorkflowScriptResult::emits` already carries the final
                // list. (b) STRONGER: this is the ONE callback whose failure ABORTS the run, so a
                // forwarder that errors kills the workflow. Live emit forwarding is a real feature,
                // but it needs a delta the engine does not offer.
                on_emit: None,
                // `supports_host()` is false, so no host step can be produced.
                on_host_step: None,
            },
        )
        .await;

        // WORKFLOW_6 §4.4 — pi's settlement `finally` (`subagent-executor.ts:5789-5799`):
        // "Idempotent cleanup only". Unconditional and ahead of the Ok/Err match, because a
        // workflow that FAILED is exactly as settled as one that succeeded — leaving the entry
        // behind would make WORKFLOW_10 count a dead workflow forever and make WORKFLOW_8's
        // dismiss refusal permanent for this id.
        self.executor.settle_workflow_controller(&workflow_run_id);

        match outcome {
            Ok(result) => {
                let preview =
                    crate::workflows::scripted::format_workflow_json_preview(&result.value, 1_000)
                        .unwrap_or_else(|| "undefined".to_string());
                let mut text = format!(
                    "Workflow completed with {} child run(s). Return: {preview}",
                    result.children.len()
                );
                if !result.emits.is_empty() {
                    let rendered: Vec<String> = result
                        .emits
                        .iter()
                        .map(|value| {
                            crate::workflows::scripted::format_workflow_json_preview(value, 200)
                                .unwrap_or_else(|| "undefined".to_string())
                        })
                        .collect();
                    text.push_str(&format!(" Emitted: {}", rendered.join(", ")));
                }
                text.push_str(&format!(" Trace: {} event(s).", result.trace.len()));
                // §3.2: settle via the composer (first production caller of plan_workflow_settlement).
                let details = self
                    .settle_foreground_workflow(
                        serde_json::json!({
                            // The inline surface's typed payloads
                            // (`IntercomPayload::from_group_children`) are keyed to
                            // `StepResult`/`SingleResult`, not to `WorkflowScriptChildResult`, and
                            // no conversion exists — so v1 attaches the children as plain JSON
                            // rather than inventing a lossy `StepResult` shim that a follow-up
                            // task would have to unpick. `WorkflowScriptChildResult` derives
                            // `Serialize` with `rename_all = "camelCase"`, so this is upstream's
                            // own wire shape. `RunMode::Workflow` already exists and is already
                            // rendered as "workflow" elsewhere, so the mode label is the real one,
                            // not a placeholder.
                            "mode": "workflow",
                            // Cloned: `&result.children` is also borrowed below, for the receipt.
                            "children": result.children.clone(),
                            "trace": result.trace.len(),
                            "emits": result.emits,
                            "console": result.console,
                        }),
                        &status,
                        crate::background::RunState::Complete,
                        &run_dir,
                        &run_dir_name,
                        crate::workflows::WorkflowReceiptState::Complete,
                        &result.children,
                        &result.trace,
                        text.clone(),
                    )
                    .await
                    .map_err(ToolError::new)?;
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(text)],
                    details: Some(details),
                    terminate: TerminateHint::Unspecified,
                    ..Default::default()
                })
            }
            Err(error) => {
                // The partial IS the point of this error type: a failed workflow still yields its
                // trace, children, console and emits, and they must reach the caller.
                let mut message = error.message.clone();
                let detached: Vec<&str> = error
                    .partial
                    .children
                    .iter()
                    .filter(|c| c.detached)
                    .map(|c| c.key.as_str())
                    .collect();
                if !detached.is_empty() {
                    // A detach with no real failed child is upstream's `paused` shape, which this
                    // build cannot represent as a run state — so it is reported as a failure that
                    // names the detached keys rather than as a resumable pause.
                    message.push_str(&format!(
                        " Detached child run(s): {}. This build cannot resume a detached workflow.",
                        detached.join(", ")
                    ));
                }
                message.push_str(&format!(
                    " Partial: {} child run(s), {} trace event(s).",
                    error.partial.children.len(),
                    error.partial.trace.len()
                ));
                // §3.2 failure arm: trace now reaches the composer (with_workflow_children reads it).
                match self
                    .settle_foreground_workflow(
                        serde_json::json!({}),
                        &status,
                        crate::background::RunState::Failed,
                        &run_dir,
                        &run_dir_name,
                        crate::workflows::WorkflowReceiptState::Failed,
                        &error.partial.children,
                        &error.partial.trace,
                        message.clone(),
                    )
                    .await
                {
                    Ok(details) => Err(ToolError::new(message).with_details(details)),
                    Err(e) => Err(ToolError::new(format!("{message} {e}"))),
                }
            }
        }
    }

    /// §3d rules 1-3: build the receipt from `children`, persist it, and fold
    /// `workflowRunId`/`workflowReceipt` onto `base_details` — or, when persistence fails (an
    /// ENVIRONMENT fault), log/omit the receipt rather than failing the workflow, mirroring
    /// upstream's success-arm `try/catch` around `writeWorkflowReceipt`
    /// (`subagent-executor.ts:5728-5730`).
    ///
    /// A malformed child (a bad/duplicate key, a resumable child with no run id) is a DIFFERENT
    /// kind of fault — a real defect in the engine's own output — and is surfaced as an `Err`
    /// rather than swallowed.
    /// The ONE terminal writer for a foreground workflow (WORKFLOW_3 §3c's production caller).
    ///
    /// Order is WORKFLOW_3's: advance the status, build the receipt, persist it, hand the persistence
    /// OUTCOME (path or error) to [`crate::workflows::plan_workflow_settlement`], then write the status
    /// the plan produced. The composer owns the `evidence-persistence-failed` promotion, the
    /// `workflow_children` projection and the `workflow_receipt_path` stamp; this function owns only
    /// the two writes and the details map.
    #[allow(clippy::too_many_arguments)]
    async fn settle_foreground_workflow(
        &self,
        mut base_details: serde_json::Value,
        status: &std::sync::Arc<std::sync::Mutex<crate::background::RunStatus>>,
        terminal: crate::background::RunState,
        run_dir: &Path,
        run_dir_name: &crate::identity::RunDirName,
        receipt_state: crate::workflows::WorkflowReceiptState,
        children: &[crate::workflows::WorkflowScriptChildResult],
        trace: &[crate::workflows::WorkflowScriptTraceEntry],
        summary: String,
    ) -> Result<serde_json::Value, String> {
        // 1. Terminal status FIRST — `plan_workflow_settlement` DERIVES `success` from `status.state`
        //    (`settlement.rs:709`) and never advances it itself. Snapshot out of the shared record so
        //    no std guard is alive across the awaits below.
        let settled = {
            let mut guard = status
                .lock()
                .map_err(|_| "workflow status lock was poisoned".to_string())?;
            // The engine's returned `children` is the COMPLETE settled list — authoritative over
            // whatever `publish_steps` last managed to write.
            guard.steps = crate::workflows::workflow_step_statuses(children);
            guard.current_step = guard.steps.len().checked_sub(1);
            guard.advance_state(terminal).map_err(|e| e.to_string())?;
            guard.clone()
        };

        // 2. Build the receipt. `workflow_children: None` stays — the PLAN fills it
        //    (`settlement.rs:692-701`), which is the whole reason the composer exists.
        let receipt = crate::workflows::build_workflow_receipt(crate::workflows::BuildWorkflowReceipt {
            workflow_run_id: run_dir_name,
            state: receipt_state,
            children,
            host_steps: &[],          // `on_host_step: None` in this build — WORKFLOW_19
            workflow_children: None,
            resource: None,           // `one_use_permit: None` in this build
            terminal_outcome: None,
            created_at: None,
        })
        .map_err(|error| format!("workflow completed but its receipt is invalid: {error}"))?;

        // 3. Persist it, and carry the OUTCOME rather than swallowing it.
        let (receipt_path, receipt_persistence_error) =
            match crate::workflows::write_workflow_receipt(run_dir, &receipt) {
                Ok(path) => (Some(path), None),
                Err(error) => (None, Some(error.to_string())),
            };

        // 4. WORKFLOW_3's composer. Its first production call site.
        let plan = crate::workflows::plan_workflow_settlement(crate::workflows::PlanWorkflowSettlement {
            status: &settled,
            summary,
            trace,
            receipt: Some(receipt),
            receipt_path,
            receipt_persistence_error,
            resolution: None,
            terminal_outcome: None,
            now: None,
            event_metadata: serde_json::Map::new(),
        });

        // 5. The terminal status write — now carrying `workflow_children` and `workflow_receipt_path`,
        //    which `control.rs:305` / `wait.rs:602` / `run_status.rs:367` /
        //    `wait_completions/project.rs:39,184` have been reading for nothing.
        crate::background::atomic::write_atomic_json(&run_dir.join("status.json"), &plan.status)
            .await
            .map_err(|e| format!("workflow terminal status could not be written: {e}"))?;

        if let Some(map) = base_details.as_object_mut() {
            map.insert("workflowRunId".to_string(), serde_json::json!(plan.status.run_id.as_str()));
            if let (Some(receipt), Some(path)) = (&plan.receipt, &plan.receipt_path) {
                map.insert(
                    "workflowReceipt".to_string(),
                    serde_json::json!({ "path": path, "receipt": receipt }),
                );
            }
            if let Some(children) = &plan.status.workflow_children {
                map.insert("workflowChildren".to_string(), serde_json::json!(children));
            }
            // Always empty until WORKFLOW_15 records per-child resumability
            // (`workflow_recovery_actions` filters on `entry.resume.is_resumable()`,
            // `settlement.rs:413-432`). Surfaced anyway so the wiring is provably correct now and
            // starts reporting the moment WORKFLOW_15 lands.
            if !plan.recovery.is_empty() {
                map.insert("recovery".to_string(), serde_json::json!(plan.recovery));
            }
        }
        Ok(base_details)
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
    ) -> Result<ToolResult, ToolError> {
        match action {
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
            "status" | "interrupt" | "stop" | "dismiss" | "resume" | "steer" | "append-step" => {
                self.route_control_action(action, p, cwd).await
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
            // `state_enabled` is `false`: this build grants no mission state, and the flag exists
            // precisely because `state` is only a legal global when the run has a mission.
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
                    content: vec![cyrup_core::Content::text(render_validation_findings(&report))],
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
        // →`scheduleCreate` once SUBA-016 lands). Every other control verb is ungated upstream and
        // must stay ungated here, which is why the mapping returns `None` for them rather than
        // this arm defaulting to "gated".
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
            // SUBA-038 residual 3: this arm is defensively unreachable (only the six control verbs
            // reach `route_control_action`), but it listed "status, interrupt, resume, steer,
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
        let group = RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: specs,
            concurrency,
            fail_fast: false,
            worktree: p.worktree.unwrap_or(false),
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
