//! The background (detached second-hop) run paths: `spawn_background` and
//! `spawn_background_steps`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::background::atomic::write_atomic_json;
use crate::background::{RunId, RunMode, RunPaths};
use crate::error::SubagentError;
use crate::exec::ResolvedAgentPersona;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::resolve_background_storage_roots;
use crate::extension::executor::requests::{BackgroundSingleRequest, BackgroundStepsSpec};
use crate::extension::host::slash_render::plan_step_agent_names;
use crate::extension::tool::task_items::{
    normalize_single_output_override, parse_tool_output_mode, resolve_effective_output_mode,
    resolve_single_output_path, resolve_single_run_output_base_dir,
    resolve_single_run_session_root,
};
use crate::fork_context::resolve_effective_context;
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use crate::spawn::depth::resolve_effective_depth;

impl SubagentExecutor {
    // ---------------------------------------------------------------------------------------
    // Background dispatch (the tool's `bg: true` shape; genuine second, detached OS-process hop)
    // ---------------------------------------------------------------------------------------

    /// Spawn one subagent task as a detached background run (func-SA §5.4; the tool's `bg: true`
    /// shape). Mints a [`RunId`], eagerly resolves fork-context (R-SA-137's eager whole-batch
    /// rule, degenerate single-task case), writes the one-shot `runner-config.json` handoff file
    /// (R-SA-073), and spawns hop 1 via [`spawn_detached_runner`](crate::background::spawn_detached::spawn_detached_runner) — a genuine SECOND, detached OS
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
            machine,
            machine_cwd,
            output,
            output_mode,
            fast,
            skills,
            share,
            session_dir,
            artifacts,
            timeout_ms,
            structured_output_schema,
            tool_budget,
            thinking,
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
        let agent = self.resolve_agent(cwd, agent_name, agent_scope, &cfg.roots)?;
        // SUBA-100 — pi `executeAsyncSingle` → `buildSeqStep`'s placement block
        // (`async-execution.ts:990-1001` @v0.68.0): `params.machine ?? a.machine`, the runner
        // refusal, then the resolution — all BEFORE the run id or the detached runner exists.
        let placement = crate::placement::resolve::resolve_single_placement(
            crate::placement::resolve::PlacementAgent {
                name: &agent.name,
                machine: agent.machine.as_deref(),
                runner: agent.runner.as_ref(),
            },
            machine.as_deref(),
            machine_cwd.as_deref(),
            &mut crate::placement::resolve::launch_resolver(cwd, &cfg)?,
        )
        .await
        .map_err(SubagentError::Management)?;
        let mut resolved_persona = crate::exec::resolve_step_agent_config(&agent);
        // SUBA-047 (async half) / pi `params.toolBudget ?? agentConfig.toolBudget`
        // (`runs/background/async-execution.ts:1298`). The persona map IS what hop 2 dispatches
        // from (`ResolvedAgentPersona::to_agent_config`), so folding the caller's budget in here
        // reaches the same `TOOL_BUDGET_ENV` encoder the foreground path does, with no new
        // `RunnerConfig` field and no on-disk config-format change.
        if let Some(budget) = tool_budget.clone() {
            resolved_persona.tool_budget = Some(budget);
        }
        // SCOPE_19/A1 (async half of the caller `thinking` rung) — same fold, same reason as
        // `tool_budget` directly above: the persona map IS what hop 2 dispatches from, so stamping
        // the caller's explicit level here reaches `build_step_agent_config` as the agent's own
        // `thinking:` — caller beats persona — with no per-step config-format change. The
        // parent-session rung stays SEPARATE (`RunnerConfig::inherited_session_thinking` below),
        // because it must sit UNDER the persona's own level, not over it.
        if let Some(level) = thinking.clone() {
            resolved_persona.thinking = Some(level);
        }
        let resolved_agents: BTreeMap<String, ResolvedAgentPersona> =
            BTreeMap::from([(agent_name.to_string(), resolved_persona)]);
        // Fork default-mode (Tier-2): an OMITTED call-site `context` takes the defaults ladder (pi
        // `resolveAgentDefaultContextPolicy`), an explicit value still wins.
        //
        // SUBA-079: same three rungs as the foreground path, including the availability test that
        // downgrades an INHERITED `fork` preference to `Fresh` rather than failing the launch.
        // `cfg` is the snapshot this function already took for the depth guard above — one read
        // per launch, matching the foreground path.
        let can_prefer_fork = match context {
            Option::None => self.can_prefer_fork(cwd).await,
            Some(_) => false,
        };
        let effective_context = resolve_effective_context(
            context,
            agent_name,
            agent.default_context,
            crate::fork_context::resolve_default_subagent_context(
                cfg.default_subagent_context.as_ref(),
            )
            .map_err(SubagentError::Management)?,
            can_prefer_fork,
        )?;
        // R-SA-137: eager fork-context resolution before ANY process is spawned for this batch.
        //
        // SUBA-075: `true` is upstream's own `forceThinkingOffForIndex?.(index) ?? true` fallback.
        // The background hand-off carries only a session-file PATH across to hop 2
        // (`RunnerConfig`'s `SingleStepSpec::session_file`), so a resolved `thinking_override` has
        // nowhere to ride even if this path computed one; threading it is the async half of
        // SUBA-075, filed separately. Until then this stays on the conservative side of the gate
        // rather than resolving a ladder whose answer it could not transmit.
        let fork_context = self.resolve_context(cwd, effective_context, true).await?;

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
            self.host_services()
                .and_then(|s| s.session_file())
                .as_deref(),
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
        // pi `async-execution.ts:1836` @v0.68.0: `params.outputMode ?? agentConfig.outputMode ??
        // "inline"`. The comment that stood here said pi "never consults the persona's own mode" —
        // true at v0.43.0 (`:908`), false from v0.57.0 on, and the reason the agent-level mode was
        // dropped on this path (SUBA-096). The `file-only` refusal just below therefore sees the
        // AGENT's mode too, which is the point: an agent declaring `outputMode: file-only` with no
        // resolvable path is refused before spawn, as upstream refuses it.
        let effective_output_mode = resolve_effective_output_mode(
            parse_tool_output_mode(output_mode.as_deref()),
            agent.output.as_ref(),
        );
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
            // SUBA-100 — the resolved placement, carried to hop 2 on the step itself.
            machine: placement,
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
            // SUBA-096 — pi `params.fast ?? agentConfig.fast` (`async-execution.ts:1741`
            // @v0.68.0), resolved here where the persona is, and carried on the step to hop 2.
            fast: fast.or(agent.fast),
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
                // SUBA-073 — pi `resolvePermissionRules(ctx.config?.permissions,
                // agentConfig.permissions)` (`async-execution.ts`): the two-rung merge (no
                // per-call tier — permissions has none), resolved HERE for the same reason
                // `turn_budget` above is: hop 2 has neither discovery nor a live config to
                // re-derive it from.
                permission_rules: crate::exec::permissions::resolve_permission_rules(
                    crate::exec::permissions::validate_permission_config(
                        cfg.permissions.as_ref(),
                        "config.permissions",
                    )
                    .map_err(SubagentError::Management)?
                    .as_ref(),
                    agent.permission_rules.as_ref(),
                ),
                // SCOPE_9: a fresh top-level async run, never a resume — it charges the
                // session's cap in its own right.
                transfer_from: None,
                // VL-S3: a fresh launch, not a revival of a stored session file — no lease.
                revival_lease: None,
                // A fresh launch inherits this process's ceilings through env; only a revive
                // carries an explicit ceiling to re-apply.
                thinking_ceiling: None,
                capability_ceiling: None,
                // …and only a revive carries a stored `modelOrigin`; a fresh launch derives it.
                model_origin: None,
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
    /// [`spawn_detached_runner`](crate::background::spawn_detached::spawn_detached_runner) exactly as [`crate::extension::SubagentExecutor::spawn_background`] documents; the caller is
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
    /// SCOPE_9 — the resolved [`CapacityOptions`](crate::background::active_async_capacity::CapacityOptions)
    /// this process uses for every capacity decision.
    ///
    /// The root comes from `cfg.roots`, never a re-read of the environment
    /// (`background/artifact_roots.rs`'s stated reason: "the optional form put this decision in the
    /// callee, where it could be answered differently from the same decision made two frames up"),
    /// and the abandoned-slot policy is resolved EXACTLY ONCE here so the "absent means default"
    /// rung cannot be applied twice with different answers.
    pub(crate) fn capacity_options(
        cfg: &crate::registration::SubagentExtensionConfig,
        live_workflow_run_ids: std::collections::HashSet<RunId>,
    ) -> crate::background::active_async_capacity::CapacityOptions {
        crate::background::active_async_capacity::CapacityOptions::new(
            crate::background::active_async_capacity_root_in(&cfg.roots),
        )
        .with_live_workflow_run_ids(live_workflow_run_ids)
        .with_abandoned_slot_release(
            crate::background::active_async_capacity::resolve_abandoned_slot_release(
                cfg.capacity
                    .as_ref()
                    .and_then(|capacity| capacity.abandoned_slot_release_after_ms),
            ),
        )
    }

    /// SCOPE_9 — give the slot back on an error path between the claim and `Ok(run_id)`.
    ///
    /// `None` (no cap configured, or no session) is a no-op. A rollback that reports `false` is
    /// logged rather than escalated: the caller is already returning an error, and the only shapes
    /// that produce `false` — the slot was reclaimed under us, or it has already been bound to a
    /// runner — both mean the slot is no longer this failed spawn's to release.
    async fn rollback_capacity(
        capacity: Option<&mut crate::background::active_async_capacity::ActiveAsyncCapacityHandle>,
        run_id: &RunId,
    ) {
        if let Some(handle) = capacity
            && !handle.rollback().await
        {
            tracing::warn!(
                run_id = %run_id,
                "active-async capacity slot could not be rolled back after a failed spawn; \
                 reconciliation will reclaim it once the run's status is readable"
            );
        }
    }

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
            permission_rules,
            usage_budget,
            transfer_from,
            revival_lease,
            thinking_ceiling: requested_thinking_ceiling,
            capability_ceiling: requested_capability_ceiling,
            model_origin: stored_model_origin,
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
        // Launch-time refusal of `fast` for a foreign runner — pi `async-execution.ts:1012` (every
        // step of `buildAsyncRunnerSteps`) and `:1756` (`executeAsyncSingle`) @v0.68.0, both BEFORE
        // the async dir, the status file or the runner exist. Checked here, ahead of every
        // directory this function creates, so the tool call itself errors and no run is left
        // behind; the spawn-time refusal in `exec::external_cli` stays as the backstop for the
        // foreground path.
        refuse_external_runner_fast(&steps, &resolved_agents)?;

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
        let deadline_at_ms = timeout_ms.map(|ms| {
            u64::try_from(crate::time::now_epoch_millis())
                .unwrap_or(0)
                .saturating_add(ms)
        });

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
        // `cfg.roots` IS honoured here even though this is a detached spawn: the two roots
        // are resolved in THIS process and cross to the runner as absolute paths in
        // `RunnerConfig::async_root`/`results_dir`, so the child never re-derives them from its own
        // environment. Contrast `cfg.spawn_command`, which cannot cross and is inert here.
        let (async_root, results_dir) =
            resolve_background_storage_roots(cwd, inherited_nested_route.as_ref(), &cfg.roots)?;
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

        // Hoisted out of the `RunnerConfig` literal below: see `model_scope` there.
        let model_scope = Self::resolve_model_scope(cwd, &cfg.roots)?;

        // The two ceilings this LAUNCHING process is bound by, resolved once, parent-side, and
        // BEFORE the capacity claim below for `model_scope`'s reason: every fallible step past the
        // claim must roll the slot back explicitly, and a `?` here cannot. They feed the recovery
        // descriptor (pi `thinkingCeiling`/`capabilityCeiling`, `async-execution.ts:2013,2049`
        // @v0.68.0) and, on a REVIVE, the detached runner's env overlay below (pi
        // `subagent-executor.ts:2151,2183`). Intersected with whatever the caller handed over so a
        // revive can only ever tighten (pi `applySteeringRecoveryAgentConfig`, `async-resume.ts:610`,
        // and the three-way intersection at `:2183`); fail-CLOSED on a malformed inherited value,
        // exactly as `exec::run_sync` is — a bound that vanished is worse than a refused launch.
        let revive_carries_thinking_ceiling = requested_thinking_ceiling.is_some();
        let revive_carries_capability_ceiling = requested_capability_ceiling.is_some();
        let own_thinking_ceiling = crate::exec::thinking_ceiling::inherited_thinking_ceiling()
            .map_err(SubagentError::ThinkingCeilingViolation)?;
        let thinking_ceiling = crate::exec::thinking_ceiling::intersect_thinking_ceilings(&[
            requested_thinking_ceiling.as_deref(),
            own_thinking_ceiling.as_deref(),
        ])
        .map_err(SubagentError::ThinkingCeilingViolation)?;
        // The inherited half is read through the extension's env seam — `env_overrides` over this
        // process — the same layering `SubagentsExtension::env_lookup` gives every other injectable
        // read, so a pinned (or scrubbed) `CYRUP_SUBAGENT_CAPABILITY_CEILING_V1` binds this launch
        // exactly as an inherited one would.
        let own_capability_ceiling =
            crate::exec::capability_ceiling::resolve_current_capability_ceiling_from(
                self.current_session_id().as_deref(),
                &|key| match cfg.env_overrides.get(key) {
                    Some(pinned) => pinned.clone(),
                    None => std::env::var(key).ok(),
                },
            )
            .map_err(SubagentError::CapabilityCeilingViolation)?;
        let capability_ceiling = crate::exec::capability_ceiling::intersect_capability_ceilings(&[
            requested_capability_ceiling,
            own_capability_ceiling,
        ]);

        // =========================================================================================
        // SCOPE_9/SUBTASK4 — the per-SESSION active-async capacity gate
        // =========================================================================================
        //
        // Position is upstream's own (`subagent-executor.ts:6793-6814` @v0.66.0): AFTER the depth
        // guard and after the roots exist (the owner record stores `async_dir = run_paths.run_dir`),
        // BEFORE the `RunnerConfig` is built. Charging here rather than in `spawn_background`
        // covers both entry points at once — that one delegates to this function — so the cap can
        // never be double-charged, and all five callers inherit the gate for free.
        //
        // # The eligibility gate, term by term
        //
        // Upstream gates on `depth === 0 && !inheritedNestedRouteValue &&
        // !effectiveParams.workflowParentRunId`. The first two terms are already bound above. The
        // third has NO cyrup input — `BackgroundStepsSpec` carries no workflow-parent marker, only
        // the FOREGROUND request does — and it is left unthreaded deliberately, because there is no
        // cyrup path it could distinguish: `route_workflow_mode` refuses the async workflow shape
        // outright (`extension/tool/routing.rs:598-599`), so a workflow's children run in the
        // foreground of the workflow shell, and a child that itself backgrounds a subagent does so
        // from a DESCENDANT process, where `depth > 0` and an inherited nested route already
        // exclude it. Threading a field through five call sites to re-express a condition the first
        // two terms already imply would be scope for no behaviour.
        //
        // # No session id means no gate — the run is simply not partitionable
        //
        // `current_session_id()` is an `Option<String>` (headless, unpersisted, or no host services
        // bound) and `SessionId` cannot be built from `None`. Upstream asserts non-null
        // (`state.currentSessionId!`); cyrup, whose headless surfaces are real, SKIPS the gate
        // instead. Refusing every headless async spawn would be a visible behaviour change for a
        // cap that could not be enforced anyway — a run with no session belongs to no pool.
        let capacity_options = Self::capacity_options(&cfg, self.live_workflow_run_ids());
        let capacity_limit =
            crate::background::active_async_capacity::resolve_max_active_async_runs_per_session(
                cfg.max_active_async_runs_per_session,
            );
        let capacity_session =
            crate::identity::SessionId::parse_opt(self.current_session_id().as_deref());
        let top_level_async = depth.current_depth == 0 && inherited_nested_route.is_none();
        // # A resume TRANSFERS, it does not acquire
        //
        // Upstream branches the very same gate on `target.source === "async"`
        // (`subagent-executor.ts:2085-2098` @v0.68.0): a resume of an async run hands the SOURCE
        // run's already-held slot to the new run instead of taking a second one. `transfer_from`
        // is that discriminator, set by `control_resume`'s terminal-revival arm — the only cyrup
        // caller that resumes an async run at all. Without it a revive double-charges the session,
        // and at `max_active_async_runs_per_session = 1` the source run's own retained slot makes
        // the revive refuse ITSELF with the exhausted sentence.
        //
        // `transfer` needs no unconfigured-cap arm of its own: its no-source-slot fall-through IS
        // `acquire` (pi `:513`), which answers `Ok(None)` there — upstream takes the identical
        // route, pool scan included, and a session that never claimed a slot has no pool to list.
        //
        // `acquire` returns `Ok(None)` for an unconfigured cap (pi `:456`), so an install that
        // never set `maxActiveAsyncRunsPerSession` performs zero extra filesystem work here.
        let mut capacity = match capacity_session.as_ref().filter(|_| top_level_async) {
            Some(session) => match transfer_from.as_ref() {
                Some(source_run_id) => crate::background::active_async_capacity::transfer(
                    crate::background::active_async_capacity::TransferInput {
                        session_id: session,
                        limit: capacity_limit,
                        source_run_id,
                        run_id: &run_id,
                        async_dir: &run_paths.run_dir,
                    },
                    &capacity_options,
                )
                .await?,
                None => crate::background::active_async_capacity::acquire(
                    crate::background::active_async_capacity::AcquireInput {
                        session_id: session,
                        limit: capacity_limit,
                        run_id: &run_id,
                        kind:
                            crate::background::active_async_capacity::ActiveAsyncCapacityKind::Runner,
                        async_dir: &run_paths.run_dir,
                    },
                    &capacity_options,
                )
                .await?,
            },
            None => None,
        };

