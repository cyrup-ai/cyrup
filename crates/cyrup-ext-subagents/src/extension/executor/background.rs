//! The background (detached second-hop) run paths: `spawn_background` and
//! `spawn_background_steps`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::background::{RunId, RunMode, RunPaths};
use crate::background::atomic::write_atomic_json;
use crate::background::spawn_detached::spawn_detached_runner;
use crate::error::SubagentError;
use crate::exec::ResolvedAgentPersona;
use crate::fork_context::resolve_effective_context;
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use crate::spawn::depth::resolve_effective_depth;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::resolve_background_storage_roots;
use crate::extension::executor::requests::{BackgroundSingleRequest, BackgroundStepsSpec};
use crate::extension::host::slash_render::plan_step_agent_names;
use crate::extension::tool::task_items::{
    normalize_single_output_override, parse_tool_output_mode, resolve_single_output_path,
    resolve_single_run_output_base_dir, resolve_single_run_session_root,
};

impl SubagentExecutor {

    // ---------------------------------------------------------------------------------------
    // Background dispatch (the tool's `bg: true` shape; genuine second, detached OS-process hop)
    // ---------------------------------------------------------------------------------------

    /// Spawn one subagent task as a detached background run (func-SA §5.4; the tool's `bg: true`
    /// shape). Mints a [`RunId`], eagerly resolves fork-context (R-SA-137's eager whole-batch
    /// rule, degenerate single-task case), writes the one-shot `runner-config.json` handoff file
    /// (R-SA-073), and spawns hop 1 via [`spawn_detached_runner`] — a genuine SECOND, detached OS
    /// process (`cyrup __subagent-runner --config <path>`) that survives this orchestrator
    /// process's own exit (R-SA-070/071, DI-SA-8). Immediately tracks the new run
    /// ([`crate::background::tracker::JobTracker::track`], R-SA-093) and returns without waiting for the run to complete
    /// (R-SA-074).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055, SAFETY-CRITICAL) if this process's own
    /// recursion-depth ceiling is already reached — checked FIRST, before agent discovery,
    /// fork-context resolution, run-directory creation, or the detached hop-1 spawn, so a blocked
    /// call touches none of that setup work and spawns nothing (not even the detached runner
    /// process itself). Otherwise returns [`SubagentError`] if the agent cannot be resolved,
    /// fork-context resolution fails hard, the run directory cannot be created, the one-shot
    /// config cannot be written, or the detached spawn itself fails.
    pub async fn spawn_background(
        &self,
        request: BackgroundSingleRequest<'_>,
    ) -> Result<RunId, SubagentError> {
        let BackgroundSingleRequest {
            cwd,
            agent_name,
            task,
            context,
            model_override,
            agent_scope,
            acceptance,
            control,
            include_progress,
            output,
            output_mode,
            skills,
            share,
            session_dir,
            artifacts,
            timeout_ms,
            structured_output_schema,
            tool_budget,
            turn_budget,
            usage_budget,
        } = request;
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before agent discovery or
        // fork-context resolution below, and therefore also before `spawn_background_steps`' own
        // (correct, but too-late-for-THIS-call-site) independent re-check, since this function
        // itself performs real discovery/fork-context I/O ahead of ever delegating there.
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // R-SA-055: resolve the agent (and therefore validate it exists) before any spawn.
        // T0.1/C13: the SAME resolved definition is projected into the plan-time persona map handed
        // to the runner, so hop 2 dispatches this agent's REAL persona rather than a placeholder.
        let agent = self.resolve_agent(cwd, agent_name, agent_scope)?;
        let mut resolved_persona = crate::exec::resolve_step_agent_config(&agent);
        // SUBA-047 (async half) / pi `params.toolBudget ?? agentConfig.toolBudget`
        // (`runs/background/async-execution.ts:1298`). The persona map IS what hop 2 dispatches
        // from (`ResolvedAgentPersona::to_agent_config`), so folding the caller's budget in here
        // reaches the same `TOOL_BUDGET_ENV` encoder the foreground path does, with no new
        // `RunnerConfig` field and no on-disk config-format change.
        if let Some(budget) = tool_budget.clone() {
            resolved_persona.tool_budget = Some(budget);
        }
        let resolved_agents: BTreeMap<String, ResolvedAgentPersona> =
            BTreeMap::from([(agent_name.to_string(), resolved_persona)]);
        // Fork default-mode (Tier-2): an OMITTED call-site `context` falls back to THIS agent's own
        // `default_context` (pi `resolveAgentDefaultContextPolicy`), an explicit value still wins.
        let effective_context = resolve_effective_context(context, agent.default_context);
        // R-SA-137: eager fork-context resolution before ANY process is spawned for this batch.
        let fork_context = self.resolve_context(cwd, effective_context).await?;

        // SUBA-N03 — the run id is minted HERE, not inside `spawn_background_steps`, because the
        // SINGLE-mode output base directory is run-scoped (`<artifactsDir>/outputs/<runId>`) and a
        // relative `output` must resolve against it BEFORE the spawn call. pi hoists it for exactly
        // this reason: `const id = randomUUID()` (`subagent-executor.ts:3607` @v0.43.0) feeds
        // `resolveSingleRunOutputBaseDir(deps, artifactsDir, id)` at `:2861` and only then reaches
        // `executeAsyncSingle(id, …)`. `RunId::new` is 128 bits of fresh entropy per call and
        // `spawn_background_steps` `mkdir`s the run dir before writing anything into it, so two
        // concurrent background runs cannot share a run-scoped directory.
        let run_id = RunId::new();

        // SUBA-N03 / T6 — pi `subagent-executor.ts:3387-3391`: `enabled: params.artifacts !== false`,
        // so ONLY an explicit `artifacts: false` turns the quadruple off. The same
        // `ArtifactConfig::foreground()` shape the foreground path uses, so an async run leaves the
        // identical four files.
        //
        // Note the artifacts DIRECTORY is resolved either way: pi likewise computes `artifactsDir`
        // unconditionally and only gates what it PASSES (`artifactsDir: artifactConfig.enabled ?
        // artifactsDir : undefined`, `async-execution.ts:964`), because the same directory is also
        // the root of the run-scoped output base dir below — turning artifact FILES off must not
        // move where a relative `output:` lands.
        let art_cfg = crate::artifacts::ArtifactConfig {
            enabled: artifacts != Some(false),
            ..crate::artifacts::ArtifactConfig::foreground()
        };
        // SUBA-048 (async half) — pi resolves `artifactsDir` the same way before handing it to
        // `executeAsyncSingle` (`subagent-executor.ts:1471`, consumed at
        // `async-execution.ts:989`/`:1037`). Same defect as the foreground site: a hard-coded temp
        // root made the `artifactDir` preference, and upstream's `project` default, unreachable.
        let art_dir = crate::artifacts::resolve_artifacts_dir(
            self.host_services().and_then(|s| s.session_file()).as_deref(),
            Some(cwd),
            cwd,
            cfg.artifact_dir_preference(),
        );
        let output_base_dir = resolve_single_run_output_base_dir(&cfg, &art_dir, &run_id);

        // pi `async-execution.ts:905-907` (`normalizeSingleOutputOverride(params.output,
        // agentConfig.output)` → `resolveSingleOutputPath(effectiveOutput, …, params.outputBaseDir)`):
        // the persona's own `output:` is the fallback for an omitted param and the referent of
        // `output: true`. Resolved parent-side here rather than in the detached runner because only
        // this process knows the persona and the configured `singleRunOutputBaseDir`.
        let output_path = resolve_single_output_path(
            normalize_single_output_override(
                output.as_ref(),
                agent
                    .output
                    .as_ref()
                    .and_then(|spec| spec.path.as_deref())
                    .and_then(Path::to_str),
            )
            .as_deref(),
            &output_base_dir,
        );
        // pi `async-execution.ts:908`: `const outputMode = params.outputMode ?? "inline"` — from the
        // PARAM alone; pi never consults the persona's own mode here.
        let effective_output_mode = parse_tool_output_mode(output_mode.as_deref())
            .unwrap_or(crate::discovery::types::OutputMode::Inline);
        // pi `validateFileOnlyOutputMode(outputMode, outputPath, \`Async single run (${agent})\`)`
        // (`async-execution.ts:909-910`, via `single-output.ts:140-145`): `file-only` with no
        // resolvable output path is refused BEFORE any spawn, and on the async path it is refused
        // HERE — the detached runner's own R-SA-025 `validate_file_only_requires_path` would
        // otherwise only surface it as a hop-2 step failure the caller never sees synchronously.
        if effective_output_mode == crate::discovery::types::OutputMode::FileOnly
            && output_path.is_none()
        {
            return Err(SubagentError::OutputPathRequired);
        }

        // pi `sessionDir: sessionRoot ? path.join(sessionRoot, `async-${id}`) : undefined`
        // (`async-execution.ts:966`). cyrup's per-run scoping already lives inside
        // `resolve_single_run_session_root` (the configured-default rung joins `run_id`), and the
        // per-CHILD leaf is pi's own `sessionDirForIndex(0)` → `run-0`, the identical leaf the
        // foreground single path appends — so an async run's child session store is scoped exactly
        // as a foreground one's is, keyed by this run's id.
        let step_session_dir =
            resolve_single_run_session_root(&cfg, session_dir.as_deref(), &run_id)
                .map(|root| root.join("run-0"));

        let step = SingleStepSpec {
            agent: agent_name.to_string(),
            task: task.to_string(),
            cwd: None,
            // pi `executeAsyncSingle` (`async-execution.ts:1290-1295`): `params.modelOverride ??
            // agent.model` reaches the detached runner's step unconditionally — a per-call
            // `model:` override on an async SINGLE run is never dropped just because the run is
            // background rather than foreground.
            model: model_override,
            tools: None,
            extensions: None,
            session_file: fork_context.session_file_path.clone(),
            max_depth_override: None,
            // SUBA-043 / pi `params.outputSchema` (`extension/schemas.ts:351` @v0.43.0). Pinned
            // `None` here until now, which is the async half of the same unreachable-capability
            // defect the foreground `RunOptions` carried: hop 2 already threads this field into the
            // step's `RunOptions`, so only the constructor was dropping it.
            structured_output_schema,
            output: None,
            // SUBA-N03: the resolved output FILE path + mode (pi's runner step `outputPath`/
            // `outputMode`, `async-execution.ts:772,774` @v0.43.0). Previously hardcoded `None` — which is
            // why `route_single` refused `output`/`outputMode` on this branch rather than let hop 2
            // drop them.
            output_path: output_path.map(|p| p.display().to_string()),
            output_mode: Some(effective_output_mode),
            // SUBA-N03: the per-call `skill` override (pi's runner step `skills`,
            // `async-execution.ts:990`), already normalized by `normalize_skill_input` at the tool
            // boundary. `None` still defers to the persona's own `skills:` inside `run_sync`.
            skills,
            // SUBA-N03: this child's own session directory (pi `config.sessionDir` →
            // `--session-dir`), resolved parent-side; see `SingleStepSpec::session_dir`.
            session_dir: step_session_dir,
            reads: None,
            // SUBA-N04: an async SINGLE run's declared acceptance policy rides to the detached
            // runner on its own step, exactly like the `model` override directly above — pi
            // `async-execution.ts:956-962` @v0.34.0. Before this it was hardcoded `None` and the tool
            // surface refused the param outright, because the runner dropped it anyway.
            acceptance,
            context: Some(effective_context),
            agent_scope: None,
        };

        self.spawn_background_steps(
            cwd,
            BackgroundStepsSpec {
                // SUBA-021 — the caller's validated `usageBudget`, carried to hop 2 (pi
                // `spawnRunner({ …, usageBudget: params.usageBudget })`,
                // `runs/background/async-execution.ts:1471`). ONE rung, unlike `turn_budget`
                // below: upstream has no `usageBudget:` frontmatter key and no
                // `subagents.usageBudget` config key at either baseline.
                usage_budget,
                // SUBA-008 — the same three-rung chain the foreground path resolves
                // (`subagent-executor.ts:4928`): caller > this agent's `turnBudget:` frontmatter >
                // `subagents.turnBudget`. Resolved HERE because hop 2 has neither discovery nor a
                // live config to re-derive it from.
                turn_budget: match turn_budget.or(agent.default_turn_budget) {
                    Some(budget) => Some(budget),
                    None => crate::exec::turn_budget::resolve_turn_budget_config(
                        cfg.turn_budget.as_ref(),
                        "turnBudget",
                    )
                    .map_err(SubagentError::Management)?,
                },
                steps: vec![RunnerStep::SingleStep(step)],
                mode: RunMode::Single,
                session_file: fork_context.session_file_path,
                resolved_agents,
                // A single top-level task IS its own `{task}` value; a single run has no dedicated
                // chain scratch dir (`{chain_dir}` → the run cwd).
                original_task: task.to_string(),
                chain_dir: None,
                // SUBA-N05: the same parent-side `resolveControlConfig(deps.config.control,
                // params.control)` fold the foreground path performs (`cfg` was already snapshotted
                // above for the depth guard), carried to hop 2 on `RunnerConfig::control`. pi:
                // `executeAsyncSingle(id, { …, controlConfig, … })`,
                // `subagent-executor.ts:2845,2868` @v0.34.0.
                control: Some(crate::exec::control::resolve_control_config(
                    cfg.control.as_ref(),
                    control.as_ref(),
                )),
                // SUBA-N06: the caller's `includeProgress`, carried verbatim to hop 2 (there is no
                // config-level base to fold it against — pi has none either).
                include_progress,
                // SUBA-N03: the id this call already resolved paths against.
                run_id,
                // SUBA-N03: `timeoutMs`/`maxRuntimeMs` (pi `timeoutMs: data.timeoutMs`,
                // `subagent-executor.ts:3653`). `spawn_background_steps` stamps the absolute
                // `deadline_at_ms` from it at spawn time.
                //
                // SUBA-051 / pi `timeoutMs: a.defaultTimeoutMs ?? DEFAULT_ASYNC_TIMEOUT_MS`
                // (`runs/background/async-execution.ts:782` @v0.47.1, constant at `:131` =
                // `30 * 60 * 1000`). Landed in `635c1bd` ("fix: add default async child timeouts",
                // fixes #978), released v0.47.0.
                //
                // Upstream's split is deliberate and reproduced here: the default is applied when
                // building an async CHILD step, and the composite PARENT (`chain`/`tasks`/graph
                // roots) is left unbounded. This function IS the async SINGLE path — one child,
                // one run — so the default belongs here and NOT in `spawn_background_steps`, which
                // the composite entry points also route through. The stopping machinery already
                // exists (SUBA-S03); only the default was missing, so every async fan-out was
                // unbounded by default exactly as it was before that fix.
                //
                // Precedence: an explicit caller `timeoutMs`/`maxRuntimeMs` wins, then the agent's
                // own frontmatter `timeoutMs` (already folded into `timeout_ms` upstream of this
                // call by `applySingleAgentLaunchDefaults`, pi's `a.defaultTimeoutMs`), then this
                // constant.
                timeout_ms: Some(
                    timeout_ms.unwrap_or(crate::background::DEFAULT_ASYNC_CHILD_TIMEOUT_MS),
                ),
                // SUBA-N03: `share` (pi `share: shareEnabled`, `async-execution.ts:965`).
                share,
                // SUBA-N03: pi's `artifactsDir: artifactConfig.enabled ? artifactsDir : undefined`
                // (`async-execution.ts:964`) — an explicit `artifacts: false` reaches hop 2 as BOTH
                // an absent dir and a disabled config, matching pi's own two-term runner gate.
                artifacts_dir: art_cfg.enabled.then(|| art_dir.clone()),
                artifact_config: art_cfg,
            },
        )
        .await
    }