        // Captured before `steps` moves into `runner_config` below — pi's `flatAgents`/`firstAgents`
        // (`async-execution.ts:749-768,794-795` @v0.34.0), needed only for the `subagent.nested.started`
        // event's `agent`/`agents`/`chainStepCount` fields.
        let event_agents = plan_step_agent_names(&steps);
        let event_step_count = i64::try_from(steps.len()).unwrap_or(i64::MAX);
        let event_mode_str = match mode {
            RunMode::Single => "single",
            RunMode::Parallel => "parallel",
            RunMode::Chain => "chain",
            RunMode::Workflow => "workflow",
        };

        // Read before `cfg.worktree_base_dir` (a non-`Copy` `Option<PathBuf>`) is moved out of
        // `cfg` below by the struct literal — `dynamic_fanout_max_items()` takes `&self` on the
        // whole (by-then-partially-moved) `cfg`, so it must be evaluated first.
        let dynamic_fanout_max_items = cfg.dynamic_fanout_max_items();
        // pi `async-execution.ts:707`: ONE fresh v4 uuid per launch, minted in the PARENT before
        // anything is written, because it has to reach four places that must all agree — the
        // runner's config, the process-terminal candidate, the pending proof sidecar, and the
        // runner's own `status.json`. Minting it inside the runner would make the orchestrator's
        // own record of the launch unmatched against the proof the runner later writes.
        let runner_process_instance_id =
            crate::background::process_terminal::RunnerProcessInstanceId::new();
        let runner_config = crate::background::runner_main::RunnerConfig {
            // SUBA-021 — the run-level usage budget the orchestrator validated, carried verbatim
            // onto hop 2 (pi `spawnRunner({ …, usageBudget })`, `async-execution.ts:1471`).
            usage_budget,
            // SUBA-008 — the run-level turn budget the orchestrator resolved, carried verbatim.
            turn_budget,
            // SUBA-073 — the run-level, fully-merged permission policy the orchestrator resolved,
            // carried verbatim.
            permission_rules,
            run_id: run_id.clone(),
            // pi `async-execution.ts:707` `const runnerProcessInstanceId = randomUUID();` — minted
            // HERE, in the parent, before the config is written, and carried into the child at
            // `:710`. This one value is the identity every process-terminal artifact this run
            // produces is keyed on.
            runner_process_instance_id: Some(runner_process_instance_id.clone()),
            // VL-S3 — pi `config.revivalLease` (`subagent-runner.ts:5241`). The ORCHESTRATOR
            // decides that a launch is a revival; the RUNNER is the process that holds the claim,
            // for exactly as long as it runs. Carried verbatim, never derived here: see
            // `BackgroundStepsSpec::revival_lease`.
            revival_lease,
            mode,
            steps,
            cwd: cwd.to_path_buf(),
            session_file,
            // SUBA-031 (pi `sessionId: ctx.currentSessionId`, `async-execution.ts:1042`): the
            // launching session, carried into the one-shot config so the detached runner can stamp
            // it onto `status.json` and every session-scoped listing can honour it.
            session_id: self.current_session_id(),
            // pi `hostAvailableBuiltins` (`async-execution.ts:976,1392,1775`): the LIVE host tool
            // registry, observed HERE at plan time — THIS process holds the `HostServices` handle —
            // and carried into the one-shot config, because the detached runner has no host-services
            // backend and its own registry is not this one. Without it every async/background run
            // launches with the host intersection inert.
            host_available_builtins: crate::exec::tool_surface::host_builtin_tool_names(
                self.host_services().as_deref(),
            ),
            // pi `completionOwnerId` — THIS process's identity, minted once per process
            // (`shared/completion-owner.ts:10-14`). Stamped at spawn so the terminal result
            // records who is entitled to consume it; the detached runner cannot derive this,
            // because its own owner id is not the orchestrator's.
            completion_owner_id: Some(crate::identity::current_completion_owner_id()),
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
            // SCOPE_19/A1: the thinking half of the field above, resolved through the SAME
            // remembered-value seam and final at plan time for the same reason — the detached
            // runner cannot re-read the parent session later. Folded runner-side BELOW each
            // persona's own `thinking:` (`build_step_agent_config`), so an inheriting step reasons
            // at the parent's level and a persona that declares a level keeps it.
            inherited_session_thinking: self.remembered_parent_thinking(),
            // SUBA-003: the model-scope policy in force at authorization time, baked into the
            // one-shot config so the detached hop-2 runner enforces the SAME policy the foreground
            // path does. Without it, `subagent({..., background: true})` would be an unpoliced way
            // around an enforcing `modelScope`. Resolved ABOVE the capacity claim (SCOPE_9): every
            // fallible step between the claim and `Ok(run_id)` must roll the slot back explicitly,
            // and a `?` buried inside this struct literal cannot.
            model_scope,
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

        // pi `async-execution.ts:1993-2055` @v0.68.0: the RESOLVED launch contract, persisted as
        // `recovery-descriptor.json` BEFORE `runner-config.json` and before any process exists, so
        // a failed descriptor leaves no config and no runner — and, like the two fallible steps
        // after it, rolls the capacity slot back. A write failure FAILS THE LAUNCH (`:2053`): a
        // run that cannot be recovered is refused up front rather than discovered at resume time.
        // Built from `runner_config` itself because that literal IS the resolved launch — the same
        // locals pi projects (`recoveryAgentConfig`, `params.*`, `deadlineAt`, `shareEnabled`,
        // `artifactsDir`, `artifactConfig`, `controlConfig`); on a revive the persona map already
        // holds the OVERLAID persona, so the revived run's own descriptor records the contract it
        // was revived under (pi `recoveryAgentConfig`, `:1992`). `None` for every non-single
        // launch: pi writes one only from `executeAsyncSingle`.
        if let Some(descriptor) = crate::background::RecoveryDescriptor::for_single_launch(
            &runner_config,
            crate::background::LaunchInputs {
                thinking_ceiling: thinking_ceiling.as_deref(),
                capability_ceiling: capability_ceiling.as_ref(),
                stored_model_origin,
            },
        ) && let Err(error) = descriptor
            .write(
                &crate::background::RunDir::for_existing(&run_paths.run_dir).recovery_descriptor(),
            )
            .await
        {
            Self::rollback_capacity(capacity.as_mut(), &run_id).await;
            return Err(error.into());
        }

        let cfg_path = run_paths.run_dir.join("runner-config.json");
        // SCOPE_9: first of the two fallible steps between the claim and `Ok(run_id)`. A
        // held-but-never-spawned slot is a PERMANENT leak no reconcile can clear — there is no
        // `status.json` for any verdict to read — so every early return past the claim rolls back
        // explicitly. A `Drop` guard cannot do this: `rollback` is `async`.
        if let Err(error) = write_atomic_json(&cfg_path, &runner_config).await {
            Self::rollback_capacity(capacity.as_mut(), &run_id).await;
            return Err(SubagentError::Spawn(error));
        }

        // Tier 2 for a DETACHED child: this runner is a separate process that re-resolves its own
        // binary and paths from the environment it inherits, so `cfg.spawn_command` is handed to it
        // as the command to exec and `cfg.roots`' child override rides along as `CYRUP_HOME` on ITS
        // `Command` — and ONLY when the roots are a sandbox the child could not derive itself.
        // Neither is set on this process. With both unset this is byte-for-byte the previous
        // `spawn_detached_runner` behaviour.
        let resolved_command = cfg
            .spawn_command
            .clone()
            .unwrap_or_else(crate::spawn::resolve_spawn_command);
        // A REVIVE re-applies the source run's persisted ceilings to its runner through the same
        // two env vars the runner already reads (pi `thinkingCeiling`/`capabilityCeiling` on the
        // revived launch, `subagent-executor.ts:2151,2183` @v0.68.0). Inserted ONLY when the caller
        // carried one: an ordinary launch keeps the inherit-only overlay byte-for-byte, so its
        // runner sees exactly the environment it always did. The values are the intersections
        // computed above the capacity claim — never wider than what THIS process is bound by.
        let mut env_overlay =
            crate::background::parent_anchor::detached_runner_env_overlay_in(&cfg.roots);
        // SUBA-100 — the detached runner reaches a placed step's machine through ITS environment,
        // so the placement keys this extension pins in `env_overrides` (the ssh binary and agent
        // socket, the herdr binary) are forwarded to it; unpinned keys are inherited as ever.
        for key in crate::placement::resolve::PLACEMENT_ENV_KEYS {
            if let Some(Some(value)) = cfg.env_overrides.get(*key) {
                env_overlay.insert((*key).to_string(), value.clone());
            }
        }
        if revive_carries_thinking_ceiling && let Some(level) = &thinking_ceiling {
            env_overlay.insert(
                crate::exec::thinking_ceiling::THINKING_CEILING_ENV.to_string(),
                level.clone(),
            );
        }
        if revive_carries_capability_ceiling
            && let Some(encoded) = crate::exec::capability_ceiling::encode_capability_ceiling(
                capability_ceiling.as_ref(),
            )
        {
            env_overlay.insert(
                crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV.to_string(),
                encoded,
            );
        }
        // pi `initializeProcessTerminal(launchAsyncDir, launchRunId, runnerProcessInstanceId)`
        // (`async-execution.ts:851`), whose own contract is *"Establish ownership before
        // authorizing a runner to start any child session."* (`process-terminal.ts:118`).
        //
        // [CYRUP-DELTA — placement, not behaviour.] Upstream calls it AFTER the spawn because its
        // runner is held at a startup barrier: it blocks on
        // `waitForStartupControl(startupProceedPath, launchBarrierToken, "proceed")`
        // (`subagent-runner.ts:5233-5240`) and cannot touch a child session until the parent
        // writes the proceed token at `async-execution.ts:868`. cyrup has NO such barrier —
        // `grep -rn 'launch_barrier\|startup-proceed\|runner-startup' crates/cyrup-ext-subagents/src`
        // matches no CODE (only this comment and `initialize_process_terminal`'s, each quoting the
        // grep), and `spawn_detached_runner_with_command` returns with the runner already
        // running — so in cyrup the SPAWN *is* the authorization and the only placement that
        // satisfies that contract is before it. Nothing in `initialize_process_terminal` needs the
        // runner's pid, so there is no obstacle.
        //
        // A failure REFUSES THE LAUNCH, exactly as upstream's own `catch` does (`:853-857`
        // terminates the runner and returns the error): a run whose ownership could not be
        // established has no proof anyone can read, and every later reader would be left to guess.
        // Refusing here is cheaper than upstream's equivalent — no process exists yet to kill.
        if let Err(error) = crate::background::process_terminal::initialize_process_terminal(
            &crate::background::RunDir::for_existing(&run_paths.run_dir),
            &run_id,
            &runner_process_instance_id,
        )
        .await
        {
            Self::rollback_capacity(capacity.as_mut(), &run_id).await;
            return Err(SubagentError::Spawn(error));
        }

        let pid = match crate::background::spawn_detached::spawn_detached_runner_with_command(
            &resolved_command,
            &cfg_path,
            &run_paths.runner_stdout_log,
            &run_paths.runner_stderr_log,
            &env_overlay,
        ) {
            Ok(pid) => pid,
            Err(error) => {
                // SCOPE_9: the second fallible step past the claim, and the one that actually
                // happens in the field (a missing or unexecutable subagent binary).
                Self::rollback_capacity(capacity.as_mut(), &run_id).await;
                return Err(error);
            }
        };

        // SCOPE_9 — pi `markStarted` (`async-execution.ts:1409`, `:1422`), at its exact position:
        // the first statement after the spawn is CONFIRMED. The bind carries pi's own
        // `runnerProcessInstanceId` — the uuid minted above, the value the release verdict matches
        // this run's process-terminal proof against — together with the runner pid and that pid's
        // start identity, which are the no-proof fallback ladder beneath it. Binding is what
        // promotes the slot from a rollbackable reservation to a real run whose slot only
        // reconciliation may reclaim.
        //
        // A failure here is logged and NOT propagated: the detached runner is already running, and
        // failing the spawn over a bookkeeping write would orphan a live process. The handle
        // releases the slot itself when the durable bind cannot land (see `mark_started`), so the
        // worst case is one extra admission, never a permanently held slot.
        if let Some(handle) = capacity.as_mut()
            && let Err(error) = handle
                .mark_started(pid, runner_process_instance_id.clone())
                .await
        {
            tracing::warn!(
                run_id = %run_id,
                %error,
                "failed to bind the active-async capacity slot to the runner pid; the run is \
                 unaffected and the slot has been released"
            );
        }

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
            .track(
                run_id.clone(),
                run_paths,
                Some(std::time::SystemTime::now()),
            )
            .await;

        Ok(run_id)
    }
}

/// The `runner.type` of a foreign runner (`external-cli` / `external-job`), or `None` for the
/// native child — pi `const externalRunner = runner?.type === "external-cli" || runner?.type ===
/// "external-job"` (`async-execution.ts:1754` @v0.68.0).
pub(crate) fn external_runner_type(
    runner: Option<&crate::runner::AgentRunnerConfig>,
) -> Option<&'static str> {
    runner
        .filter(|runner| !matches!(runner, crate::runner::AgentRunnerConfig::Pi))
        .map(crate::runner::AgentRunnerConfig::type_str)
}