    /// Spawn an ARBITRARY already-resolved step list (`/chain`, `/parallel`, `/run-chain`'s `--bg`
    /// shape, R-SA-129/130) as a detached background run — the general form [`crate::extension::SubagentExecutor::spawn_background`]
    /// itself is a thin single-step wrapper around. Mints a [`RunId`], writes the one-shot
    /// `runner-config.json` handoff file (R-SA-073), and spawns hop 1 via
    /// [`spawn_detached_runner`] exactly as [`crate::extension::SubagentExecutor::spawn_background`] documents; the caller is
    /// responsible for having already resolved fork-context (R-SA-137's eager whole-batch rule)
    /// and for choosing `session_file` accordingly, since a multi-step chain's fork-context
    /// resolution is a per-call-site concern (a single top-level task fork-resolves once for
    /// itself; a chain fork-resolves once for its own first step) this shared helper does not
    /// itself re-derive.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055, SAFETY-CRITICAL) if this process's own
    /// recursion-depth ceiling is already reached — checked FIRST, before any run-directory
    /// creation or the detached hop-1 spawn, so a blocked call touches none of that setup work and
    /// spawns nothing (not even the detached runner process itself). Otherwise returns
    /// [`SubagentError`] if the run directory cannot be created, the one-shot config cannot be
    /// written, or the detached spawn itself fails.
    pub async fn spawn_background_steps(
        &self,
        cwd: &Path,
        spec: BackgroundStepsSpec,
    ) -> Result<RunId, SubagentError> {
        let BackgroundStepsSpec {
            steps,
            mode,
            session_file,
            resolved_agents,
            original_task,
            chain_dir,
            control,
            include_progress,
            run_id,
            timeout_ms,
            share,
            artifacts_dir,
            artifact_config,
            turn_budget,
            usage_budget,
        } = spec;
        let cfg = self.config_snapshot().await;
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before run-directory creation
        // or spawning the detached hop-1 process — since a background run is exactly as much a
        // "spawn" as a foreground one, and the resulting hop-2 runner process
        // (`background::runner_main::run`) will itself go on to spawn further real children for
        // every step in its own chain, each funneling through `exec::run_sync`'s own independent
        // re-check as defense in depth.
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // SUBA-N03: the run id is the CALLER'S (`BackgroundStepsSpec::run_id`), never minted here.
        // pi hoists it the same way and for the same reason — `const id = randomUUID()` at
        // `subagent-executor.ts:3607` feeds `resolveSingleRunOutputBaseDir(deps, artifactsDir, id)`
        // at `:2861` BEFORE `executeAsyncSingle(id, …)` is called — so a caller that must resolve a
        // run-scoped output path can do so against the very id this run will be keyed by.
        //
        // pi's own deadline arithmetic (`async-execution.ts:924` `deadlineAt = Date.now() +
        // params.timeoutMs`) is done HERE, parent-side, and carried as an absolute epoch stamp: the
        // detached hop-2 process cannot be handed a `std::time::Instant` (opaque, monotonic,
        // process-local), and computing the deadline on the far side would silently refund every
        // millisecond the hop-1 spawn and hop-2 startup consumed.
        let deadline_at_ms = timeout_ms.map(|ms| crate::background::now_epoch_ms().saturating_add(ms));

        // pi `executeAsyncChain`/`executeAsyncSingle` (`async-execution.ts:631-634,890-893` @v0.34.0): a
        // background run started from WITHIN an already-nested run (this process inherited a nested
        // route via its own env, set by ITS OWN parent's spawn) reroutes its storage under that same
        // root's `nested-subagent-runs`/`nested` subtree, rather than becoming an indistinguishable
        // top-level run in the shared per-cwd async/results roots. A top-level (non-nested) run
        // resolves `None` here and keeps the C7 shared-roots derivation exactly as before.
        let inherited_nested_route =
            crate::spawn::nested_events::resolve_inherited_nested_route_from_env(|key| {
                std::env::var(key).ok()
            });
        let nested_address = inherited_nested_route.as_ref().and_then(|_| {
            crate::spawn::nested_events::resolve_nested_parent_address_from_env(|key| {
                std::env::var(key).ok()
            })
        });

        // C7: derive the two sibling roots ONCE from the shared source of truth and create them
        // (ensureAccessibleDir-equivalent), then pass their ABSOLUTE paths through `RunnerConfig`
        // so the detached runner writes its terminal ResultFile into the SAME `results_dir` this
        // orchestrator created and watches — never a re-derived, never-created divergent dir.
        let (async_root, results_dir) =
            resolve_background_storage_roots(cwd, inherited_nested_route.as_ref())?;
        crate::background::ensure_accessible_dir(&async_root)
            .await
            .map_err(SubagentError::Spawn)?;
        crate::background::ensure_accessible_dir(&results_dir)
            .await
            .map_err(SubagentError::Spawn)?;
        let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        crate::background::ensure_accessible_dir(&run_paths.run_dir)
            .await
            .map_err(SubagentError::Spawn)?;

        // Captured before `steps` moves into `runner_config` below — pi's `flatAgents`/`firstAgents`
        // (`async-execution.ts:749-768,794-795` @v0.34.0), needed only for the `subagent.nested.started`
        // event's `agent`/`agents`/`chainStepCount` fields.
        let event_agents = plan_step_agent_names(&steps);
        let event_step_count = i64::try_from(steps.len()).unwrap_or(i64::MAX);
        let event_mode_str = match mode {
            RunMode::Single => "single",
            RunMode::Parallel => "parallel",
            RunMode::Chain => "chain",
        };

        // Read before `cfg.worktree_base_dir` (a non-`Copy` `Option<PathBuf>`) is moved out of
        // `cfg` below by the struct literal — `dynamic_fanout_max_items()` takes `&self` on the
        // whole (by-then-partially-moved) `cfg`, so it must be evaluated first.
        let dynamic_fanout_max_items = cfg.dynamic_fanout_max_items();
        let runner_config = crate::background::runner_main::RunnerConfig {
            // SUBA-021 — the run-level usage budget the orchestrator validated, carried verbatim
            // onto hop 2 (pi `spawnRunner({ …, usageBudget })`, `async-execution.ts:1471`).
            usage_budget,
            // SUBA-008 — the run-level turn budget the orchestrator resolved, carried verbatim.
            turn_budget,
            run_id: run_id.clone(),
            mode,
            steps,
            cwd: cwd.to_path_buf(),
            session_file,
            // SUBA-031 (pi `sessionId: ctx.currentSessionId`, `async-execution.ts:1042`): the
            // launching session, carried into the one-shot config so the detached runner can stamp
            // it onto `status.json` and every session-scoped listing can honour it.
            session_id: self.current_session_id(),
            global_concurrency_limit: cfg.global_concurrency_limit as usize,
            worktree_base_dir: cfg.worktree_base_dir,
            max_subagent_depth: cfg.max_subagent_depth,
            async_root: async_root.clone(),
            results_dir: results_dir.clone(),
            // T0.1/C13: the plan-time persona map the orchestrator resolved (via
            // `resolve_plan_personas` / `exec::resolve_step_agent_config`) travels with the one-shot
            // config so the detached hop-2 runner dispatches each step's REAL persona and never
            // re-discovers or falls back to a placeholder `AgentConfig`.
            resolved_agents,
            // A (pi `originalTask`/`chainDir`): the run-wide `{task}` value + dedicated scratch chain
            // dir, resolved once by the orchestrator and serialized here so the detached runner
            // substitutes the SAME `{task}`/`{chain_dir}` the foreground path does.
            original_task,
            chain_dir,
            // Intercom child-bridge (pi `config.controlIntercomTarget`, `subagent-runner.ts:1823`):
            // this orchestrator's own presence target, resolved once here at plan time and carried
            // into the detached runner (which inherits no useful intercom env), so every step's
            // spawned child activates its `contact_supervisor` bridge addressed at this supervisor.
            // `None` (headless / no live intercom session) leaves each child un-bridged.
            orchestrator_intercom_target: self.orchestrator_intercom_target(),
            // Session-model inheritance (pi `ctx.model`): the live parent session model, resolved
            // once here at plan time and carried into the detached runner (which has no host-services
            // backend of its own), so a step whose persona declares no `model:` inherits the parent's
            // model rather than hard-failing on an empty ladder. `None` (headless / no live session)
            // leaves each inheriting step on its persona's own `model`/`fallback_models`.
            //
            // Resolved through [`SubagentExecutor::remembered_parent_model`] — pi's
            // `parentModel: requestParentModel` on every async hand-off (`subagent-executor.ts:4744,
            // 5168,5220` @v0.43.0) — so a background plan cut while the live `ctx.model` probe is
            // momentarily unavailable still bakes in the model this session has been running on.
            // That matters more here than anywhere else: the detached runner has no host-services
            // backend of its own and cannot re-read the parent later, so whatever is captured at
            // plan time is final for every step of the run.
            inherited_session_model: self.remembered_parent_model(),
            // SUBA-003: the model-scope policy in force at authorization time, baked into the
            // one-shot config so the detached hop-2 runner enforces the SAME policy the foreground
            // path does. Without it, `subagent({..., background: true})` would be an unpoliced way
            // around an enforcing `modelScope`.
            model_scope: Self::resolve_model_scope(cwd)?,
            // Nested-route inheritance (pi `config.nestedRoute`/`config.nestedSelf`,
            // `async-execution.ts:727-731,989-993` @v0.34.0): carried verbatim so the detached runner (were it
            // ever to relay ITS OWN descendants further, a later unit's concern) inherits the SAME
            // root route this orchestrator resolved, never re-reading env itself.
            nested_route: inherited_nested_route.clone(),
            nested_self: nested_address.clone(),
            // C16 (pi `config.chain.dynamicFanout.maxItems`): resolved once here at plan time and
            // carried into the detached runner so a background `DynamicGroup` step whose own
            // `expand.maxItems` is absent falls back to the SAME run-wide cap the foreground path
            // applies (`run_chain_foreground`), rather than always failing materialization.
            dynamic_fanout_max_items,
            // SUBA-N05 (pi `config.controlConfig`, `subagent-runner.ts:1802` @v0.34.0): the
            // live-control thresholds/channels this run was AUTHORIZED with, resolved parent-side
            // by the caller and baked in here. This is the only channel by which a per-call
            // `control` override reaches the detached hop-2 runner — it has no settings access and
            // no orchestrator to ask.
            control,
            // SUBA-N06: R-SA-043 compaction's opt-out, carried to hop 2 for the same reason — the
            // detached runner cannot ask anyone what the caller requested.
            include_progress,
            // SUBA-N03 (pi `spawnRunner({ …, timeoutMs: params.timeoutMs, deadlineAt, share:
            // shareEnabled, sessionDir, artifactsDir, artifactConfig, … })`,
            // `async-execution.ts:960-983` @v0.34.0): the five run-level knobs whose absence from
            // this boundary was the whole reason `route_single` refused them on the async branch.
            // Every one is resolved parent-side (this process has settings + `$HOME` context; the
            // detached runner has neither) and carried verbatim.
            timeout_ms,
            deadline_at_ms,
            share,
            artifacts_dir,
            artifact_config,
        };

        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &runner_config)
            .await
            .map_err(SubagentError::Spawn)?;

        let pid = spawn_detached_runner(
            &cfg_path,
            &run_paths.runner_stdout_log,
            &run_paths.runner_stderr_log,
        )?;

        // pi `executeAsyncChain`/`executeAsyncSingle` (`async-execution.ts:1198-1565` @v0.43.0): once
        // hop 1's pid is CONFIRMED (never before — an unconfirmed spawn must not appear in the root's
        // nested registry at all), relay a `subagent.nested.started` event into the inherited route's
        // sink so the grandparent's `project_nested_events` projection can see this run without ever
        // having spawned it directly. Best-effort: a write failure is logged, never fatal to the
        // (already fully spawned) background run itself.
        if let (Some(route), Some(address)) = (&inherited_nested_route, &nested_address) {
            let now = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0),
            )
            .unwrap_or(i64::MAX);
            let child = crate::spawn::nested_events::NestedRunSummary {
                id: run_id.as_str().to_string(),
                parent_run_id: address.parent_run_id.clone(),
                parent_step_index: address.parent_step_index,
                parent_agent: None,
                depth: address.depth,
                path: address.path.clone(),
                async_dir: Some(run_paths.run_dir.to_string_lossy().into_owned()),
                pid: Some(i64::from(pid)),
                session_id: None,
                session_file: None,
                intercom_target: None,
                owner_intercom_target: self.orchestrator_intercom_target(),
                // No per-step intercom-target concept is computed at this generic multi-step entry
                // point (pi's own `childIntercomTargets?.[0]`, resolved per named step) — left absent
                // rather than guessed.
                leaf_intercom_target: None,
                owner_state: Some("live".to_string()),
                control_inbox: None,
                capability_token: None,
                mode: Some(event_mode_str.to_string()),
                state: "running".to_string(),
                agent: event_agents.first().cloned(),
                agents: Some(event_agents.clone()),
                current_step: None,
                chain_step_count: Some(event_step_count),
                activity_state: None,
                last_activity_at: None,
                current_tool: None,
                current_tool_started_at: None,
                current_path: None,
                turn_count: None,
                tool_count: None,
                total_tokens: None,
                total_cost: None,
                started_at: Some(now),
                ended_at: None,
                last_update: Some(now),
                error: None,
                steps: None,
                children: None,
            };
            if let Err(err) = crate::spawn::nested_events::write_nested_event(
                route,
                &crate::spawn::nested_events::NestedEventInput {
                    event_type: "subagent.nested.started".to_string(),
                    ts: now,
                    parent_run_id: address.parent_run_id.clone(),
                    parent_step_index: address.parent_step_index,
                    child,
                },
            ) {
                tracing::warn!(error = %err, "failed to emit nested async start event");
            }
        }

        self.tracker
            .track(run_id.clone(), run_paths, Some(std::time::SystemTime::now()))
            .await;

        Ok(run_id)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::discovery::types::AgentReadScope;
    use crate::fork_context::ContextMode;
    use cyrup_core::ModelId;
    use std::sync::Arc;

    /// SUBA-N03, the load-bearing half: the six formerly-refused SINGLE-mode overrides are not
    /// merely ACCEPTED on the async path, they genuinely reach the detached hop-2 runner.
    ///
    /// "No longer refused" and "honoured" are different claims, and only the second one matters —
    /// an accepted-and-silently-dropped param is the exact defect SUBA-041 exists to prevent, and
    /// is strictly worse than the refusal this unit removed. Asserted at the `runner-config.json`
    /// filesystem boundary, which IS the entire hop-1 -> hop-2 contract (R-SA-073): whatever is in
    /// that file is what the detached process will do, and nothing else crosses.
    ///
    /// Upstream equivalents, all @v0.34.0: `executeAsyncSingle` receives `skills`, `output`,
    /// `outputMode`, `outputBaseDir`, `shareEnabled`, `sessionRoot`, `artifactsDir` and
    /// `artifactConfig` (`runs/foreground/subagent-executor.ts:2845-2874`) and forwards them into
    /// `spawnRunner` (`runs/background/async-execution.ts:414-559`).
    #[tokio::test]
    async fn a_background_single_run_honours_the_nine_single_mode_overrides() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let session_root = dir.path().join("sessions");

        let run_id = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: Some(true),
                output: Some(serde_json::json!("report.md")),
                output_mode: Some("file-only".to_string()),
                // `normalize_skill_input`'s output shape, exactly as `route_single` hands it over.
                skills: Some(vec!["rust".to_string()]),
                share: Some(true),
                session_dir: Some(session_root.display().to_string()),
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(dir.path());
        let run_paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        let raw = std::fs::read_to_string(run_paths.run_dir.join("runner-config.json"))
            .expect("spawn_background must have written runner-config.json before spawning hop 1");
        let cfg: crate::background::runner_main::RunnerConfig =
            serde_json::from_str(&raw).expect("runner-config.json must deserialize");
        let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
            panic!("a single-agent background run must produce exactly one SingleStep");
        };

        // `output` — resolved to an ABSOLUTE path parent-side against the run-scoped output base
        // dir (pi `resolveSingleRunOutputBaseDir` -> `<artifactsDir>/outputs/<runId>`), never left
        // relative for the detached runner to resolve against some other cwd.
        let output_path = step
            .output_path
            .as_deref()
            .expect("the `output` override must reach the step, not be dropped");
        assert!(
            std::path::Path::new(output_path).is_absolute(),
            "a relative `output` must be resolved parent-side against the run-scoped base dir, \
             so a bare `report.md` never lands in the user's repo: {output_path}"
        );
        assert!(
            output_path.ends_with("report.md"),
            "the resolved path must still name the requested file: {output_path}"
        );
        assert!(
            output_path.contains(run_id.as_str()),
            "the output base dir is RUN-SCOPED (`<artifactsDir>/outputs/<runId>`), so two \
             concurrent background runs cannot write over each other: {output_path}"
        );

        // `outputMode`
        assert_eq!(
            step.output_mode,
            Some(crate::discovery::types::OutputMode::FileOnly),
            "the `outputMode` override must reach the step"
        );
        // `skill`
        assert_eq!(
            step.skills.as_deref(),
            Some(["rust".to_string()].as_slice()),
            "the `skill` override must reach the step"
        );
        // `sessionDir` — expanded/absolutized parent-side and scoped to this child (`run-0`, pi's
        // own `sessionDirForIndex(0)` leaf).
        let step_session_dir = step
            .session_dir
            .as_deref()
            .expect("the `sessionDir` override must reach the step");
        assert_eq!(
            step_session_dir,
            session_root.join("run-0"),
            "an explicit sessionDir becomes the ROOT verbatim and the child gets pi's `run-0` leaf"
        );
        // `share` — run-level.
        assert_eq!(cfg.share, Some(true), "the `share` override must reach the runner config");
        // `artifacts` omitted => enabled (pi's `enabled: params.artifacts !== false`).
        assert!(
            cfg.artifacts_dir.is_some(),
            "an omitted `artifacts` param leaves the quadruple ON, matching pi's `!== false`"
        );
        assert!(cfg.artifact_config.enabled);
        // SUBA-N06's flag, re-asserted here so all nine live in one place.
        assert_eq!(cfg.include_progress, Some(true));

        // The on-disk JSON is the actual contract — hop 2 reads this file, not a Rust value.
        let json: serde_json::Value =
            serde_json::from_str(&raw).expect("runner-config.json must be valid JSON");
        assert_eq!(
            json["steps"][0]["skills"],
            serde_json::json!(["rust"]),
            "the step's skills must serialize under pi's camelCase step shape: {raw}"
        );
        assert!(
            json["share"].as_bool() == Some(true),
            "`share` must serialize onto the runner config: {raw}"
        );
    }

    /// SUBA-047's async half, at the hop-1 → hop-2 filesystem contract: the caller's budget must
    /// arrive on the RESOLVED PERSONA the detached runner dispatches from, because that is what
    /// `ResolvedAgentPersona::to_agent_config` feeds to the `TOOL_BUDGET_ENV` encoder. pi collapses
    /// the same two rungs into one value — `params.toolBudget ?? agentConfig.toolBudget`
    /// (`runs/background/async-execution.ts:1298`).
    #[tokio::test]
    async fn a_background_single_run_carries_the_callers_tool_budget_onto_the_resolved_persona() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");

        let budget = crate::exec::tool_budget::validate_tool_budget_config(
            Some(&serde_json::json!({ "hard": 3, "soft": 1 })),
            "toolBudget",
        )
        .expect("valid budget")
        .expect("Some");

        let run_id = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: Some(serde_json::json!({ "type": "object" })),
                tool_budget: Some(budget.clone()),
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                skills: None,
                share: None,
                artifacts: None,
                session_dir: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(dir.path());
        let run_paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        let raw = std::fs::read_to_string(run_paths.run_dir.join("runner-config.json"))
            .expect("runner-config.json must exist");
        let cfg: crate::background::runner_main::RunnerConfig =
            serde_json::from_str(&raw).expect("runner-config.json must deserialize");

        let persona = cfg
            .resolved_agents
            .get("worker")
            .expect("the resolved persona map must carry the dispatched agent");
        assert_eq!(
            persona.tool_budget.as_ref(),
            Some(&budget),
            "the caller's toolBudget must reach hop 2 on the persona; runner config was {raw}"
        );

        // SUBA-043's async half, asserted on the same config so the two land together: the step's
        // `structuredOutputSchema` was pinned `None` at this constructor.
        let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
            panic!("a single-agent background run must produce exactly one SingleStep");
        };
        assert_eq!(
            step.structured_output_schema,
            Some(serde_json::json!({ "type": "object" })),
            "the top-level outputSchema must reach the async step; runner config was {raw}"
        );
    }

    /// SUBA-N03: `artifacts: false` reaches hop 2 as BOTH an absent artifacts dir and a disabled
    /// config — pi's own two-term gate (`artifactsDir: artifactConfig.enabled ? artifactsDir :
    /// undefined`, `runs/background/async-execution.ts:964`, read back by the runner as
    /// `if (ctx.artifactsDir && ctx.artifactConfig?.enabled !== false)`,
    /// `runs/background/subagent-runner.ts:1192`).
    ///
    /// Separate from the sibling above because it asserts the NEGATIVE configuration, and because
    /// the interesting property is that turning artifact FILES off must not move where a relative
    /// `output:` lands — the artifacts dir is also the root of the run-scoped output base dir, so a
    /// naive "skip resolving artifactsDir when disabled" would silently relocate the output file.
    #[tokio::test]
    async fn a_background_single_run_honours_artifacts_false_without_moving_the_output_path() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");

        let mut resolved: Vec<(Option<bool>, String, bool)> = Vec::new();
        for artifacts in [None, Some(true), Some(false)] {
            let run_id = executor
                .spawn_background(BackgroundSingleRequest {
                    // SUBA-021: unbudgeted on this path (see the field doc).
                    usage_budget: None,
                    turn_budget: None,
                    structured_output_schema: None,
                    tool_budget: None,
                    cwd: dir.path(),
                    agent_name: "worker",
                    task: "do something",
                    context: Some(ContextMode::Fresh),
                    model_override: None,
                    agent_scope: AgentReadScope::Both,
                    acceptance: None,
                    control: None,
                    include_progress: None,
                    output: Some(serde_json::json!("report.md")),
                    output_mode: None,
                    skills: None,
                    share: None,
                    session_dir: None,
                    artifacts,
                    timeout_ms: None,
                })
                .await
                .expect("spawn_background should succeed for a resolvable builtin agent");

            let crate::background::RunArtifactRoots { async_root, results_dir } =
                crate::background::run_artifact_roots(dir.path());
            let run_paths =
                crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
            let raw = std::fs::read_to_string(run_paths.run_dir.join("runner-config.json"))
                .expect("runner-config.json must exist");
            let cfg: crate::background::runner_main::RunnerConfig =
                serde_json::from_str(&raw).expect("runner-config.json must deserialize");
            let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
                panic!("expected one SingleStep");
            };
            let output = step.output_path.clone().expect("output path must be resolved");
            // Strip the run-scoped leaf so the three runs' paths are comparable.
            let shape = output.replace(run_id.as_str(), "<runId>");
            resolved.push((artifacts, shape, cfg.artifacts_dir.is_some() && cfg.artifact_config.enabled));
        }

        assert!(
            resolved[0].2,
            "an OMITTED `artifacts` leaves the quadruple on (pi `params.artifacts !== false`)"
        );
        assert!(resolved[1].2, "`artifacts: true` leaves it on");
        assert!(
            !resolved[2].2,
            "`artifacts: false` must reach hop 2 as a disabled quadruple, not be dropped"
        );
        assert_eq!(
            resolved[0].1, resolved[2].1,
            "turning artifact FILES off must not relocate the resolved output path — the \
             artifacts dir is also the root of the run-scoped output base dir"
        );
    }

    /// SUBA-N03: `timeoutMs`/`maxRuntimeMs` on an async SINGLE run arms a REAL deadline on hop 2.
    ///
    /// The refusal this replaces claimed to mirror "pi's own precedent of erroring on timeoutMs +
    /// async (`subagent-executor.ts:3022`)". That precedent does not exist: at v0.34.0 `:3015-3030`
    /// is foreground intercom-receipt construction, and `git grep` over the whole of v0.34.0 `src/`
    /// finds no timeout-vs-async refusal. Upstream states the opposite in its own schema
    /// (`extension/schemas.ts:265-266`: "foreground and async/background runs") and implements it
    /// (`runs/background/async-execution.ts:924` `deadlineAt = Date.now() + params.timeoutMs`,
    /// `:982-983` passed to `spawnRunner`, armed as a live timer at
    /// `runs/background/subagent-runner.ts:2078-2081`).
    ///
    /// The deadline is stamped as ABSOLUTE epoch milliseconds because it crosses a process
    /// boundary in a JSON file — a `std::time::Instant` is opaque, monotonic and meaningless in
    /// another process — so this asserts the arithmetic, not just the presence of a field.
    #[tokio::test]
    async fn a_background_single_run_carries_the_timeout_and_deadline_into_the_runner_config() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");

        let before = crate::background::now_epoch_ms();
        let run_id = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: Some(60_000),
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");
        let after = crate::background::now_epoch_ms();

        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(dir.path());
        let run_paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        let raw = std::fs::read_to_string(run_paths.run_dir.join("runner-config.json"))
            .expect("runner-config.json must exist");
        let cfg: crate::background::runner_main::RunnerConfig =
            serde_json::from_str(&raw).expect("runner-config.json must deserialize");

        assert_eq!(
            cfg.timeout_ms,
            Some(60_000),
            "the NOMINAL budget must reach hop 2 — it is what the timed-out message renders"
        );
        let deadline = cfg
            .deadline_at_ms
            .expect("a run carrying a timeout must also carry an absolute deadline");
        assert!(
            deadline >= before + 60_000 && deadline <= after + 60_000,
            "the deadline must be stamped as `now + timeoutMs` in absolute epoch ms (pi \
             `deadlineAt = Date.now() + params.timeoutMs`); got {deadline}, expected within \
             [{}, {}]",
            before + 60_000,
            after + 60_000
        );

        // SUBA-051: a run with NO caller timeout now carries pi's DEFAULT async CHILD budget
        // (`DEFAULT_ASYNC_TIMEOUT_MS = 30 * 60 * 1000`, `async-execution.ts:131,782` @v0.47.1),
        // not `None`. This assertion used to pin `None` on both fields — which was pinning the
        // defect: every async fan-out was unbounded by default, so a wedged child burned tokens
        // and CPU until a human noticed and issued `interrupt`.
        let untimed = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed");
        let untimed_paths =
            crate::background::RunPaths::for_run(&async_root, &results_dir, &untimed);
        let untimed_cfg: crate::background::runner_main::RunnerConfig = serde_json::from_str(
            &std::fs::read_to_string(untimed_paths.run_dir.join("runner-config.json"))
                .expect("runner-config.json must exist"),
        )
        .expect("runner-config.json must deserialize");
        assert_eq!(
            untimed_cfg.timeout_ms,
            Some(crate::background::DEFAULT_ASYNC_CHILD_TIMEOUT_MS),
            "an async CHILD with no explicit timeout must be bounded at pi's 30-minute default"
        );
        let default_deadline = untimed_cfg
            .deadline_at_ms
            .expect("the default budget must also be stamped as an absolute deadline");
        assert!(
            default_deadline >= before + crate::background::DEFAULT_ASYNC_CHILD_TIMEOUT_MS,
            "the default deadline is `now + 30min` in absolute epoch ms; got {default_deadline}"
        );
    }

    /// SUBA-N03, the adversarial question a prior review raised: hoisting the `RunId` out of
    /// `spawn_background_steps` and into `spawn_background` means the caller now owns run identity.
    /// Can two CONCURRENT background runs collide on their run-scoped output directory?
    ///
    /// Constructed rather than reasoned about: two `spawn_background` calls are driven CONCURRENTLY
    /// against the same cwd (so they share an artifacts root and would collide if the run-scoping
    /// were not real), and their resolved output paths must differ.
    #[tokio::test]
    async fn two_concurrent_background_runs_get_distinct_run_scoped_output_dirs() {
        let executor = Arc::new(SubagentExecutor::new());
        let dir = tempfile::tempdir().expect("tempdir");

        let request = |exec: Arc<SubagentExecutor>, root: std::path::PathBuf| async move {
            exec.spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: &root,
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: Some(serde_json::json!("report.md")),
                output_mode: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed")
        };

        let (a, b) = tokio::join!(
            request(Arc::clone(&executor), dir.path().to_path_buf()),
            request(Arc::clone(&executor), dir.path().to_path_buf()),
        );
        assert_ne!(a.as_str(), b.as_str(), "two runs must never share a run id");

        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(dir.path());
        let read_output = |run: &RunId| {
            let paths = crate::background::RunPaths::for_run(&async_root, &results_dir, run);
            let cfg: crate::background::runner_main::RunnerConfig = serde_json::from_str(
                &std::fs::read_to_string(paths.run_dir.join("runner-config.json"))
                    .expect("runner-config.json must exist"),
            )
            .expect("runner-config.json must deserialize");
            let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
                panic!("expected one SingleStep");
            };
            step.output_path.clone().expect("output path must be resolved")
        };
        let (out_a, out_b) = (read_output(&a), read_output(&b));
        assert_ne!(
            out_a, out_b,
            "two concurrent runs writing the same relative `output` must resolve to DISTINCT \
             run-scoped paths, or one silently overwrites the other: {out_a} vs {out_b}"
        );
        // And the run directories themselves — created by `ensure_accessible_dir` before either
        // config was written — must both exist, which is where a genuine id collision would have
        // surfaced as an error rather than a silent share.
        for run in [&a, &b] {
            let paths = crate::background::RunPaths::for_run(&async_root, &results_dir, run);
            assert!(paths.run_dir.exists(), "each run's directory must be created before its write");
        }
    }

    /// SUBA-N03, the second adversarial question a prior review raised: does an
    /// [`crate::exec::acceptance::AcceptanceContract`] survive the crossing to the separate OS
    /// process INTACT — every variant, the `verify[]` command strings, and the required level?
    /// Silent degradation to a weaker level is the same bug class SUBA-N04 fixed, so it is
    /// constructed here rather than reasoned about.
    ///
    /// The mechanism under test: nothing serializes an `AcceptanceContract` at all. SUBA-N04
    /// carries the RAW wire policy on the step (pi does the same — `explicit: params.acceptance`,
    /// `runs/background/async-execution.ts:1349-1356` @v0.43.0) and hop 2 lowers it with the SAME
    /// [`crate::exec::acceptance::lower_acceptance_input`] the foreground `route_single` uses. So
    /// the property that must hold is: lowering the policy AFTER a JSON round-trip through
    /// `runner-config.json` yields a contract byte-identical to lowering it parent-side.
    ///
    /// Every input shape upstream accepts is exercised: the `false` shorthand, each of the five
    /// level strings, `"auto"` (which infers rather than pinning a level), and the full object form
    /// carrying multiple `verify[]` commands alongside an explicit level.
    #[tokio::test]
    async fn an_acceptance_contract_survives_the_hop_2_json_boundary_at_full_strength() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");

        // G78 — the bare `"none"`, `"verified"` and `"reviewed"` strings this list used to carry
        // are no longer valid policies at pi-subagents v0.43.0 (`acceptance.ts:180-185`), so their
        // still-valid object equivalents take their place and the coverage of the hop-2 boundary
        // is unchanged in breadth: a disable, three plain levels, an object with verify commands,
        // and an object with a review gate.
        let policies = [
            serde_json::json!(false),
            serde_json::json!("attested"),
            serde_json::json!("checked"),
            serde_json::json!("auto"),
            serde_json::json!({ "level": "none", "reason": "prototype spike" }),
            serde_json::json!({
                "level": "verified",
                "verify": [
                    { "id": "unit", "command": "cargo test --workspace" },
                    { "id": "lint", "command": "cargo clippy -- -D warnings" },
                ],
            }),
            serde_json::json!({
                "level": "checked",
                "review": { "agent": "reviewer", "required": true },
            }),
        ];

        for policy in &policies {
            // What the FOREGROUND path would have produced for this same policy.
            let expected = crate::exec::acceptance::lower_acceptance_input(policy)
                .expect("every policy here is valid");

            let run_id = executor
                .spawn_background(BackgroundSingleRequest {
                    // SUBA-021: unbudgeted on this path (see the field doc).
                    usage_budget: None,
                    turn_budget: None,
                    structured_output_schema: None,
                    tool_budget: None,
                    cwd: dir.path(),
                    agent_name: "worker",
                    task: "do something",
                    context: Some(ContextMode::Fresh),
                    model_override: None,
                    agent_scope: AgentReadScope::Both,
                    acceptance: Some(policy.clone()),
                    control: None,
                    include_progress: None,
                    output: None,
                    output_mode: None,
                    skills: None,
                    share: None,
                    session_dir: None,
                    artifacts: None,
                    timeout_ms: None,
                })
                .await
                .expect("spawn_background should succeed");

            let crate::background::RunArtifactRoots { async_root, results_dir } =
                crate::background::run_artifact_roots(dir.path());
            let run_paths =
                crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
            // Read back through the REAL file the detached process reads — not a Rust value handed
            // across a function boundary — so the JSON encoding itself is under test.
            let cfg: crate::background::runner_main::RunnerConfig = serde_json::from_str(
                &std::fs::read_to_string(run_paths.run_dir.join("runner-config.json"))
                    .expect("runner-config.json must exist"),
            )
            .expect("runner-config.json must deserialize");
            let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
                panic!("expected one SingleStep");
            };
            let carried = step
                .acceptance
                .as_ref()
                .expect("the raw policy must survive the boundary");

            // Hop 2's own lowering, on the value that actually crossed.
            let lowered = crate::exec::acceptance::lower_acceptance_input(carried)
                .expect("the round-tripped policy must still lower cleanly");

            assert_eq!(
                lowered, expected,
                "the contract hop 2 lowers must equal the one the foreground path lowers, for \
                 policy {policy}"
            );
            // Stated explicitly rather than left implicit in the struct equality above, because a
            // silent WEAKENING of exactly these three is the failure mode this test exists for.
            match (&lowered, &expected) {
                (Some(l), Some(e)) => {
                    assert_eq!(l.required_level, e.required_level, "required_level for {policy}");
                    assert_eq!(l.verify, e.verify, "verify[] commands for {policy}");
                    assert!(l.explicit, "an explicitly declared policy stays explicit: {policy}");
                }
                (None, None) => {
                    // `"auto"` lowers to `None` — "infer heuristically", which is NOT a weaker
                    // explicit level; `run_sync` applies R-SA-023's default from it.
                    assert_eq!(policy, &serde_json::json!("auto"));
                }
                _ => panic!("lowering disagreed about presence for {policy}"),
            }
        }
    }

    /// pi `executeAsyncSingle` (`async-execution.ts:1290-1295`): `params.modelOverride ?? agent.model`
    /// reaches the detached runner's step for an async SINGLE run regardless of whether that run is
    /// foreground or background. Before this fix, [`SubagentExecutor::spawn_background`] hardcoded
    /// `model: None` into the `SingleStepSpec` it wrote into `runner-config.json`, silently dropping
    /// any per-call model override the instant a SINGLE run went `bg: true` (it reached the runner
    /// fine on the foreground path, `run_foreground_streaming`'s `model_override`). Proven at the
    /// filesystem boundary: the one-shot `runner-config.json` handoff file this call writes (R-SA-073)
    /// must carry the override on its sole step.
    #[tokio::test]
    async fn spawn_background_single_carries_the_model_override_into_the_runner_config() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: Some(ModelId::from("anthropic/claude-override-test")),
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(dir.path());
        let run_paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        let raw = std::fs::read_to_string(&cfg_path)
            .expect("spawn_background must have written runner-config.json before spawning hop 1");
        let cfg: crate::background::runner_main::RunnerConfig =
            serde_json::from_str(&raw).expect("runner-config.json must deserialize");
        let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
            panic!("a single-agent background run must produce exactly one SingleStep, got: {:?}", cfg.steps[0]);
        };
        assert_eq!(
            step.model.as_ref().map(cyrup_core::ModelId::as_str),
            Some("anthropic/claude-override-test"),
            "the per-call model override must reach the background single run's step, not be \
             silently dropped in favor of the persona's own model"
        );
    }

    /// SUBA-N04, the async SINGLE half: a background run's declared `acceptance` policy reaches the
    /// detached hop-2 runner's step WHOLE, in the object form that carries `verify[]`.
    ///
    /// Upstream honours acceptance on the async path exactly as on the foreground one
    /// (`runs/background/async-execution.ts:1282-1289` resolves `explicit: params.acceptance` with
    /// `async: true`; `:1319` persists it on the steering recovery descriptor). cyrup used to hard-
    /// code `acceptance: None` into the `SingleStepSpec` it wrote into `runner-config.json` — and
    /// then refuse the param at the tool boundary to keep the drop from being silent. Both are gone;
    /// this asserts the replacement at the same filesystem boundary the sibling model-override test
    /// uses, since that file is the entire hop-1 -> hop-2 contract.
    #[tokio::test]
    async fn spawn_background_single_carries_the_acceptance_policy_into_the_runner_config() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = serde_json::json!({
            "level": "verified",
            "verify": [{ "id": "unit", "command": "cargo test" }]
        });
        let run_id = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: Some(policy.clone()),
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(dir.path());
        let run_paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        let raw = std::fs::read_to_string(run_paths.run_dir.join("runner-config.json"))
            .expect("spawn_background must have written runner-config.json before spawning hop 1");
        let cfg: crate::background::runner_main::RunnerConfig =
            serde_json::from_str(&raw).expect("runner-config.json must deserialize");
        let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
            panic!("a single-agent background run must produce exactly one SingleStep");
        };
        assert_eq!(
            step.acceptance.as_ref(),
            Some(&policy),
            "the declared acceptance policy must survive the hop-1 -> hop-2 handoff whole, \
             verify[] commands included"
        );
    }

    /// SUBA-N05, the async SINGLE half: a background run's declared `control` override is RESOLVED
    /// parent-side and reaches the detached hop-2 runner's one-shot config.
    ///
    /// This is the defect the advertised-vs-honoured audit turned up: `control` was parsed at the
    /// tool boundary, was NOT on `route_single`'s foreground-only refusal list, and had nowhere to
    /// go on `BackgroundSingleRequest` — so `subagent({ async: true, control: {...} })` accepted the
    /// param and dropped it silently, which is precisely the failure mode SUBA-041 exists to
    /// prevent. Upstream honours it: `executeAsyncSingle(id, { ..., controlConfig:
    /// resolveControlConfig(deps.config.control, effectiveParams.control), ... })`
    /// (`subagent-executor.ts:2845,2868-2870` @v0.34.0), read back by the runner as
    /// `config.controlConfig ?? DEFAULT_CONTROL_CONFIG` (`subagent-runner.ts:1802`).
    ///
    /// Asserted at the `runner-config.json` filesystem boundary — the entire hop-1 -> hop-2
    /// contract — exactly like its `acceptance` and `model` siblings above. Both halves of the
    /// resolution are checked: the explicitly-overridden field takes the call's value, and an
    /// UNSET field takes the extension-level `subagents.control` block rather than being reset to
    /// the hardcoded default (pi's `resolveControlConfig` is field-by-field, never wholesale).
    #[tokio::test]
    async fn spawn_background_single_resolves_and_carries_control_into_the_runner_config() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.control = Some(crate::registration::ControlConfig {
                // Overridden by the per-call value below.
                needs_attention_after_ms: Some(11_000),
                // NOT overridden — must survive into the resolved config.
                active_notice_after_ms: Some(22_000),
                ..crate::registration::ControlConfig::default()
            });
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: Some(crate::registration::ControlConfig {
                    needs_attention_after_ms: Some(1_234),
                    notify_channels: Some(vec![
                        crate::registration::ControlNotificationChannel::Async,
                    ]),
                    ..crate::registration::ControlConfig::default()
                }),
                include_progress: None,
                output: None,
                output_mode: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(dir.path());
        let run_paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        let raw = std::fs::read_to_string(run_paths.run_dir.join("runner-config.json"))
            .expect("spawn_background must have written runner-config.json before spawning hop 1");
        // The on-disk shape is the contract, so assert against the RAW JSON keys too — hop 2 reads
        // this file, not a Rust value handed across a function boundary.
        let json: serde_json::Value =
            serde_json::from_str(&raw).expect("runner-config.json must be valid JSON");
        assert_eq!(
            json["control"]["needsAttentionAfterMs"],
            serde_json::json!(1_234),
            "the per-call override must win, and must serialize under pi's camelCase key: {raw}"
        );
        assert_eq!(
            json["control"]["activeNoticeAfterMs"],
            serde_json::json!(22_000),
            "a field the call did not override must inherit the extension-level config, not the \
             hardcoded default (resolveControlConfig is field-by-field): {raw}"
        );

        let cfg: crate::background::runner_main::RunnerConfig =
            serde_json::from_str(&raw).expect("runner-config.json must deserialize");
        let control = cfg.control.expect("the resolved control config must be present");
        assert_eq!(control.needs_attention_after_ms, 1_234);
        assert_eq!(control.active_notice_after_ms, 22_000);
        assert_eq!(
            control.notify_channels,
            vec![crate::registration::ControlNotificationChannel::Async],
            "the notify-channel list must survive the handoff, so hop 2 knows which channels this \
             run was authorized to use"
        );
        assert_eq!(
            control.failed_tool_attempts_before_attention,
            crate::exec::control::DEFAULT_FAILED_TOOL_ATTEMPTS_BEFORE_ATTENTION,
            "and an entirely unmentioned field falls through to DEFAULT_CONTROL_CONFIG"
        );
    }

}