/// pi's async launch-time `fast mode` refusal for a foreign runner
/// (`async-execution.ts:1012,1756` @v0.68.0): every agent-running step whose EFFECTIVE `fast`
/// (`s.fast ?? params.fast ?? a.fast`, already folded onto the step by every dispatch site) is
/// `true` and whose persona declares an `external-cli`/`external-job` runner is refused with
/// upstream's own sentence, before anything is created.
///
/// `[CYRUP-DELTA]` (text only): upstream lists every unsupported feature of the step in one
/// sentence (`does not support: model override, fast mode.`); this check owns only `fast`, so its
/// list is `fast mode` alone.
fn refuse_external_runner_fast(
    steps: &[RunnerStep],
    resolved_agents: &BTreeMap<String, ResolvedAgentPersona>,
) -> Result<(), SubagentError> {
    let mut specs: Vec<&SingleStepSpec> = Vec::new();
    for step in steps {
        match step {
            RunnerStep::SingleStep(spec) => specs.push(spec),
            RunnerStep::ParallelGroup(group) => specs.extend(group.steps.iter()),
            RunnerStep::DynamicGroup(dynamic) => specs.push(&dynamic.template),
            RunnerStep::ImportAsyncRoot(_) => {}
        }
    }
    for spec in specs {
        if spec.fast != Some(true) {
            continue;
        }
        let Some(persona) = resolved_agents.get(&spec.agent) else {
            continue;
        };
        if let Some(runner_type) = external_runner_type(persona.runner.as_ref()) {
            return Err(SubagentError::Management(format!(
                "Agent '{}' uses runner.type='{runner_type}' and does not support: fast mode.",
                spec.agent
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::discovery::types::AgentReadScope;
    use crate::fork_context::ContextRequest;
    use cyrup_core::ModelId;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Launch-time `fast` refusal for a foreign runner (pi `async-execution.ts:1012,1756`
    /// @v0.68.0), driven through the REAL tool entry (`subagent` with `async: true`).
    mod external_fast_refusal {
        use super::*;
        use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
        use crate::paths::Roots;
        use crate::spawn::SpawnCommand;

        const REFUSAL: &str =
            "Agent 'foreign' uses runner.type='external-cli' and does not support: fast mode.";

        fn entries(dir: &Path) -> Vec<PathBuf> {
            match std::fs::read_dir(dir) {
                Ok(read) => read.filter_map(|e| e.ok().map(|e| e.path())).collect(),
                Err(_) => Vec::new(),
            }
        }

        async fn tool_for(dir: &Path, persona: &str) -> crate::extension::tool::SubagentTool {
            let agents = dir.join(".cyrup").join("agents");
            std::fs::create_dir_all(&agents).expect("mkdir agents");
            std::fs::write(agents.join("foreign.md"), persona).expect("write persona");
            let executor = Arc::new(SubagentExecutor::new());
            crate::extension::testsupport::arm_scoped_missions(&executor, dir).await;
            {
                let mut cfg = executor.config_cell().lock().await;
                cfg.roots = Roots::sandboxed(dir);
                cfg.spawn_command = Some(SpawnCommand {
                    binary: PathBuf::from("true"),
                    base_args: Vec::new(),
                });
            }
            crate::extension::tool::SubagentTool::new(executor, dir.to_path_buf())
        }

        async fn launch_text(
            tool: &crate::extension::tool::SubagentTool,
            params: serde_json::Value,
        ) -> String {
            match crate::extension::testsupport::dispatch_tool(tool, params).await {
                Ok(result) => crate::extension::testsupport::tool_text(&result),
                Err(error) => error.to_string(),
            }
        }

        /// The call's `fast: true` and the agent's own `fast: true` are each refused at the tool
        /// call, with pi's sentence, and neither leaves a run directory, a status file or a result
        /// behind. The same agent WITHOUT fast gets past the check and creates its run — so the
        /// refusal keys on `fast`, not on the runner. Mutation killed: removing the
        /// `refuse_external_runner_fast` call (the launch then creates the run and the refusal
        /// surfaces only at hop-2 spawn), or moving it after the run-dir creation.
        #[tokio::test]
        async fn an_external_cli_agent_with_fast_is_refused_before_any_run_exists() {
            const FOREIGN: &str = "---\nname: foreign\ndescription: F\n\
runner: {\"type\": \"external-cli\", \"command\": \"true\"}\n---\n\nbody\n";
            const FOREIGN_FAST: &str = "---\nname: foreign\ndescription: F\nfast: true\n\
runner: {\"type\": \"external-cli\", \"command\": \"true\"}\n---\n\nbody\n";
            for (persona, params) in [
                (
                    FOREIGN,
                    serde_json::json!({"agent": "foreign", "task": "t", "async": true, "fast": true}),
                ),
                (
                    FOREIGN_FAST,
                    serde_json::json!({"agent": "foreign", "task": "t", "async": true}),
                ),
                (
                    FOREIGN,
                    serde_json::json!({"chain": [{"agent": "foreign", "task": "t", "fast": true}], "async": true}),
                ),
            ] {
                let dir = tempfile::tempdir().expect("tempdir");
                let tool = tool_for(dir.path(), persona).await;
                let text = launch_text(&tool, params.clone()).await;
                assert!(text.contains(REFUSAL), "{params}: {text}");
                let roots = Roots::sandboxed(dir.path());
                let async_root = default_async_root_in(&roots, dir.path());
                let results = default_results_dir_in(&roots, dir.path());
                assert!(
                    entries(&async_root).is_empty(),
                    "{params}: no run directory: {:?}",
                    entries(&async_root)
                );
                assert!(
                    entries(&results).is_empty(),
                    "{params}: no result: {:?}",
                    entries(&results)
                );
            }

            // Control: no fast, same foreign agent — the launch proceeds and creates its run.
            let dir = tempfile::tempdir().expect("tempdir");
            let tool = tool_for(dir.path(), FOREIGN).await;
            let text = launch_text(
                &tool,
                serde_json::json!({"agent": "foreign", "task": "t", "async": true}),
            )
            .await;
            assert!(!text.contains(REFUSAL), "{text}");
            let roots = Roots::sandboxed(dir.path());
            assert!(
                !entries(&default_async_root_in(&roots, dir.path())).is_empty(),
                "the unrefused launch creates its run directory: {text}"
            );
        }
    }

    /// The inherited capability ceiling (`CYRUP_SUBAGENT_CAPABILITY_CEILING_V1`, pi
    /// `resolveCurrentSubagentCapabilityCeiling`, `capability-ceiling.ts:168-170` @v0.68.0) is
    /// read through the extension's env seam on the async launch path, so a MALFORMED inherited
    /// value is provably refused — fail closed, never "unbounded" — through the REAL tool entry,
    /// with no process-environment mutation.
    mod inherited_capability_ceiling_refusal {
        use super::*;
        use crate::paths::Roots;
        use crate::spawn::SpawnCommand;

        async fn tool_with_pin(
            dir: &Path,
            pin: Option<&str>,
        ) -> crate::extension::tool::SubagentTool {
            let agents = dir.join(".cyrup").join("agents");
            std::fs::create_dir_all(&agents).expect("mkdir agents");
            std::fs::write(
                agents.join("worker.md"),
                "---\nname: worker\ndescription: W\n---\n\nbody\n",
            )
            .expect("write persona");
            let executor = Arc::new(SubagentExecutor::new());
            crate::extension::testsupport::arm_scoped_missions(&executor, dir).await;
            {
                let mut cfg = executor.config_cell().lock().await;
                cfg.roots = Roots::sandboxed(dir);
                cfg.spawn_command = Some(SpawnCommand {
                    binary: PathBuf::from("true"),
                    base_args: Vec::new(),
                });
                cfg.env_overrides.insert(
                    crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV.to_string(),
                    pin.map(str::to_string),
                );
            }
            crate::extension::tool::SubagentTool::new(executor, dir.to_path_buf())
        }

        async fn launch_text(tool: &crate::extension::tool::SubagentTool) -> String {
            match crate::extension::testsupport::dispatch_tool(
                tool,
                serde_json::json!({"agent": "worker", "task": "t", "async": true}),
            )
            .await
            {
                Ok(result) => crate::extension::testsupport::tool_text(&result),
                Err(error) => error.to_string(),
            }
        }

        /// A malformed inherited ceiling refuses the launch with the decoder's own text; the same
        /// launch with the variable scrubbed goes ahead. Both a non-base64 value and a well-formed
        /// value of the wrong version fail closed.
        #[tokio::test]
        async fn a_malformed_inherited_capability_ceiling_refuses_an_async_launch() {
            use base64::Engine as _;
            let wrong_version = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(br#"{"version":99,"sources":["parent"]}"#);
            for (pin, expected) in [
                (
                    "%%% not base64 %%%",
                    "Invalid inherited capability ceiling: ",
                ),
                (
                    wrong_version.as_str(),
                    "Invalid inherited capability ceiling version.",
                ),
            ] {
                let dir = tempfile::tempdir().expect("tempdir");
                let text = launch_text(&tool_with_pin(dir.path(), Some(pin)).await).await;
                assert!(text.contains(expected), "{pin}: {text}");
            }

            let dir = tempfile::tempdir().expect("tempdir");
            let text = launch_text(&tool_with_pin(dir.path(), None).await).await;
            assert!(
                !text.contains("Invalid inherited capability ceiling"),
                "a scrubbed ceiling is unbounded, not refused: {text}"
            );
        }

        /// The FOREGROUND launch reads the same inherited ceiling through the same seam:
        /// `run_foreground_impl` hands `env_overrides` to `RunOptions::parent_env_overrides`, and
        /// `exec::spawn_plan::preflight_capability_ceiling` resolves the ceiling through it before
        /// any child exists. A malformed value refuses (fail closed); a well-formed one that
        /// excludes the agent refuses with upstream's restriction text
        /// (`capabilityCeilingAgentRestrictionMessage`, `capability-ceiling.ts:176-181`); a
        /// scrubbed one is unbounded.
        #[tokio::test]
        async fn an_inherited_capability_ceiling_binds_a_foreground_launch() {
            async fn foreground_text(pin: Option<&str>) -> String {
                let dir = tempfile::tempdir().expect("tempdir");
                let tool = tool_with_pin(dir.path(), pin).await;
                match crate::extension::testsupport::dispatch_tool(
                    &tool,
                    serde_json::json!({"agent": "worker", "task": "t", "async": false, "model": "anthropic/claude-sonnet-4-5"}),
                )
                .await
                {
                    Ok(result) => crate::extension::testsupport::tool_text(&result),
                    Err(error) => error.to_string(),
                }
            }

            let text = foreground_text(Some("%%% not base64 %%%")).await;
            assert!(
                text.contains("Invalid inherited capability ceiling: "),
                "{text}"
            );

            let excluding = crate::exec::capability_ceiling::encode_capability_ceiling(Some(
                &crate::exec::capability_ceiling::ResolvedCapabilityCeiling {
                    version: crate::exec::capability_ceiling::CAPABILITY_CEILING_VERSION,
                    allowed_tools: None,
                    allowed_agents: Some(vec!["reviewer".to_string()]),
                    deny_extensions: false,
                    sources: vec!["parent-ceiling".to_string()],
                },
            ))
            .expect("encode");
            let text = foreground_text(Some(&excluding)).await;
            assert!(
                text.contains(
                    "Capability ceiling from parent-ceiling does not allow agent 'worker'. \
                     Allowed agents: reviewer."
                ),
                "{text}"
            );

            let text = foreground_text(None).await;
            assert!(
                !text.contains("Invalid inherited capability ceiling")
                    && !text.contains("does not allow agent"),
                "a scrubbed ceiling is unbounded, not refused: {text}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------
    // SCOPE_9/SUBTASK4 — the per-session active-async capacity gate at the spawn path.
    // ---------------------------------------------------------------------------------------

    /// A `BackgroundSingleRequest` with nothing set beyond the agent and task, so the capacity
    /// assertions below are about the SLOT and not about step configuration.
    fn bare_background_request<'a>(cwd: &'a Path) -> BackgroundSingleRequest<'a> {
        BackgroundSingleRequest {
            machine_cwd: None,
            machine: None,
            thinking: None,
            usage_budget: None,
            turn_budget: None,
            structured_output_schema: None,
            tool_budget: None,
            cwd,
            agent_name: "worker",
            task: "do something",
            context: Some(ContextRequest::Fresh),
            model_override: None,
            agent_scope: AgentReadScope::Both,
            acceptance: None,
            control: None,
            include_progress: None,
            output: None,
            output_mode: None,
            fast: None,
            skills: None,
            share: None,
            session_dir: None,
            artifacts: None,
            timeout_ms: None,
        }
    }

    /// How many slots the capacity pool for `session` holds under a sandboxed root.
    async fn capacity_slots(root: &Path, session: &str) -> usize {
        let roots = crate::paths::Roots::sandboxed(root);
        let session = crate::identity::SessionId::parse(session).expect("non-empty");
        let pool = crate::background::active_async_capacity::session_pool_dir(
            &crate::background::active_async_capacity_root_in(&roots),
            &session,
        );
        crate::background::active_async_capacity::key::occupied_slots(&pool)
            .await
            .expect("listing")
            .len()
    }

    /// The ONE owner record the pool holds — the assertion target for a transfer, which must move
    /// a slot rather than add one.
    async fn only_capacity_owner(
        root: &Path,
        session: &str,
    ) -> crate::background::active_async_capacity::ActiveAsyncCapacityOwner {
        let roots = crate::paths::Roots::sandboxed(root);
        let session = crate::identity::SessionId::parse(session).expect("non-empty");
        let pool = crate::background::active_async_capacity::session_pool_dir(
            &crate::background::active_async_capacity_root_in(&roots),
            &session,
        );
        let slots = crate::background::active_async_capacity::key::occupied_slots(&pool)
            .await
            .expect("listing");
        let [slot] = slots.as_slice() else {
            panic!("expected exactly one occupied slot, got {slots:?}");
        };
        crate::background::active_async_capacity::read_owner(slot)
            .await
            .expect("owner.json")
    }

    /// SCOPE_9 — **the leak test.** Between the claim and `Ok(run_id)` there are two fallible
    /// steps, and a held-but-never-spawned slot is a PERMANENT leak that no reconcile can clear:
    /// there is no `status.json` for any release verdict to read, so the slot's owner sits at the
    /// "status file is missing or unreadable" rung forever.
    ///
    /// Driven through the real spawn path with a `spawn_command` that cannot exec — the shape
    /// `registration/mod.rs`'s `#[serde(skip)]` override exists for.
    #[tokio::test]
    async fn a_failed_detached_spawn_rolls_the_slot_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(crate::extension::testsupport::FixedSessionHost(
            "cap-rollback",
        )));
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.roots = crate::paths::Roots::sandboxed(dir.path());
            cfg.max_active_async_runs_per_session = Some(1);
            cfg.spawn_command = Some(crate::spawn::SpawnCommand {
                binary: dir.path().join("no-such-binary"),
                base_args: Vec::new(),
            });
        }

        let error = executor
            .spawn_background(bare_background_request(dir.path()))
            .await
            .expect_err("a spawn command that cannot exec must fail the spawn");
        assert!(
            matches!(error, SubagentError::Spawn(_)),
            "the spawn failure itself must surface, not a capacity error: {error}"
        );
        assert_eq!(
            capacity_slots(dir.path(), "cap-rollback").await,
            0,
            "the claim must be rolled back on every early return past it"
        );
    }

    /// SCOPE_9 §Q2 — a run with no session identity cannot be partitioned, so the gate is SKIPPED
    /// rather than the spawn refused. Refusing every headless async spawn would be a visible
    /// behaviour change for a cap that could not be enforced anyway.
    #[tokio::test]
    async fn a_spawn_with_no_session_id_claims_no_slot() {
        let dir = tempfile::tempdir().expect("tempdir");
        // NO `set_host_services` — `current_session_id()` is `None`, the headless case.
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.roots = crate::paths::Roots::sandboxed(dir.path());
            cfg.max_active_async_runs_per_session = Some(1);
            // `true(1)` execs and exits immediately: hop 1 is genuinely spawned and confirmed,
            // which is all this test needs, without a real runner writing into the sandbox.
            cfg.spawn_command = Some(crate::spawn::SpawnCommand {
                binary: PathBuf::from("true"),
                base_args: Vec::new(),
            });
        }

        for _ in 0..3 {
            executor
                .spawn_background(bare_background_request(dir.path()))
                .await
                .expect("a headless async spawn is never capacity-refused");
        }
        assert!(
            !crate::background::active_async_capacity_root_in(&crate::paths::Roots::sandboxed(
                dir.path()
            ))
            .exists(),
            "an unpartitionable run must not create a pool at all"
        );
    }

    /// SCOPE_9 — the gate, end to end through `spawn_background_steps`: two top-level async spawns
    /// in ONE session at `limit = 1`, the second refused with upstream's verbatim sentence.
    #[tokio::test]
    async fn a_second_top_level_async_spawn_at_the_cap_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(crate::extension::testsupport::FixedSessionHost(
            "cap-session",
        )));
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.roots = crate::paths::Roots::sandboxed(dir.path());
            cfg.max_active_async_runs_per_session = Some(1);
            cfg.spawn_command = Some(crate::spawn::SpawnCommand {
                binary: PathBuf::from("true"),
                base_args: Vec::new(),
            });
        }

        executor
            .spawn_background(bare_background_request(dir.path()))
            .await
            .expect("the first top-level async spawn fits");
        assert_eq!(capacity_slots(dir.path(), "cap-session").await, 1);

        let refused = executor
            .spawn_background(bare_background_request(dir.path()))
            .await
            .expect_err("the session is at its cap");
        assert_eq!(
            refused.to_string(),
            "Active async run capacity exhausted: 1/1 used.",
            "pi `active-async-capacity.ts:74` verbatim"
        );
        assert!(matches!(
            refused,
            SubagentError::ActiveAsyncCapacityExhausted(_)
        ));
        assert_eq!(
            capacity_slots(dir.path(), "cap-session").await,
            1,
            "a refused admission takes no slot of its own"
        );
    }

    /// SCOPE_9 — **the transfer, driven through `action: "resume"`.** pi branches the capacity
    /// gate on `target.source === "async"` and calls `transferActiveAsyncCapacity` there
    /// (`subagent-executor.ts:2085-2098` @v0.68.0); cyrup's equivalent moment is
    /// `control_resume`'s terminal-revival arm, which is the only caller that resumes an async
    /// run at all.
    ///
    /// Driven end to end through the PRODUCTION verb — `control_resume`, not `transfer` — at a cap
    /// of 1, with the source run still holding the session's only slot. Two facts are asserted,
    /// and only a transfer produces both: the pool still holds exactly ONE slot (an `acquire`
    /// would either refuse the revive with the exhausted sentence or add a second one), and that
    /// slot is now the REVIVED run's, at `generation` 1 with the source recorded as its
    /// `source_run_id` breadcrumb — the two fields `transfer` alone writes.
    #[tokio::test]
    async fn a_revive_transfers_the_source_runs_slot_rather_than_charging_the_cap_twice() {
        let dir = tempfile::tempdir().expect("tempdir");

        // `revive_from_transcript` resolves the revived persona against the ORIGINAL run's own
        // recorded cwd, so the agent has to be discoverable there.
        let agents_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir agents dir");
        std::fs::write(
            agents_dir.join("worker.md"),
            "---\nname: worker\ndescription: The revived persona\n---\nBody.\n",
        )
        .expect("write worker fixture");

        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(crate::extension::testsupport::FixedSessionHost(
            "cap-revive",
        )));
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.roots = crate::paths::Roots::sandboxed(dir.path());
            cfg.max_active_async_runs_per_session = Some(1);
            // `true(1)` execs and exits immediately: both hops are genuinely spawned and confirmed
            // (so `mark_started` binds a real pid) without a runner writing into the sandbox.
            cfg.spawn_command = Some(crate::spawn::SpawnCommand {
                binary: PathBuf::from("true"),
                base_args: Vec::new(),
            });
        }

        // The source run, admitted through the real gate: the session's one slot, generation 0.
        let source_run_id = executor
            .spawn_background(bare_background_request(dir.path()))
            .await
            .expect("the first top-level async spawn fits");
        let before = only_capacity_owner(dir.path(), "cap-revive").await;
        assert_eq!(before.run_id, source_run_id);
        assert_eq!(before.generation, 0);
        assert_eq!(before.source_run_id, None);

        // Settle it exactly as `finish_run` does — a terminal status whose step carries a
        // persisted transcript, which is what `control::resume` needs to resolve a revival, and
        // what `transfer` re-reads to decide the slot is no longer a live run's.
        let roots = crate::paths::Roots::sandboxed(dir.path());
        let async_root =
            crate::extension::executor::paths::default_async_root_in(&roots, dir.path());
        let results_dir =
            crate::extension::executor::paths::default_results_dir_in(&roots, dir.path());
        let source_paths = RunPaths::for_run(&async_root, &results_dir, &source_run_id);
        let session_file = dir.path().join("source-session.jsonl");
        std::fs::write(&session_file, "").expect("write dummy transcript");
        let mut status = crate::background::RunStatus::queued(
            source_run_id.clone(),
            RunMode::Single,
            Some(4242),
        );
        status
            .advance_state(crate::background::RunState::Running)
            .expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("worker");
        step.status = crate::background::StepState::Complete;
        step.session_file = Some(session_file.clone());
        status.steps = vec![step];
        status
            .advance_state(crate::background::RunState::Complete)
            .expect("Running -> Complete");
        status.cwd = Some(dir.path().to_path_buf());
        status.session_id = crate::identity::SessionId::parse("cap-revive");
        write_atomic_json(&source_paths.status, &status)
            .await
            .expect("write terminal status fixture");

        let confirmation = executor
            .control_resume(
                dir.path(),
                Some(source_run_id.as_str()),
                Some("carry on"),
                None,
                None,
            )
            .await
            .expect(
                "a revive at a cap of 1 must take over the source run's slot, not refuse itself \
                 with the exhausted sentence",
            );
        let revived_run_id = confirmation
            .lines()
            .find_map(|line| line.strip_prefix("Revived run: "))
            .expect("the confirmation names the revived run")
            .to_string();
        assert_ne!(revived_run_id, source_run_id.as_str());

        assert_eq!(
            capacity_slots(dir.path(), "cap-revive").await,
            1,
            "a revive is ONE operator-visible run continuing: the session must still hold exactly \
             one slot, never a second one charged for the same work"
        );
        let after = only_capacity_owner(dir.path(), "cap-revive").await;
        assert_eq!(
            after.run_id.as_str(),
            revived_run_id,
            "the slot must now be the revived run's"
        );
        assert_eq!(
            after.source_run_id.as_ref().map(RunId::as_str),
            Some(source_run_id.as_str()),
            "`transfer`'s breadcrumb, the only thing that lets `inspect_active_async_capacity_owner` \
             answer 'the slot moved' rather than 'it vanished'"
        );
        assert_eq!(
            after.generation, 1,
            "the generation bump is what invalidates the source run's stale handle; an `acquire` \
             would have written a fresh generation-0 record instead"
        );
        assert_eq!(
            after.slot, before.slot,
            "the SAME slot index, moved in place"
        );
        assert_eq!(
            after.reservation_token, before.reservation_token,
            "a transfer rewrites the owner in place; it does not re-reserve"
        );
    }

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
                machine_cwd: None,
                machine: None,
                thinking: None,
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextRequest::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: Some(true),
                output: Some(serde_json::json!("report.md")),
                output_mode: Some("file-only".to_string()),
                fast: None,
                // `normalize_skill_input`'s output shape, exactly as `route_single` hands it over.
                skills: Some(vec!["rust".to_string()]),
                share: Some(true),
                session_dir: Some(session_root.display().to_string()),
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots {
            async_root,
            results_dir,
        } = crate::background::run_artifact_roots(dir.path());
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
        assert_eq!(
            cfg.share,
            Some(true),
            "the `share` override must reach the runner config"
        );
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
                machine_cwd: None,
                machine: None,
                thinking: None,
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: Some(serde_json::json!({ "type": "object" })),
                tool_budget: Some(budget.clone()),
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextRequest::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                fast: None,
                skills: None,
                share: None,
                artifacts: None,
                session_dir: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots {
            async_root,
            results_dir,
        } = crate::background::run_artifact_roots(dir.path());
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
                    machine_cwd: None,
                    machine: None,
                    thinking: None,
                    // SUBA-021: unbudgeted on this path (see the field doc).
                    usage_budget: None,
                    turn_budget: None,
                    structured_output_schema: None,
                    tool_budget: None,
                    cwd: dir.path(),
                    agent_name: "worker",
                    task: "do something",
                    context: Some(ContextRequest::Fresh),
                    model_override: None,
                    agent_scope: AgentReadScope::Both,
                    acceptance: None,
                    control: None,
                    include_progress: None,
                    output: Some(serde_json::json!("report.md")),
                    output_mode: None,
                    fast: None,
                    skills: None,
                    share: None,
                    session_dir: None,
                    artifacts,
                    timeout_ms: None,
                })
                .await
                .expect("spawn_background should succeed for a resolvable builtin agent");

            let crate::background::RunArtifactRoots {
                async_root,
                results_dir,
            } = crate::background::run_artifact_roots(dir.path());
            let run_paths =
                crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
            let raw = std::fs::read_to_string(run_paths.run_dir.join("runner-config.json"))
                .expect("runner-config.json must exist");
            let cfg: crate::background::runner_main::RunnerConfig =
                serde_json::from_str(&raw).expect("runner-config.json must deserialize");
            let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
                panic!("expected one SingleStep");
            };
            let output = step
                .output_path
                .clone()
                .expect("output path must be resolved");
            // Strip the run-scoped leaf so the three runs' paths are comparable.
            let shape = output.replace(run_id.as_str(), "<runId>");
            resolved.push((
                artifacts,
                shape,
                cfg.artifacts_dir.is_some() && cfg.artifact_config.enabled,
            ));
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

        let before = u64::try_from(crate::time::now_epoch_millis()).unwrap_or(0);
        let run_id = executor
            .spawn_background(BackgroundSingleRequest {
                machine_cwd: None,
                machine: None,
                thinking: None,
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextRequest::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                fast: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: Some(60_000),
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");
        let after = u64::try_from(crate::time::now_epoch_millis()).unwrap_or(0);

        let crate::background::RunArtifactRoots {
            async_root,
            results_dir,
        } = crate::background::run_artifact_roots(dir.path());
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
                machine_cwd: None,
                machine: None,
                thinking: None,
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextRequest::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                fast: None,
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
                machine_cwd: None,
                machine: None,
                thinking: None,
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: &root,
                agent_name: "worker",
                task: "do something",
                context: Some(ContextRequest::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: Some(serde_json::json!("report.md")),
                output_mode: None,
                fast: None,
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

        let crate::background::RunArtifactRoots {
            async_root,
            results_dir,
        } = crate::background::run_artifact_roots(dir.path());
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
            step.output_path
                .clone()
                .expect("output path must be resolved")
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
            assert!(
                paths.run_dir.exists(),
                "each run's directory must be created before its write"
            );
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
                    machine_cwd: None,
                    machine: None,
                    thinking: None,
                    // SUBA-021: unbudgeted on this path (see the field doc).
                    usage_budget: None,
                    turn_budget: None,
                    structured_output_schema: None,
                    tool_budget: None,
                    cwd: dir.path(),
                    agent_name: "worker",
                    task: "do something",
                    context: Some(ContextRequest::Fresh),
                    model_override: None,
                    agent_scope: AgentReadScope::Both,
                    acceptance: Some(policy.clone()),
                    control: None,
                    include_progress: None,
                    output: None,
                    output_mode: None,
                    fast: None,
                    skills: None,
                    share: None,
                    session_dir: None,
                    artifacts: None,
                    timeout_ms: None,
                })
                .await
                .expect("spawn_background should succeed");

            let crate::background::RunArtifactRoots {
                async_root,
                results_dir,
            } = crate::background::run_artifact_roots(dir.path());
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
                    assert_eq!(
                        l.required_level, e.required_level,
                        "required_level for {policy}"
                    );
                    assert_eq!(l.verify, e.verify, "verify[] commands for {policy}");
                    assert!(
                        l.explicit,
                        "an explicitly declared policy stays explicit: {policy}"
                    );
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
                machine_cwd: None,
                machine: None,
                thinking: None,
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextRequest::Fresh),
                model_override: Some(ModelId::from("anthropic/claude-override-test")),
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                fast: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots {
            async_root,
            results_dir,
        } = crate::background::run_artifact_roots(dir.path());
        let run_paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        let raw = std::fs::read_to_string(&cfg_path)
            .expect("spawn_background must have written runner-config.json before spawning hop 1");
        let cfg: crate::background::runner_main::RunnerConfig =
            serde_json::from_str(&raw).expect("runner-config.json must deserialize");
        let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
            panic!(
                "a single-agent background run must produce exactly one SingleStep, got: {:?}",
                cfg.steps[0]
            );
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
                machine_cwd: None,
                machine: None,
                thinking: None,
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextRequest::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: Some(policy.clone()),
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                fast: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots {
            async_root,
            results_dir,
        } = crate::background::run_artifact_roots(dir.path());
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
                machine_cwd: None,
                machine: None,
                thinking: None,
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextRequest::Fresh),
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
                fast: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots {
            async_root,
            results_dir,
        } = crate::background::run_artifact_roots(dir.path());
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
        let control = cfg
            .control
            .expect("the resolved control config must be present");
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

    // ---------------------------------------------------------------------------------------
    // The recovery descriptor — pi `SteeringRecoveryDescriptor` (`shared/types.ts:805` @v0.68.0):
    // written by `spawn_background_steps` before the runner exists, consumed by `action: "resume"`.
    // Every test below drives the PRODUCTION verbs (`spawn_background` → `control_resume`) and
    // asserts at the `runner-config.json` boundary of the REVIVED run — the entire hop-1 → hop-2
    // contract (R-SA-073), written before its spawn, so no runner or model is needed.
    // ---------------------------------------------------------------------------------------
    mod recovery_descriptor {
        use super::*;
        use crate::artifacts::ArtifactConfig;
        use crate::background::runner_main::RunnerConfig;
        use crate::background::{
            DescriptorVersion, ModelOrigin, RecoveryDescriptor, RecoveryDescriptorError, RunDir,
            RunState, RunStatus, StepState, StepStatus,
        };
        use crate::discovery::types::{
            AgentMemoryConfig, MemoryScope, OutputMode, ResolvedToolBudget, SystemPromptMode,
            ToolBudgetBlock, ToolRef,
        };
        use crate::exec::control::ResolvedControlConfig;
        use crate::exec::turn_budget::ResolvedTurnBudget;
        use crate::exec::usage_budget::{UsageBudgetConfig, UsageBudgetLimit};
        use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
        use crate::extension::testsupport::FixedSessionHost;
        use crate::fork_context::ContextMode;
        use crate::paths::Roots;
        use crate::spawn::SpawnCommand;
        use crate::watchdog::permission_arbiter::PermissionRuleDecision;
        use std::collections::BTreeSet;

        /// The NARROW persona the round trip launches with. Every capability-shaped row (tools,
        /// excludeTools, maxSubagentDepth, completionGuard, subagentOnlyExtensions, …) is TIGHTER
        /// than the wide rewrite below, so a revive that consults the file instead of the
        /// descriptor fails on every one of them. Every row is also NON-default for its type:
        /// `inheritProjectContext`/`inheritSkills` are `true` because the parser's default for a
        /// non-`delegate` name is `false`, and a writer that emits the bool default must be
        /// visible — the rewrite flips them back. `modelProvider` is absent because the
        /// frontmatter parser projects none — see row 10 in the table.
        const NARROW_MD: &str = "---\nname: worker\ndescription: The launch persona\n\
model: fixture/persona-model\nthinking: low\nsystemPromptMode: replace\n\
inheritProjectContext: true\ninheritSkills: true\ntools: read, grep\nexcludeTools: bash\n\
allowNestedSubagents: true\nextensions: ext-a\nsubagentOnlyExtensions: ./child-only.ts\n\
skills: alpha\nmaxSubagentDepth: 2\ncompletionGuard: false\n\
toolBudget: {\"hard\": 7, \"soft\": 3}\nmemory: {scope: project, path: notes.md}\n---\n\
Launch body.\n";

        /// The WIDE rewrite of the persona file between launch and resume: every row differs from
        /// the launch contract and the capability rows are WIDER. This is the load-bearing trick
        /// of the mutation table — without it, persona-derived rows would pass by re-discovery
        /// even if the writer dropped them.
        const WIDE_MD: &str = "---\nname: worker\ndescription: The widened persona\n\
model: fixture/other-model\nthinking: medium\nsystemPromptMode: append\n\
inheritProjectContext: false\ninheritSkills: false\ntools: read, grep, bash\n\
allowNestedSubagents: false\nextensions: ext-z\nsubagentOnlyExtensions: ./other-child.ts\n\
skills: zeta\nmaxSubagentDepth: 9\ncompletionGuard: true\ntoolBudget: {\"hard\": 99}\n\
memory: {scope: user, path: other.md}\n---\nRewritten body.\n";

        /// Write `<cwd>/.cyrup/agents/<agent>.md` from one of the templates above.
        fn write_persona(cwd: &Path, agent: &str, template: &str) -> PathBuf {
            let agents_dir = cwd.join(".cyrup").join("agents");
            std::fs::create_dir_all(&agents_dir).expect("mkdir agents dir");
            let path = agents_dir.join(format!("{agent}.md"));
            std::fs::write(
                &path,
                template.replace("name: worker", &format!("name: {agent}")),
            )
            .expect("write persona fixture");
            path
        }

        fn launch_tool_budget() -> ResolvedToolBudget {
            ResolvedToolBudget {
                hard: 5,
                soft: None,
                block: ToolBudgetBlock::Names(vec!["read".to_string()]),
            }
        }

        fn launch_turn_budget() -> ResolvedTurnBudget {
            ResolvedTurnBudget {
                max_turns: 9,
                grace_turns: 1,
            }
        }

        fn launch_usage_budget() -> UsageBudgetConfig {
            UsageBudgetConfig {
                tokens: Some(UsageBudgetLimit {
                    soft: None,
                    hard: 1000.0,
                }),
                cost_usd: None,
            }
        }

        fn launch_memory() -> AgentMemoryConfig {
            AgentMemoryConfig {
                scope: MemoryScope::Project,
                path: "notes.md".to_string(),
            }
        }

        fn narrow_tools() -> Option<Vec<ToolRef>> {
            Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Builtin("grep".to_string()),
            ])
        }

        /// A host that reports a live session id AND a live model — what a launch under a running
        /// session sees, so a persona with no `model:` of its own INHERITS the session's (pi
        /// `ctx.currentModel` → `modelOrigin: "inherited"`). The model can be switched after the
        /// launch to play the session having moved to another model by the time it revives
        /// (`set_host_services` binds once, so the host itself must change its answer).
        struct InheritingHost {
            session: &'static str,
            model: std::sync::Mutex<&'static str>,
        }

        impl InheritingHost {
            fn new(session: &'static str, model: &'static str) -> Arc<Self> {
                Arc::new(Self {
                    session,
                    model: std::sync::Mutex::new(model),
                })
            }

            fn switch_model(&self, model: &'static str) {
                *self
                    .model
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = model;
            }
        }

        impl cyrup_ext::host::HostServices for InheritingHost {
            fn session_id(&self) -> Option<String> {
                Some(self.session.to_string())
            }
            fn current_model(&self) -> Option<String> {
                Some(
                    self.model
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .to_string(),
                )
            }
        }

        /// What varies between the descriptor tests' launches: the host they bind, the persona
        /// they write and the per-call model they pass. Everything else on the request is fixed
        /// in [`launch_with`], with an explicit NON-default value on every per-call field.
        struct LaunchSpec {
            session: &'static str,
            agent: &'static str,
            host: Arc<dyn cyrup_ext::host::HostServices>,
            persona: String,
            model_override: Option<ModelId>,
        }

        impl LaunchSpec {
            /// The round trip's launch: a fixed session with no live model, the NARROW persona,
            /// and an EXPLICIT per-call model.
            fn standard(session: &'static str, agent: &'static str) -> Self {
                Self {
                    session,
                    agent,
                    host: Arc::new(FixedSessionHost(session)),
                    persona: NARROW_MD.to_string(),
                    model_override: Some(ModelId::from("anthropic/override")),
                }
            }
        }

        /// One launch every descriptor test starts from: a sandboxed executor with a fixed
        /// session, the NARROW persona on disk, and a `spawn_background` carrying an explicit
        /// value on EVERY per-call field so that no row of the mutation table can be satisfied by
        /// the `None`/default the revive would otherwise fall back to.
        struct Launch {
            dir: tempfile::TempDir,
            executor: SubagentExecutor,
            session: &'static str,
            agent: &'static str,
            run_id: RunId,
            source_paths: RunPaths,
            schema: serde_json::Value,
            acceptance: serde_json::Value,
        }

        impl Launch {
            fn cwd(&self) -> &Path {
                self.dir.path()
            }

            fn descriptor_path(&self) -> PathBuf {
                RunDir::for_existing(&self.source_paths.run_dir).recovery_descriptor()
            }

            /// Settle the source run exactly as `finish_run` does: a terminal status whose one
            /// step carries a persisted transcript — what `control::resume` needs to resolve a
            /// revival. `cwd` is what the runner would have recorded, or `None` for a
            /// synthesized status that never observed it.
            async fn settle(&self, cwd: Option<PathBuf>) -> PathBuf {
                let session_file = self.cwd().join("source-session.jsonl");
                std::fs::write(&session_file, "").expect("write dummy transcript");
                let mut status =
                    RunStatus::queued(self.run_id.clone(), RunMode::Single, Some(4242));
                status
                    .advance_state(RunState::Running)
                    .expect("Queued -> Running");
                let mut step = StepStatus::pending(self.agent);
                step.status = StepState::Complete;
                step.session_file = Some(session_file.clone());
                status.steps = vec![step];
                status
                    .advance_state(RunState::Complete)
                    .expect("Running -> Complete");
                status.cwd = cwd;
                status.session_id = crate::identity::SessionId::parse(self.session);
                write_atomic_json(&self.source_paths.status, &status)
                    .await
                    .expect("write terminal status fixture");
                session_file
            }

            /// `action: "resume"` — the production verb, exactly as the tool and RPC surfaces
            /// reach it.
            async fn resume(&self) -> Result<String, String> {
                self.executor
                    .control_resume(
                        self.cwd(),
                        Some(self.run_id.as_str()),
                        Some("carry on"),
                        None,
                        None,
                    )
                    .await
            }

            /// The revived run's paths under `cwd`'s async root (the revive spawns in its
            /// effective cwd, and cyrup's async root is per-cwd).
            fn run_paths_under(&self, cwd: &Path, run_id: &RunId) -> RunPaths {
                let roots = Roots::sandboxed(self.cwd());
                RunPaths::for_run(
                    &default_async_root_in(&roots, cwd),
                    &default_results_dir_in(&roots, cwd),
                    run_id,
                )
            }
        }

        fn revived_id(confirmation: &str) -> RunId {
            RunId::from_token(
                confirmation
                    .lines()
                    .find_map(|line| line.strip_prefix("Revived run: "))
                    .expect("the confirmation names the revived run")
                    .to_string(),
            )
        }

        /// The REVIVED run's one-shot config — the whole hop-1 → hop-2 contract, written by
        /// `spawn_background_steps` before its spawn.
        fn read_runner_config(run_dir: &Path) -> RunnerConfig {
            let raw = std::fs::read_to_string(run_dir.join("runner-config.json"))
                .expect("the revive writes runner-config.json before spawning hop 1");
            serde_json::from_str(&raw).expect("runner-config.json must deserialize")
        }

        fn single_step(cfg: &RunnerConfig) -> &SingleStepSpec {
            match cfg.steps.as_slice() {
                [RunnerStep::SingleStep(step)] => step,
                other => panic!("a revive is one SingleStep, got {other:?}"),
            }
        }

        async fn launch(session: &'static str, agent: &'static str) -> Launch {
            launch_with(LaunchSpec::standard(session, agent)).await
        }

        async fn launch_with(spec: LaunchSpec) -> Launch {
            let LaunchSpec {
                session,
                agent,
                host,
                persona,
                model_override,
            } = spec;
            let dir = tempfile::tempdir().expect("tempdir");
            write_persona(dir.path(), agent, &persona);
            let executor = SubagentExecutor::new();
            executor.set_host_services(host);
            {
                let mut cfg = executor.config_cell().lock().await;
                cfg.roots = Roots::sandboxed(dir.path());
                cfg.max_active_async_runs_per_session = Some(1);
                cfg.max_subagent_depth = 4;
                cfg.permissions = Some(serde_json::json!({"rules": {"write": "deny"}}));
                // `true(1)` execs and exits immediately: both hops are genuinely spawned and
                // confirmed without a runner writing into the sandbox.
                cfg.spawn_command = Some(SpawnCommand {
                    binary: PathBuf::from("true"),
                    base_args: Vec::new(),
                });
            }
            let schema =
                serde_json::json!({"type": "object", "properties": {"ok": {"type": "boolean"}}});
            let acceptance = serde_json::json!({
                "level": "verified",
                "verify": [{"id": "unit", "command": "true"}]
            });
            let run_id = executor
                .spawn_background(BackgroundSingleRequest {
                    machine_cwd: None,
                    machine: None,
                    structured_output_schema: Some(schema.clone()),
                    tool_budget: Some(launch_tool_budget()),
                    turn_budget: Some(launch_turn_budget()),
                    usage_budget: Some(launch_usage_budget()),
                    cwd: dir.path(),
                    agent_name: agent,
                    task: "do the thing",
                    context: Some(ContextRequest::Fresh),
                    model_override,
                    thinking: Some("high".to_string()),
                    agent_scope: AgentReadScope::Both,
                    acceptance: Some(acceptance.clone()),
                    control: Some(crate::registration::ControlConfig {
                        needs_attention_after_ms: Some(1234),
                        ..crate::registration::ControlConfig::default()
                    }),
                    include_progress: Some(true),
                    output: Some(serde_json::json!("out.md")),
                    output_mode: Some("file-only".to_string()),
                    fast: None,
                    skills: Some(vec!["beta".to_string()]),
                    share: Some(true),
                    session_dir: Some(dir.path().join("sessions").display().to_string()),
                    // ENABLED, so row 47 lands a real directory and row 48 the `foreground()`
                    // config (jsonl on) — `false` would make both rows indistinguishable from a
                    // dropped field (`None` / `default()`).
                    artifacts: Some(true),
                    timeout_ms: Some(60_000),
                })
                .await
                .expect("the launch succeeds and writes its descriptor");
            let roots = Roots::sandboxed(dir.path());
            let source_paths = RunPaths::for_run(
                &default_async_root_in(&roots, dir.path()),
                &default_results_dir_in(&roots, dir.path()),
                &run_id,
            );
            Launch {
                dir,
                executor,
                session,
                agent,
                run_id,
                source_paths,
                schema,
                acceptance,
            }
        }

        /// **The round trip, through production** — pi `async-execution.ts:1993-2055` (write) →
        /// `async-resume.ts:310-632` (read + overlay) → `subagent-executor.ts:2059-2185` (the
        /// revived launch), all @v0.68.0.
        ///
        /// Assert A checks the WRITE: the file the launch leaves behind, its mode, pi's key
        /// spelling, and the typed reader's view of it field for field. Then the run is settled,
        /// the persona file is REWRITTEN WIDER, and `action: "resume"` is driven. Assert B is the
        /// mutation table, read off the REVIVED run's `runner-config.json`. Every launch value is
        /// NON-default for its type and differs from what the widened file would give, so a
        /// writer that drops a field — emitting `None`, an empty list, `false`, or the config's
        /// raw value in its place — fails that field's own row here.
        ///
        /// Six descriptor fields have NO row in this table and are pinned elsewhere:
        /// `modelOverrideFromParent` and `modelOrigin` by
        /// `a_revive_keeps_the_stored_model_origin_rather_than_re_deriving_it`;
        /// `capabilityCeiling` and `thinkingCeiling` by
        /// `a_revive_re_applies_the_launch_s_ceilings_to_its_runner_s_environment`;
        /// `launchContractDigest` by `the_launch_digest_binds_the_task` (Assert A checks only
        /// its shape); `lane` and `runFanoutBudget` are always `None` on a single run and have no
        /// discriminating test (disclosed in `[EXEC — descriptor]`). Trimming those sibling tests
        /// would silently lose the only production coverage of those fields.
        ///
        /// Two rows here are known-vacuous through production and pinned by unit tests instead:
        /// `modelProvider` (row 10: the frontmatter parser projects none, so no in-crate launch can
        /// seed one) and `thinkingCeiling` (row 13: its only launch source is the process env, which
        /// `forbid(unsafe_code)` keeps a test from setting). Those two are caught by
        /// `recovery_descriptor::tests::for_single_launch_projects_every_field_of_the_resolved_launch`.
        /// The per-field mutation record (field → the test that failed) is the task's
        /// `[FIX — descriptor]` section.
        #[tokio::test]
        async fn a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s() {
            let launch = launch("descriptor-round-trip", "worker").await;
            let descriptor_path = launch.descriptor_path();

            // ---------------------------------------------------------------- Assert A: the write
            let raw: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&descriptor_path)
                    .expect("the launch writes recovery-descriptor.json before it spawns"),
            )
            .expect("JSON");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(
                    std::fs::metadata(&descriptor_path)
                        .expect("metadata")
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600,
                    "pi `writePrivateAtomicJson`: the file carries the system prompt"
                );
            }
            assert_eq!(raw["version"], 1);
            assert_eq!(raw["sourceRunId"], launch.run_id.as_str());
            assert_eq!(raw["agent"], "worker");
            assert_eq!(raw["cwd"], launch.cwd().display().to_string());
            assert_eq!(raw["outputMode"], "file-only");
            assert_eq!(
                raw["tools"],
                serde_json::json!(["read", "grep"]),
                "pi's string form"
            );
            assert_eq!(raw["maxSubagentDepth"], 2);
            assert_eq!(raw["share"], true);
            assert_eq!(raw["modelOrigin"], "explicit");

            let written = RecoveryDescriptor::read(&descriptor_path)
                .await
                .expect("the typed reader accepts the writer's output")
                .expect("present");
            // Evidence rows, asserted by shape (their exact values derive from the tempdir and
            // the clock) and then copied into the field-for-field expectation below.
            let digest = written.launch_contract_digest.as_str();
            assert!(
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "launchContractDigest is 64 lower-hex: {digest}"
            );
            let now = u64::try_from(crate::time::now_epoch_millis()).expect("epoch");
            let deadline = written
                .absolute_deadline_at
                .expect("timeoutMs arms an absolute deadline");
            assert!(
                deadline <= now + 60_000 && deadline + 5 * 60_000 > now + 60_000,
                "absoluteDeadlineAt ≈ launch + 60s: {deadline} vs now {now}"
            );
            assert!(
                written
                    .agent_file_path
                    .as_deref()
                    .is_some_and(|path| path.ends_with(".cyrup/agents/worker.md")),
                "agentFilePath is the discovered definition: {:?}",
                written.agent_file_path
            );
            assert!(
                written
                    .output_path
                    .as_deref()
                    .is_some_and(|path| path.ends_with("out.md")),
                "outputPath is the resolved absolute file: {:?}",
                written.output_path
            );
            let artifacts_dir = crate::artifacts::project_artifacts_dir(launch.cwd());
            let expected = RecoveryDescriptor {
                fast: None,
                // SUBA-101: always recorded (pi `async-execution.ts:2024`); NARROW_MD declares
                // none, so the parser default `false`.
                inherit_global_context: Some(false),
                mutation_tools: None,
                version: DescriptorVersion,
                launch_contract_digest: written.launch_contract_digest.clone(),
                source_run_id: launch.run_id.clone(),
                agent: "worker".to_string(),
                cwd: launch.cwd().to_path_buf(),
                session_file: None,
                model: Some(ModelId::from("anthropic/override")),
                model_origin: ModelOrigin::Explicit,
                model_override_from_parent: None,
                model_provider: None,
                thinking: Some("high".to_string()),
                thinking_ceiling: None,
                tools: narrow_tools(),
                exclude_tools: vec!["bash".to_string()],
                allow_nested_subagents: Some(true),
                extensions: Some(vec!["ext-a".to_string()]),
                subagent_only_extensions: vec!["./child-only.ts".to_string()],
                system_prompt: Some("Launch body.".to_string()),
                system_prompt_mode: SystemPromptMode::Replace,
                inherit_project_context: true,
                inherit_skills: true,
                skills: vec!["beta".to_string()],
                agent_file_path: written.agent_file_path.clone(),
                completion_guard: Some(false),
                memory: Some(launch_memory()),
                output_path: written.output_path.clone(),
                output_mode: OutputMode::FileOnly,
                structured_output_schema: Some(launch.schema.clone()),
                acceptance: Some(launch.acceptance.clone()),
                control_config: Some(ResolvedControlConfig {
                    needs_attention_after_ms: 1234,
                    ..ResolvedControlConfig::default()
                }),
                context: Some(ContextMode::Fresh),
                lane: None,
                absolute_deadline_at: written.absolute_deadline_at,
                initial_tool_budget: Some(launch_tool_budget()),
                max_subagent_depth: 2,
                capability_ceiling: None,
                share: true,
                session_dir: Some(launch.cwd().join("sessions").join("run-0")),
                artifacts_dir: Some(artifacts_dir.clone()),
                artifact_config: ArtifactConfig::foreground(),
                run_fanout_budget: None,
                turn_budget: Some(launch_turn_budget()),
                usage_budget: Some(launch_usage_budget()),
                permission_rules: Some(BTreeMap::from([(
                    "write".to_string(),
                    PermissionRuleDecision::Deny,
                )])),
                include_progress: Some(true),
            };
            assert_eq!(
                written, expected,
                "the descriptor IS the resolved launch, field for field"
            );

            // ------------------------------------------ settle, WIDEN the file, then resume
            let session_file = launch.settle(Some(launch.cwd().to_path_buf())).await;
            write_persona(launch.cwd(), "worker", WIDE_MD);
            let confirmation = launch
                .resume()
                .await
                .expect("a settled run with a descriptor revives");
            let revived = revived_id(&confirmation);
            assert_ne!(revived, launch.run_id);
            let revived_paths = launch.run_paths_under(launch.cwd(), &revived);
            let cfg = read_runner_config(&revived_paths.run_dir);
            let step = single_step(&cfg);
            let persona = cfg
                .resolved_agents
                .get("worker")
                .expect("the revived persona map carries the agent");

            // -------------------------------------------------- Assert B: the mutation table
            // rows 7/8 — model: an EXPLICIT launch model is the step's override again, and pi
            // pins it on the agent config too (the file now says fixture/other-model).
            assert_eq!(
                step.model.as_ref().map(ModelId::as_str),
                Some("anthropic/override")
            );
            assert_eq!(
                persona.model.as_ref().map(ModelId::as_str),
                Some("anthropic/override")
            );
            // row 10 — modelProvider. The frontmatter parser projects none, so the launch value
            // is `None` and the file rewrite cannot make this row non-vacuous here; the `Some`
            // path is proven by `recovery_descriptor::tests::for_single_launch_projects_every_
            // field_of_the_resolved_launch` and `apply_to_persona_overlays_the_descriptor_over_
            // the_file`.
            assert_eq!(persona.model_provider, None);
            // row 12 — thinking (file now says medium)
            assert_eq!(persona.thinking.as_deref(), Some("high"));
            // row 14 — tools: THE capability-widening fix (the file now grants bash)
            assert_eq!(persona.tools, narrow_tools());
            // row 15 — excludeTools (file now [])
            assert_eq!(persona.exclude_tools, vec!["bash".to_string()]);
            // row 16 — allowNestedSubagents (file now false)
            assert_eq!(persona.allow_nested_subagents, Some(true));
            // row 17 — extensions (file now ext-z)
            assert_eq!(persona.extensions, Some(vec!["ext-a".to_string()]));
            // row 18 — subagentOnlyExtensions (file now ./other-child.ts; a dropped field lands
            // an empty list)
            assert_eq!(
                persona.subagent_only_extensions,
                vec!["./child-only.ts".to_string()]
            );
            // row 21 — systemPrompt (file body rewritten)
            assert_eq!(persona.system_prompt_body, "Launch body.");
            // row 22 — systemPromptMode (file now append)
            assert_eq!(persona.system_prompt_mode, SystemPromptMode::Replace);
            // rows 23/25 — inheritProjectContext / inheritSkills: `true` at launch (the parser's
            // default is `false`), `false` in the file now — a writer emitting the bool default
            // and a re-discovery both land `false` here
            assert!(persona.inherit_project_context);
            assert!(persona.inherit_skills);
            // row 26 — skills: the EFFECTIVE launch list, on the step (pi passes it as the
            // per-call override) and on the persona (pi overlays it too)
            assert_eq!(step.skills, Some(vec!["beta".to_string()]));
            assert_eq!(persona.skills, vec!["beta".to_string()]);
            // row 28 — agentFilePath
            assert_eq!(persona.file_path, written.agent_file_path);
            // row 29 — completionGuard (file now true)
            assert_eq!(persona.completion_guard, Some(false));
            // row 30 — memory (file now user/other.md)
            assert_eq!(persona.memory, Some(launch_memory()));
            // row 31 — outputPath
            assert_eq!(step.output_path, written.output_path);
            // row 32 — outputMode (the default would be Inline)
            assert_eq!(step.output_mode, Some(OutputMode::FileOnly));
            // row 33 — structuredOutputSchema
            assert_eq!(step.structured_output_schema, Some(launch.schema.clone()));
            // row 34 — acceptance
            assert_eq!(step.acceptance, Some(launch.acceptance.clone()));
            // row 35 — controlConfig (a dropped field re-folds from config → the 60s default)
            assert_eq!(
                cfg.control.as_ref().map(|c| c.needs_attention_after_ms),
                Some(1234)
            );
            // row 36 — context (the old hard-coded Fork would fail)
            assert_eq!(step.context, Some(ContextMode::Fresh));
            // row 40 — initialToolBudget (file now says 99)
            assert_eq!(persona.tool_budget, Some(launch_tool_budget()));
            // row 41 — maxSubagentDepth: the second widening fix (file now 9; re-discovery
            // would give 9)
            assert_eq!(step.max_depth_override, Some(2));
            assert_eq!(persona.max_subagent_depth, Some(2));
            // row 45 — share
            assert_eq!(cfg.share, Some(true));
            // row 46 — sessionDir (the launch leaf)
            assert_eq!(
                step.session_dir,
                Some(launch.cwd().join("sessions").join("run-0"))
            );
            // rows 47/48 — artifactsDir / artifactConfig: artifacts were ENABLED at launch, so
            // the dir is the project artifacts root under the launch cwd and the config is
            // `foreground()` (jsonl on); the old revive sent `None` / `default()` (jsonl off),
            // which is exactly what a dropped field lands
            assert_eq!(cfg.artifacts_dir.as_deref(), Some(artifacts_dir.as_path()));
            assert_eq!(cfg.artifact_config, ArtifactConfig::foreground());
            assert!(
                cfg.artifact_config.include_jsonl,
                "the one bit on which foreground() differs from default()"
            );
            // row 4 — cwd, with `status.cwd` recorded (the descriptor rung is proven separately)
            assert_eq!(cfg.cwd.as_path(), launch.cwd());
            // the four cyrup-only run-level keys
            assert_eq!(cfg.turn_budget, Some(launch_turn_budget()));
            assert_eq!(cfg.usage_budget, Some(launch_usage_budget()));
            assert_eq!(
                cfg.permission_rules
                    .as_ref()
                    .and_then(|rules| rules.get("write")),
                Some(&PermissionRuleDecision::Deny)
            );
            assert_eq!(cfg.include_progress, Some(true));
            // the transcript the revive is seeded from (pi `revivalSessionFile`)
            assert_eq!(step.session_file.as_deref(), Some(session_file.as_path()));
            assert_eq!(cfg.session_file.as_deref(), Some(session_file.as_path()));
            // VL-S3, the ORCHESTRATOR half — pi `config.revivalLease` (`subagent-runner.ts:219`,
            // acquired at `:5242`), built in `control.rs::revive_from_transcript`. This is the one
            // path in cyrup that reopens a stored transcript for WRITING, and without the request
            // on the config the runner takes no lease at all: two revives of one session file
            // would both spawn, both open the same `.jsonl` for writing, and interleave turns —
            // VL-S3's exact hazard, reintroduced by one word. Asserted at the `runner-config.json`
            // boundary because that file IS the whole hop-1 -> hop-2 contract; the lease
            // integration test builds the request by hand and so cannot see this seam.
            let lease = cfg
                .revival_lease
                .as_ref()
                .expect("a revive must carry a session lease request to its runner");
            assert_eq!(lease.session_file.as_path(), session_file.as_path());
            assert_eq!(
                lease.run_id, revived,
                "the claim is recorded against the REVIVED run, which is the process that will \
                 hold it"
            );
            assert_eq!(
                lease.source_run_id, launch.run_id,
                "the breadcrumb names the settled run this revive continues, which is what the \
                 conflict sentence reads back to the operator"
            );
            // tools/extensions ride the persona, never the step (pi overlays `agentConfig.*`)
            assert_eq!(step.tools, None);
            assert_eq!(step.extensions, None);

            // pi `recoveryAgentConfig` (`async-execution.ts:1992`): the revived run writes its
            // OWN descriptor from the OVERLAID persona, so a second revive sees the same contract
            // — and row 6 through production: its sessionFile is the transcript it was seeded
            // from.
            let revived_descriptor = RecoveryDescriptor::read(
                &RunDir::for_existing(&revived_paths.run_dir).recovery_descriptor(),
            )
            .await
            .expect("readable")
            .expect("pi writes one per revive");
            assert_eq!(revived_descriptor.source_run_id, revived);
            assert_eq!(
                revived_descriptor.session_file.as_deref(),
                Some(session_file.as_path())
            );
            assert_eq!(revived_descriptor.tools, narrow_tools());
            assert_eq!(revived_descriptor.max_subagent_depth, 2);
            assert_eq!(revived_descriptor.model, written.model);
            assert_eq!(revived_descriptor.model_origin, ModelOrigin::Explicit);
            assert_eq!(revived_descriptor.system_prompt, written.system_prompt);
            assert_eq!(
                revived_descriptor.permission_rules,
                written.permission_rules
            );
            assert_eq!(
                revived_descriptor.initial_tool_budget,
                written.initial_tool_budget
            );
        }

        /// SUBA-103 / SUBA-101 / SUBA-102, through production: the launch's EFFECTIVE `fast`
        /// (here the agent's own `fast: true`, folded onto the step by `spawn_background`), its
        /// `inheritGlobalContext` and its `mutationTools` are written to the descriptor, the file
        /// is then rewritten WITHOUT any of them, and `action: "resume"` still relaunches the
        /// revived step fast with the launch's global-context flag and mutation tools — and
        /// records all three again on the revived run's own descriptor. Mutation killed:
        /// `fast: None` at `control.rs`'s revive (the revived step is standard tier), `fast: None`
        /// in `for_single_launch` (nothing to replay), or dropping the two `apply_to_persona`
        /// lines (the widened file's `false`/`None` win).
        #[tokio::test]
        async fn a_revived_run_replays_fast_global_context_and_mutation_tools() {
            let persona = NARROW_MD.replace(
                "completionGuard: false\n",
                "completionGuard: false\nfast: true\ninheritGlobalContext: true\n\
mutationTools: apply_patch, notebook_edit\n",
            );
            assert!(persona.contains("\nfast: true\n"), "fixture: {persona}");
            let launch = launch_with(LaunchSpec {
                persona,
                ..LaunchSpec::standard("descriptor-fast-replay", "worker")
            })
            .await;
            let raw: serde_json::Value = serde_json::from_slice(
                &std::fs::read(launch.descriptor_path()).expect("descriptor written"),
            )
            .expect("JSON");
            assert_eq!(raw["fast"], true);
            assert_eq!(raw["inheritGlobalContext"], true);
            assert_eq!(
                raw["mutationTools"],
                serde_json::json!(["apply_patch", "notebook_edit"])
            );

            launch.settle(Some(launch.cwd().to_path_buf())).await;
            write_persona(launch.cwd(), "worker", WIDE_MD);
            let confirmation = launch.resume().await.expect("revives");
            let revived = revived_id(&confirmation);
            let revived_paths = launch.run_paths_under(launch.cwd(), &revived);
            let cfg = read_runner_config(&revived_paths.run_dir);
            let step = single_step(&cfg);
            let revived_persona = cfg
                .resolved_agents
                .get("worker")
                .expect("the revived persona map carries the agent");
            assert_eq!(step.fast, Some(true), "the revived step replays fast");
            assert!(revived_persona.inherit_global_context);
            assert_eq!(
                revived_persona.mutation_tools,
                Some(vec!["apply_patch".to_string(), "notebook_edit".to_string()])
            );
            let revived_descriptor = RecoveryDescriptor::read(
                &RunDir::for_existing(&revived_paths.run_dir).recovery_descriptor(),
            )
            .await
            .expect("readable")
            .expect("written per revive");
            assert_eq!(revived_descriptor.fast, Some(true));
            assert_eq!(revived_descriptor.inherit_global_context, Some(true));
            assert_eq!(
                revived_descriptor.mutation_tools,
                Some(vec!["apply_patch".to_string(), "notebook_edit".to_string()])
            );
        }

        /// SUBA-100 — pi's revive relaunches `params.machine ?? agentConfig.machine`
        /// (`async-execution.ts:1774` @v0.68.0) over the DISCOVERED agent, resolving the placement
        /// before the run exists. So once the agent file names a saved machine, the revived step is
        /// PLACED — resolved against the catalog and `subagents.machines` — and never runs locally
        /// (its runner then meets `serializeHerdrPiLaunch`'s resume refusal, as upstream's does).
        ///
        /// *Gutted by*: `machine: None` at `control.rs`'s revive step, or dropping its fold.
        #[tokio::test]
        async fn a_revived_agent_that_names_a_machine_is_placed_there() {
            let launch = launch("descriptor-revive-machine", "worker").await;
            launch.settle(Some(launch.cwd().to_path_buf())).await;
            let placed = NARROW_MD.replace(
                "completionGuard: false\n",
                "completionGuard: false\nmachine: fake-box\n",
            );
            assert!(
                placed.contains("\nmachine: fake-box\n"),
                "fixture: {placed}"
            );
            write_persona(launch.cwd(), "worker", &placed);
            std::fs::write(
                launch.cwd().join(".cyrup/agents/settings.json"),
                serde_json::json!({"subagents":{"machines":{"fake-box":{"cwd":"/srv/repo"}}}})
                    .to_string(),
            )
            .expect("settings");
            let herdr = launch.cwd().join("fake-herdr");
            std::fs::write(
                &herdr,
                "#!/bin/sh\nprintf '%s\\n' '{\"machines\":[{\"id\":\"m-fake\",\"label\":\"fake-box\",\"target\":\"me@fake-box\",\"enabled\":true}]}'\n",
            )
            .expect("herdr");
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
            launch
                .executor
                .config_cell()
                .lock()
                .await
                .env_overrides
                .insert(
                    cyrup_herdr::cli::HERDR_BIN.to_string(),
                    Some(herdr.display().to_string()),
                );
            let confirmation = launch.resume().await.expect("revives");
            let revived_paths = launch.run_paths_under(launch.cwd(), &revived_id(&confirmation));
            let step = single_step(&read_runner_config(&revived_paths.run_dir)).clone();
            let placement = step.machine.expect("the revived step is placed");
            assert_eq!(placement.requested, "fake-box");
            let resolved = placement.resolved.expect("resolved before the run exists");
            assert_eq!(resolved.id, "m-fake");
            assert_eq!(resolved.cwd, "/srv/repo");
            assert_eq!(step.cwd, None, "the machine cwd never becomes a local one");
        }

        /// SUBA-103, the revive's FALLBACK rung, through production. A launch no rung set `fast`
        /// on records none (pi writes `fast` only when set, `async-execution.ts:2008` @v0.68.0),
        /// and pi's revive relaunches with `params.fast ?? agentConfig.fast` (`:1967`) over the
        /// CURRENTLY discovered agent — so once the file says `fast: true`, the revived step runs
        /// fast and records it. The `[CYRUP-DELTA]` half: a descriptor that recorded an explicit
        /// `false` keeps its revive standard although the file now says `true`.
        ///
        /// *Gutted by*: `fast: descriptor.fast` at `control.rs`'s revive (the fallback is lost and
        /// the first revive is standard), or `current_agent_fast` projected as `None` (same).
        #[tokio::test]
        async fn a_revive_with_no_recorded_fast_takes_the_agents_current_fast() {
            let fast_now = NARROW_MD.replace(
                "completionGuard: false\n",
                "completionGuard: false\nfast: true\n",
            );
            assert!(fast_now.contains("\nfast: true\n"), "fixture: {fast_now}");

            let unset = launch("descriptor-fast-fallback", "worker").await;
            let raw: serde_json::Value = serde_json::from_slice(
                &std::fs::read(unset.descriptor_path()).expect("descriptor written"),
            )
            .expect("JSON");
            assert!(raw.get("fast").is_none(), "no rung set fast: {raw}");
            unset.settle(Some(unset.cwd().to_path_buf())).await;
            write_persona(unset.cwd(), "worker", &fast_now);
            let confirmation = unset.resume().await.expect("revives");
            let revived_paths = unset.run_paths_under(unset.cwd(), &revived_id(&confirmation));
            assert_eq!(
                single_step(&read_runner_config(&revived_paths.run_dir)).fast,
                Some(true),
                "an unrecorded fast falls back to the agent's current `fast: true`"
            );
            let revived_descriptor = RecoveryDescriptor::read(
                &RunDir::for_existing(&revived_paths.run_dir).recovery_descriptor(),
            )
            .await
            .expect("readable")
            .expect("written per revive");
            assert_eq!(revived_descriptor.fast, Some(true), "and records it");

            let pinned = launch("descriptor-fast-false", "worker").await;
            let path = pinned.descriptor_path();
            let mut descriptor = RecoveryDescriptor::read(&path)
                .await
                .expect("readable")
                .expect("present");
            descriptor.fast = Some(false);
            descriptor.write(&path).await.expect("rewrite");
            pinned.settle(Some(pinned.cwd().to_path_buf())).await;
            write_persona(pinned.cwd(), "worker", &fast_now);
            let confirmation = pinned.resume().await.expect("revives");
            let revived_paths = pinned.run_paths_under(pinned.cwd(), &revived_id(&confirmation));
            assert_eq!(
                single_step(&read_runner_config(&revived_paths.run_dir)).fast,
                Some(false),
                "a recorded `false` is the launch's own decision and outranks the file"
            );
        }

        /// Row 4 through production: with no recorded `status.cwd` the revive runs in the cwd the
        /// WRITER recorded, and records it again on its own descriptor. cyrup's async root is
        /// keyed by the exact cwd (`background::artifact_roots::cwd_key`), so a resume can only
        /// be requested from the launch cwd — which is why a wrong recorded cwd shows up here as
        /// the revived run landing under some OTHER key (its `runner-config.json` is then absent
        /// from this root), not as a different request cwd being chosen. The reader's rung ORDER
        /// (descriptor over request) is the next test's, with a deliberately re-pointed
        /// descriptor.
        #[tokio::test]
        async fn a_revive_without_a_recorded_cwd_runs_in_the_cwd_the_writer_recorded() {
            let launch = launch("descriptor-cwd-written", "worker").await;
            launch.settle(None).await;

            let confirmation = launch.resume().await.expect("revives in the recorded cwd");
            let revived = revived_id(&confirmation);
            let revived_paths = launch.run_paths_under(launch.cwd(), &revived);
            let cfg = read_runner_config(&revived_paths.run_dir);
            assert_eq!(cfg.cwd.as_path(), launch.cwd());
            let revived_descriptor = RecoveryDescriptor::read(
                &RunDir::for_existing(&revived_paths.run_dir).recovery_descriptor(),
            )
            .await
            .expect("readable")
            .expect("written per revive");
            assert_eq!(
                revived_descriptor.cwd.as_path(),
                launch.cwd(),
                "the revived run records the same cwd for the revive after it"
            );
        }

        /// Row 4, the reader's rung ORDER — pi `async-resume.ts:579`: with no recorded
        /// `status.cwd` the revive runs in the descriptor's cwd, not the request's. Because a
        /// production descriptor always records the launch cwd (the test above), the two rungs
        /// can only be told apart by RE-POINTING the descriptor by hand at a sibling directory
        /// (which has its own copy of the persona): the revived run then demonstrably lands under
        /// THAT cwd's async root and not the request's.
        #[tokio::test]
        async fn a_revive_without_a_recorded_cwd_falls_back_to_the_descriptor_s() {
            let launch = launch("descriptor-cwd-rung", "worker").await;
            launch.settle(None).await;
            let elsewhere = launch.cwd().join("elsewhere");
            write_persona(&elsewhere, "worker", NARROW_MD);
            let path = launch.descriptor_path();
            let mut descriptor = RecoveryDescriptor::read(&path)
                .await
                .expect("readable")
                .expect("present");
            descriptor.cwd = elsewhere.clone();
            descriptor.write(&path).await.expect("rewrite");

            let confirmation = launch
                .resume()
                .await
                .expect("revives in the descriptor's cwd");
            let revived = revived_id(&confirmation);
            let cfg = read_runner_config(&launch.run_paths_under(&elsewhere, &revived).run_dir);
            assert_eq!(cfg.cwd, elsewhere);
            assert!(
                !launch
                    .run_paths_under(launch.cwd(), &revived)
                    .run_dir
                    .exists(),
                "the request cwd's async root must not hold the revived run"
            );
        }

        /// The reader's refusals, each driven through `action: "resume"` with pi's exact
        /// sentences (`async-resume.ts:566`, `subagent-executor.ts:1493`, `:2060`, and the
        /// reader's own `Invalid …` family) — and none of them revives anything: the session's
        /// one capacity slot stays the SOURCE run's.
        #[tokio::test]
        async fn a_revive_refuses_a_descriptor_that_is_missing_foreign_or_malformed() {
            let launch = launch("descriptor-refusals", "worker").await;
            launch.settle(Some(launch.cwd().to_path_buf())).await;
            let path = launch.descriptor_path();
            let pristine = std::fs::read(&path).expect("the launch wrote it");
            let mutate = |edit: &dyn Fn(&mut serde_json::Value)| {
                let mut doc: serde_json::Value =
                    serde_json::from_slice(&pristine).expect("pristine JSON");
                edit(&mut doc);
                std::fs::write(&path, serde_json::to_vec_pretty(&doc).expect("JSON"))
                    .expect("rewrite descriptor");
            };

            mutate(&|doc| doc["agent"] = serde_json::json!("other"));
            assert_eq!(
                launch.resume().await.expect_err("agent mismatch"),
                format!(
                    "Async run '{}' has a recovery descriptor for 'other', not 'worker'.",
                    launch.run_id
                )
            );

            mutate(&|doc| doc["sourceRunId"] = serde_json::json!("run-other"));
            let foreign = launch.resume().await.expect_err("source-run mismatch");
            assert!(foreign.contains("different source run"), "{foreign}");

            mutate(&|doc| doc["version"] = serde_json::json!(2));
            let version = launch.resume().await.expect_err("version 2");
            assert!(version.contains("version 2"), "{version}");

            mutate(&|doc| doc["bogus"] = serde_json::json!(1));
            let unknown = launch.resume().await.expect_err("unknown key");
            assert!(unknown.contains("bogus"), "{unknown}");

            std::fs::remove_file(&path).expect("delete descriptor");
            assert_eq!(
                launch.resume().await.expect_err("missing descriptor"),
                format!(
                    "Async child '{}' is missing its required run fan-out recovery identity. \
                     Start a new run instead.",
                    launch.run_id
                )
            );

            let owner = only_capacity_owner(launch.cwd(), launch.session).await;
            assert_eq!(
                owner.run_id, launch.run_id,
                "a refused revive transfers nothing: the slot is still the source's"
            );
            assert_eq!(owner.generation, 0);
        }

        /// pi `async-execution.ts:2051-2053`: a descriptor that cannot be persisted FAILS THE
        /// LAUNCH — no `runner-config.json`, no process, and the capacity slot rolled back like
        /// the two fallible steps after it. Forced by pre-creating the descriptor path as a
        /// DIRECTORY, so the atomic rename fails; driven through `spawn_background_steps` with a
        /// caller-minted run id (the production entry point `spawn_background` delegates to),
        /// because that is the only way to know the run directory before the call.
        #[tokio::test]
        async fn a_descriptor_that_cannot_be_written_fails_the_launch_before_any_config_or_process()
        {
            let dir = tempfile::tempdir().expect("tempdir");
            write_persona(dir.path(), "worker", NARROW_MD);
            let executor = SubagentExecutor::new();
            executor.set_host_services(Arc::new(FixedSessionHost("descriptor-write-failure")));
            {
                let mut cfg = executor.config_cell().lock().await;
                cfg.roots = Roots::sandboxed(dir.path());
                cfg.max_active_async_runs_per_session = Some(1);
                cfg.spawn_command = Some(SpawnCommand {
                    binary: PathBuf::from("true"),
                    base_args: Vec::new(),
                });
            }
            let roots = Roots::sandboxed(dir.path());
            let run_id = RunId::new();
            let run_paths = RunPaths::for_run(
                &default_async_root_in(&roots, dir.path()),
                &default_results_dir_in(&roots, dir.path()),
                &run_id,
            );
            std::fs::create_dir_all(RunDir::for_existing(&run_paths.run_dir).recovery_descriptor())
                .expect("occupy the descriptor path with a directory");
            let agent = executor
                .resolve_agent(dir.path(), "worker", AgentReadScope::Both, &roots)
                .expect("the persona resolves");
            let persona = crate::exec::resolve_step_agent_config(&agent);
            let step = SingleStepSpec {
                machine: None,
                agent: "worker".to_string(),
                task: "do the thing".to_string(),
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
                fast: None,
                reads: None,
                acceptance: None,
                skills: None,
                session_dir: None,
                context: Some(ContextMode::Fresh),
                agent_scope: None,
            };

            let error = executor
                .spawn_background_steps(
                    dir.path(),
                    BackgroundStepsSpec {
                        usage_budget: None,
                        turn_budget: None,
                        permission_rules: None,
                        transfer_from: None,
                        revival_lease: None,
                        thinking_ceiling: None,
                        capability_ceiling: None,
                        model_origin: None,
                        steps: vec![RunnerStep::SingleStep(step)],
                        mode: RunMode::Single,
                        session_file: None,
                        resolved_agents: BTreeMap::from([("worker".to_string(), persona)]),
                        original_task: "do the thing".to_string(),
                        chain_dir: None,
                        control: None,
                        include_progress: None,
                        run_id: run_id.clone(),
                        timeout_ms: None,
                        share: None,
                        artifacts_dir: None,
                        artifact_config: ArtifactConfig::default(),
                    },
                )
                .await
                .expect_err("a launch whose descriptor cannot be persisted must fail");
            assert!(
                matches!(
                    error,
                    SubagentError::RecoveryDescriptor(RecoveryDescriptorError::Persist { .. })
                ),
                "{error:?}"
            );
            assert!(
                error.to_string().starts_with(&format!(
                    "Failed to persist async recovery descriptor for '{run_id}': "
                )),
                "pi `:2053`'s sentence: {error}"
            );
            assert!(
                !run_paths.run_dir.join("runner-config.json").exists(),
                "the descriptor is written BEFORE runner-config.json, so a failed one leaves none"
            );
            assert_eq!(
                capacity_slots(dir.path(), "descriptor-write-failure").await,
                0,
                "the claim is rolled back on this early return like every other past it"
            );
        }

        /// Builds the `BackgroundStepsSpec` this module's process-terminal tests launch with — a
        /// single step against the `worker` persona, nothing else set.
        fn process_terminal_launch_spec(
            run_id: RunId,
            persona: ResolvedAgentPersona,
        ) -> BackgroundStepsSpec {
            BackgroundStepsSpec {
                usage_budget: None,
                turn_budget: None,
                permission_rules: None,
                transfer_from: None,
                revival_lease: None,
                thinking_ceiling: None,
                capability_ceiling: None,
                model_origin: None,
                steps: vec![RunnerStep::SingleStep(SingleStepSpec {
                    machine: None,
                    agent: "worker".to_string(),
                    task: "do the thing".to_string(),
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
                    fast: None,
                    reads: None,
                    acceptance: None,
                    skills: None,
                    session_dir: None,
                    context: Some(ContextMode::Fresh),
                    agent_scope: None,
                })],
                mode: RunMode::Single,
                session_file: None,
                resolved_agents: BTreeMap::from([("worker".to_string(), persona)]),
                original_task: "do the thing".to_string(),
                chain_dir: None,
                control: None,
                include_progress: None,
                run_id,
                timeout_ms: None,
                share: None,
                artifacts_dir: None,
                artifact_config: ArtifactConfig::default(),
            }
        }

        /// P1 + P2 — pi `async-execution.ts:707` (`const runnerProcessInstanceId = randomUUID();`),
        /// `:710` (carried into the child's launch config) and `:851`
        /// (`initializeProcessTerminal`).
        ///
        /// THE USER ACTION: an agent delegates a background run, the runner is SIGKILLed a moment
        /// later, and the agent asks what happened. Everything that can answer that question is
        /// keyed on ONE value minted here, before the process exists — so this test asserts that
        /// the orchestrator really mints it, really writes it onto the runner's config, and really
        /// establishes both lifecycle artifacts BEFORE the spawn. Without the last part the runner
        /// could reach its own candidate write first and the launch would clobber it.
        #[tokio::test]
        async fn a_launch_mints_one_identity_and_establishes_both_lifecycle_artifacts() {
            let dir = tempfile::tempdir().expect("tempdir");
            write_persona(dir.path(), "worker", NARROW_MD);
            let executor = SubagentExecutor::new();
            executor.set_host_services(Arc::new(FixedSessionHost("process-terminal-launch")));
            {
                let mut cfg = executor.config_cell().lock().await;
                cfg.roots = Roots::sandboxed(dir.path());
                cfg.spawn_command = Some(SpawnCommand {
                    binary: PathBuf::from("true"),
                    base_args: Vec::new(),
                });
            }
            let roots = Roots::sandboxed(dir.path());
            let run_id = RunId::new();
            let run_paths = RunPaths::for_run(
                &default_async_root_in(&roots, dir.path()),
                &default_results_dir_in(&roots, dir.path()),
                &run_id,
            );
            let agent = executor
                .resolve_agent(dir.path(), "worker", AgentReadScope::Both, &roots)
                .expect("the persona resolves");
            let persona = crate::exec::resolve_step_agent_config(&agent);
            executor
                .spawn_background_steps(
                    dir.path(),
                    process_terminal_launch_spec(run_id.clone(), persona),
                )
                .await
                .expect("the launch succeeds");

            let run_dir = RunDir::for_existing(&run_paths.run_dir);
            let config = read_runner_config(&run_paths.run_dir);
            let minted = config
                .runner_process_instance_id
                .clone()
                .expect("the orchestrator mints an identity and carries it to the runner");

            // The candidate: 0600, this run, this identity, and EMPTY — the emptiness is what
            // makes a runner that dies before declaring anything distinguishable from one that
            // finished (see `initialize_process_terminal`).
            let candidate =
                crate::background::process_terminal::read_process_terminal_candidate(&run_dir)
                    .await
                    .expect("the candidate reads")
                    .expect("the launch wrote a candidate before spawning the runner");
            assert_eq!(candidate.run_id, run_id);
            assert_eq!(candidate.runner_process_instance_id, minted);
            assert!(candidate.writers.is_empty(), "{candidate:?}");
            assert!(candidate.expected_writers.is_none(), "{candidate:?}");

            // The pending sidecar, carrying the SAME identity — the value every later reader
            // matches a proof against.
            let proof = crate::background::process_terminal::read_process_terminal(
                &run_dir,
                crate::background::process_terminal::ProofExpectation::new(&run_id, &minted),
            )
            .await
            .expect("the launch wrote a pending proof");
            assert_eq!(
                proof.state(),
                crate::background::process_terminal::ProcessTerminalState::Pending,
                "a launch has observed nothing yet: {proof:?}"
            );
            assert_eq!(*proof.runner_process_instance_id(), minted);
        }

        /// The other half of P2's placement: establishing ownership is a FALLIBLE PRE-SPAWN step,
        /// so a launch that cannot write its lifecycle artifacts is REFUSED and its capacity claim
        /// rolled back — upstream terminates the runner and fails the launch on the same error
        /// (`async-execution.ts:853-857`), which cyrup can do more cheaply because no process
        /// exists yet.
        ///
        /// Forced the same way the recovery-descriptor test forces its own failure: the candidate
        /// path is pre-occupied by a DIRECTORY, so the atomic rename cannot land.
        #[tokio::test]
        async fn a_launch_that_cannot_establish_ownership_is_refused_and_rolls_back_its_slot() {
            let dir = tempfile::tempdir().expect("tempdir");
            write_persona(dir.path(), "worker", NARROW_MD);
            let executor = SubagentExecutor::new();
            executor.set_host_services(Arc::new(FixedSessionHost("process-terminal-refusal")));
            {
                let mut cfg = executor.config_cell().lock().await;
                cfg.roots = Roots::sandboxed(dir.path());
                cfg.max_active_async_runs_per_session = Some(1);
                cfg.spawn_command = Some(SpawnCommand {
                    binary: PathBuf::from("true"),
                    base_args: Vec::new(),
                });
            }
            let roots = Roots::sandboxed(dir.path());
            let run_id = RunId::new();
            let run_paths = RunPaths::for_run(
                &default_async_root_in(&roots, dir.path()),
                &default_results_dir_in(&roots, dir.path()),
                &run_id,
            );
            std::fs::create_dir_all(
                RunDir::for_existing(&run_paths.run_dir).process_terminal_candidate(),
            )
            .expect("occupy the candidate path with a directory");
            let agent = executor
                .resolve_agent(dir.path(), "worker", AgentReadScope::Both, &roots)
                .expect("the persona resolves");
            let persona = crate::exec::resolve_step_agent_config(&agent);

            let error = executor
                .spawn_background_steps(
                    dir.path(),
                    process_terminal_launch_spec(run_id.clone(), persona),
                )
                .await
                .expect_err("a launch that cannot establish ownership must fail");
            assert!(matches!(error, SubagentError::Spawn(_)), "{error:?}");
            assert_eq!(
                capacity_slots(dir.path(), "process-terminal-refusal").await,
                0,
                "the claim is rolled back on this early return like every other past it"
            );
            assert!(
                !RunDir::for_existing(&run_paths.run_dir)
                    .process_terminal()
                    .exists(),
                "a refused launch leaves no proof for a reader to trust"
            );
        }

        /// pi `subagent-executor.ts:1894-1905`: when discovery no longer finds the agent, the
        /// revive synthesises the persona from the descriptor instead of refusing. `courier` is
        /// not a builtin, so deleting its file leaves discovery with nothing.
        #[tokio::test]
        async fn a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona() {
            let launch = launch("descriptor-synthesised", "courier").await;
            launch.settle(Some(launch.cwd().to_path_buf())).await;
            let file = launch
                .cwd()
                .join(".cyrup")
                .join("agents")
                .join("courier.md");
            std::fs::remove_file(&file).expect("delete the persona file");

            let confirmation = launch
                .resume()
                .await
                .expect("the descriptor is the contract; the file is not required");
            let cfg = read_runner_config(
                &launch
                    .run_paths_under(launch.cwd(), &revived_id(&confirmation))
                    .run_dir,
            );
            let persona = cfg
                .resolved_agents
                .get("courier")
                .expect("the synthesised persona is in the map");
            assert_eq!(persona.name, "courier");
            assert_eq!(persona.tools, narrow_tools());
            assert_eq!(persona.exclude_tools, vec!["bash".to_string()]);
            assert_eq!(persona.system_prompt_body, "Launch body.");
            assert_eq!(persona.system_prompt_mode, SystemPromptMode::Replace);
            assert_eq!(persona.max_subagent_depth, Some(2));
            assert_eq!(persona.tool_budget, Some(launch_tool_budget()));
            assert!(persona.inherit_project_context && persona.inherit_skills);
            assert_eq!(
                persona.subagent_only_extensions,
                vec!["./child-only.ts".to_string()]
            );
            assert_eq!(persona.file_path.as_deref(), Some(file.as_path()));
            assert!(
                persona.fallback_models.is_empty(),
                "nothing the descriptor lacks is invented"
            );
        }

        /// pi `storedOrigin` (`model-resolution.ts:382`, fed by `modelOrigin:
        /// recoveryDescriptor?.modelOrigin`, `subagent-executor.ts:2149`): a run whose model was
        /// INHERITED from the launching session revives on that same model — pinned by the
        /// overlay, so neither the widened file's model nor the REVIVING session's leaks in — and
        /// its revived descriptor still says `inherited` with `modelOverrideFromParent`, rather
        /// than the `configured` a re-derivation over the pinned persona would produce. Without
        /// the carry a second revive would read a different origin than the first.
        #[tokio::test]
        async fn a_revive_keeps_the_stored_model_origin_rather_than_re_deriving_it() {
            let session = "descriptor-stored-origin";
            let host = InheritingHost::new(session, "session/launch-model");
            let launch = launch_with(LaunchSpec {
                host: host.clone(),
                persona: NARROW_MD.replace("model: fixture/persona-model\n", ""),
                model_override: None,
                ..LaunchSpec::standard(session, "worker")
            })
            .await;
            let written = RecoveryDescriptor::read(&launch.descriptor_path())
                .await
                .expect("readable")
                .expect("present");
            assert_eq!(
                written.model.as_ref().map(ModelId::as_str),
                Some("session/launch-model")
            );
            assert_eq!(written.model_origin, ModelOrigin::Inherited);
            assert_eq!(written.model_override_from_parent, Some(true));

            launch.settle(Some(launch.cwd().to_path_buf())).await;
            // The file now declares a model of its own and the reviving session runs another:
            // neither may displace the launch's.
            write_persona(launch.cwd(), "worker", WIDE_MD);
            host.switch_model("session/reviving-model");
            let confirmation = launch.resume().await.expect("revives");
            let revived_paths = launch.run_paths_under(launch.cwd(), &revived_id(&confirmation));
            let cfg = read_runner_config(&revived_paths.run_dir);
            let step = single_step(&cfg);
            let persona = cfg
                .resolved_agents
                .get("worker")
                .expect("the revived persona map carries the agent");
            assert_eq!(
                step.model, None,
                "an inherited model is not a per-call override"
            );
            assert_eq!(
                persona.model.as_ref().map(ModelId::as_str),
                Some("session/launch-model"),
                "pinned: not the file's, not the reviving session's"
            );
            assert_eq!(
                cfg.inherited_session_model.as_ref().map(ModelId::as_str),
                Some("session/reviving-model"),
                "the reviving session's rung is carried, and outranked by the pinned persona"
            );
            let revived_descriptor = RecoveryDescriptor::read(
                &RunDir::for_existing(&revived_paths.run_dir).recovery_descriptor(),
            )
            .await
            .expect("readable")
            .expect("written per revive");
            assert_eq!(
                revived_descriptor.model.as_ref().map(ModelId::as_str),
                Some("session/launch-model")
            );
            assert_eq!(
                revived_descriptor.model_origin,
                ModelOrigin::Inherited,
                "pi storedOrigin: carried, not re-derived over the pinned persona"
            );
            assert_eq!(revived_descriptor.model_override_from_parent, Some(true));
        }

        /// The retention reader against the PRODUCTION writer — pi `hasResumableContract`
        /// (`async-retention.ts:196-203`); the spec's "a descriptor written by
        /// `spawn_background`, not a hand-written map". The production descriptor that carries a
        /// `sessionFile` is the REVIVED run's (its transcript is the one it was seeded from; a
        /// fresh in-crate launch has no persisted parent session to fork). The status the scan
        /// reads is a fixture — terminal, past the window, with no transcript of its own, the
        /// exact shape whose verdict rests on the descriptor alone — while every byte of the
        /// descriptor is the writer's.
        #[tokio::test]
        async fn a_production_written_descriptor_s_transcript_is_a_resumable_contract() {
            use crate::background::async_retention::{
                ASYNC_RETENTION_MS, ASYNC_RETENTION_TOMBSTONE_GRACE_MS, RetentionDecision,
                RunScanRequest, SkipReason, decide, scan_run_candidates,
            };

            let launch = launch("descriptor-retention", "worker").await;
            let session_file = launch.settle(Some(launch.cwd().to_path_buf())).await;
            let confirmation = launch.resume().await.expect("revives");
            let revived = revived_id(&confirmation);
            let revived_paths = launch.run_paths_under(launch.cwd(), &revived);
            let descriptor_path =
                RunDir::for_existing(&revived_paths.run_dir).recovery_descriptor();
            let written = RecoveryDescriptor::read(&descriptor_path)
                .await
                .expect("readable")
                .expect("the revive wrote one");
            assert_eq!(
                written.session_file.as_deref(),
                Some(session_file.as_path()),
                "row 6 through production: the transcript the revive was seeded from"
            );

            // A terminal status past the window with NO transcript of its own, for each run the
            // scan will meet.
            let now = 1_900_000_000_000i64;
            let old = now - ASYNC_RETENTION_MS - 60_000;
            let settle_old = |run_id: RunId, status_path: PathBuf| async move {
                let mut status = RunStatus::queued(run_id, RunMode::Single, Some(7));
                status.state = RunState::Complete;
                status.started_at = old;
                status.last_update = old;
                status.ended_at = Some(old);
                write_atomic_json(&status_path, &status)
                    .await
                    .expect("write the terminal status fixture");
            };
            settle_old(revived.clone(), revived_paths.status.clone()).await;
            // pi `:201` — a descriptor for ANOTHER source run: the writer's own bytes, copied
            // verbatim under a second run.
            let foreign = RunId::new();
            let foreign_paths = launch.run_paths_under(launch.cwd(), &foreign);
            std::fs::create_dir_all(&foreign_paths.run_dir).expect("mkdir foreign run dir");
            std::fs::copy(
                &descriptor_path,
                RunDir::for_existing(&foreign_paths.run_dir).recovery_descriptor(),
            )
            .expect("copy the production descriptor");
            settle_old(foreign.clone(), foreign_paths.status.clone()).await;
            // The SOURCE run's launch descriptor records no sessionFile (a fresh context), and
            // this status no longer carries the step transcript `settle` gave it.
            settle_old(launch.run_id.clone(), launch.source_paths.status.clone()).await;

            let roots = Roots::sandboxed(launch.cwd());
            let async_root = default_async_root_in(&roots, launch.cwd());
            let results_dir = default_results_dir_in(&roots, launch.cwd());
            let scan = || async {
                scan_run_candidates(RunScanRequest {
                    async_root: &async_root,
                    results_dir: &results_dir,
                    maintenance_root: None,
                    after: None,
                    budget: 100,
                })
                .await
                .expect("the scan reads the sandbox's async root")
            };
            let none = BTreeSet::new();
            let verdict = |window: &crate::background::async_retention::RunScanWindow,
                           run_id: &RunId| {
                let facts = window
                    .candidates
                    .iter()
                    .find(|facts| facts.dir_name == run_id.as_str())
                    .unwrap_or_else(|| panic!("{run_id} was not a candidate"));
                decide(
                    facts,
                    now,
                    ASYNC_RETENTION_MS,
                    ASYNC_RETENTION_TOMBSTONE_GRACE_MS,
                    &none,
                    &none,
                )
            };

            let window = scan().await;
            assert_eq!(
                verdict(&window, &revived),
                RetentionDecision::Keep(SkipReason::Resumable),
                "the writer's sessionFile exists: a resumable contract"
            );
            assert_eq!(
                verdict(&window, &foreign),
                RetentionDecision::Keep(SkipReason::Resumable),
                "pi `:201`: a descriptor for another source run cannot be disproved"
            );
            assert_eq!(
                verdict(&window, &launch.run_id),
                RetentionDecision::Tombstone,
                "a fresh launch's descriptor records no sessionFile, and nothing else protects it"
            );

            std::fs::remove_file(&session_file).expect("delete the transcript");
            let window = scan().await;
            assert_eq!(
                verdict(&window, &revived),
                RetentionDecision::Tombstone,
                "a descriptor whose transcript is gone protects nothing"
            );
        }

        /// Rows 13/43 — the only two whose landing is an env var on the revived RUNNER process
        /// (pi `thinkingCeiling`/`capabilityCeiling` on the revived launch,
        /// `subagent-executor.ts:2151,2183`). The revive's hop-1 command is a script that prints
        /// its environment to stdout, which the detached spawn redirects to
        /// `runner.stdout.log` — a real process, still no runner, no model, no network.
        ///
        /// A ceiling REGISTERED for the session is the one launch-time source that needs no env
        /// mutation (the crate is `forbid(unsafe_code)`), so row 43 is written by production.
        /// Row 13's only source is the environment, which this test cannot set; the writer's
        /// projection of a `Some` is `recovery_descriptor::tests::for_single_launch_projects_
        /// every_field…`, and the LANDING is proven here by stamping one onto the persisted
        /// contract before the resume.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_revive_re_applies_the_launch_s_ceilings_to_its_runner_s_environment() {
            use crate::exec::capability_ceiling::{
                CAPABILITY_CEILING_ENV, CAPABILITY_CEILING_VERSION, ResolvedCapabilityCeiling,
                decode_capability_ceiling, register_capability_ceiling,
            };
            use crate::exec::thinking_ceiling::THINKING_CEILING_ENV;

            let session = "descriptor-ceilings";
            let _registered = register_capability_ceiling(
                session,
                "test-policy",
                &serde_json::json!({"allowedAgents": ["worker"], "denyExtensions": true}),
            )
            .expect("registers");
            let launch = launch(session, "worker").await;
            let expected_ceiling = ResolvedCapabilityCeiling {
                version: CAPABILITY_CEILING_VERSION,
                allowed_tools: None,
                allowed_agents: Some(vec!["worker".to_string()]),
                deny_extensions: true,
                sources: vec!["test-policy".to_string()],
            };
            let path = launch.descriptor_path();
            let mut written = RecoveryDescriptor::read(&path)
                .await
                .expect("readable")
                .expect("present");
            assert_eq!(
                written.capability_ceiling,
                Some(expected_ceiling.clone()),
                "row 43: the launching session's registered ceiling is persisted"
            );
            assert_eq!(
                written.thinking_ceiling, None,
                "this process inherits no CYRUP_SUBAGENT_THINKING_CEILING"
            );
            written.thinking_ceiling = Some("low".to_string());
            written.write(&path).await.expect("rewrite");
            launch.settle(Some(launch.cwd().to_path_buf())).await;

            let script = launch.cwd().join("dump-env.sh");
            std::fs::write(&script, "#!/bin/sh\nexec env\n").expect("write script");
            std::fs::set_permissions(
                &script,
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
            )
            .expect("chmod");
            {
                let mut cfg = launch.executor.config_cell().lock().await;
                cfg.spawn_command = Some(SpawnCommand {
                    binary: script,
                    base_args: Vec::new(),
                });
            }

            let confirmation = launch.resume().await.expect("revives");
            let revived_paths = launch.run_paths_under(launch.cwd(), &revived_id(&confirmation));
            let mut dump = String::new();
            for _ in 0..200 {
                dump =
                    std::fs::read_to_string(&revived_paths.runner_stdout_log).unwrap_or_default();
                if dump.contains(&format!("{CAPABILITY_CEILING_ENV}=")) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            let env_value = |key: &str| {
                dump.lines()
                    .find_map(|line| line.strip_prefix(&format!("{key}=")))
                    .map(str::to_string)
            };
            assert_eq!(
                env_value(THINKING_CEILING_ENV).as_deref(),
                Some("low"),
                "row 13 lands on the runner's env: {dump}"
            );
            let encoded = env_value(CAPABILITY_CEILING_ENV)
                .unwrap_or_else(|| panic!("row 43 lands on the runner's env: {dump}"));
            assert_eq!(
                decode_capability_ceiling(Some(&encoded)).expect("decodes"),
                Some(expected_ceiling),
                "the revive's ceiling is the launch's, intersected with the reviver's own"
            );
        }
    }
}
