//! The `subagent` [`Tool`] itself: its construction and its [`cyrup_core::Tool`] impl.

pub(crate) mod mission;
pub(crate) mod params;
pub(crate) mod routing;
pub(crate) mod schema;
pub(crate) mod task_items;
pub(crate) mod text;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::error::SubagentError;
use crate::spawn::depth::resolve_effective_depth;
use crate::spawn::parallel::DispatchGuard;
use crate::extension::TOOL_NAME;
use crate::extension::executor::SubagentExecutor;
use crate::extension::tool::mission::{
    attach_mission_to_tool_outcome, duplicate_subagent_call_text,
    prepare_mission_binding_for_dispatch,
};
use crate::extension::tool::params::{
    normalize_public_subagent_execution, validate_execution_acceptance, SubagentToolParams,
};
use crate::extension::tool::schema::subagent_tool_parameters;
use crate::extension::tool::task_items::count_requested_subagent_spawns;
use crate::extension::tool::text::{CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION, SUBAGENT_TOOL_DESCRIPTION};

/// The `subagent` LLM-facing tool (R-SA-128). Dispatches over pi's full discriminated-union
/// parameter surface (C8): a present `action` routes to a management/control action, `tasks[]` to
/// top-level PARALLEL, `chain[]` to CHAIN, and the bare `{agent, task?}` shape to SINGLE — the SAME
/// [`SubagentExecutor`] `execute_command`'s slash-command dispatch uses (R-SA-130). All four
/// families are wired end-to-end: the SINGLE shape and read-only `doctor` (T0.5), the
/// management CRUD (`list`/`get`/`models`/`create`/`update`/`delete`, C3) and background-control
/// actions (`status`/`interrupt`/`resume`/`append-step`, C5) via [`Self::route_action`], and the
/// tool-driven PARALLEL/CHAIN routing via [`Self::route_parallel_mode`]/[`Self::route_chain_mode`]
/// (P1) — each resolving the REAL named persona (T0.1/C13) over real child processes, never a stub.
///
/// `cwd` is captured at CONSTRUCTION time (mirroring `cyrup_tools::tools::bash::BashTool::new`'s
/// established codebase convention: `cyrup_core::Tool::execute`'s signature carries no `HostCtx`,
/// so every built-in tool that needs the session's working directory captures it once, at
/// registration time, rather than re-deriving it from process-global state on every call).
pub struct SubagentTool {
    executor: Arc<SubagentExecutor>,
    cwd: PathBuf,
    parameters: serde_json::Value,
    /// The mode-specific tool description (T6, pi `fanout-child.ts:159-163` @v0.34.0; `:177-181` at
    /// v0.43.0): the root orchestrator
    /// advertises [`SUBAGENT_TOOL_DESCRIPTION`]; a fanout child advertises
    /// [`CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION`] instead, so the model inside a restricted child is
    /// told up front which management actions are blocked rather than only discovering the block via
    /// a runtime [`ToolError`].
    ///
    /// SUBA-025 — OWNED rather than `&'static str` since the description became RESOLVED: with
    /// `toolDescriptionMode: "custom"` its bytes come off disk at registration
    /// ([`crate::registration::tool_description::build_subagent_tool_description`]), so there is no
    /// static to borrow. The two built-in arms still hand over a `&'static str`; only the custom
    /// arm allocates, and only once per registration.
    description: String,
    /// Whether the mutating management actions (`create`/`update`/`delete`) are permitted (T6). The
    /// root orchestrator tool sets this `true`; a fanout-child's restricted tool sets it `false`, so
    /// a child can list/get/delegate but cannot rewrite the parent's agent config on disk (pi
    /// `fanout-child.ts` `allowMutatingManagementActions: false`).
    allow_mutating_management: bool,
    /// R-SA-069 single-dispatch guard (pi `state.subagentInProgress`,
    /// `subagent-executor.ts:5327-5348` `executeWithSingleDispatchGuard`): rejects a second
    /// non-`action` subagent call arriving while one is still in flight from this tool instance,
    /// WITHOUT affecting the intentional parallel-mode fan-out that happens *inside* one accepted
    /// dispatch. `action` calls (management/control) bypass this guard entirely, matching pi's
    /// `if (params.action) return execute(...)` early return before the flag check.
    dispatch_guard: DispatchGuard,
    /// The orchestrator's watchdog runtime (pi `deps.watchdog`, `subagent-executor.ts:300`), which
    /// the four `watchdog.*` actions read and write (`:4432`). `None` for a tool built outside the
    /// extension (this crate's own unit tests), where those actions then report
    /// `Subagent watchdog runtime is unavailable.` exactly as upstream's `!runtime` branch does.
    watchdog: Option<Arc<crate::watchdog::runtime::MainWatchdogRuntime>>,
}
impl SubagentTool {
    #[must_use]
    pub(crate) fn new(executor: Arc<SubagentExecutor>, cwd: PathBuf) -> Self {
        Self {
            executor,
            cwd,
            parameters: subagent_tool_parameters(),
            description: SUBAGENT_TOOL_DESCRIPTION.to_string(),
            allow_mutating_management: true,
            dispatch_guard: DispatchGuard::new(),
            watchdog: None,
        }
    }

    /// SUBA-025 — override the advertised description with the RESOLVED one (pi
    /// `description: buildSubagentToolDescription(config)`, `extension/index.ts:458` @v0.34.0 /
    /// `:540` @v0.43.0).
    ///
    /// A builder rather than a `new` parameter because resolution needs the extension's loaded
    /// config, which only [`cyrup_ext::NativeExtension::init`] has — every other construction site (this
    /// crate's own tests, [`crate::extension::SubagentsExtension::subagent_tool`]) keeps pi's `full` default, which
    /// is exactly what `buildSubagentToolDescription({})` returns.
    #[must_use]
    pub(crate) fn with_description(mut self, description: String) -> Self {
        self.description = description;
        self
    }

    /// Bind the orchestrator's watchdog runtime (pi hands it to the executor as `deps.watchdog`,
    /// `extension/index.ts:438`). Without it the `watchdog.*` actions are inert.
    #[must_use]
    pub(crate) fn with_watchdog(
        mut self,
        watchdog: Arc<crate::watchdog::runtime::MainWatchdogRuntime>,
    ) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// The restricted child-safe tool (T6, pi `fanout-child.ts`): identical to [`SubagentTool::new`]
    /// except the agent-config mutation actions (`create`/`update`/`delete`) are blocked, and the
    /// advertised description is pi's exact 3-line child-safe text
    /// (`extension/fanout-child.ts:177-181` @v0.43.0) instead of the full orchestrator prompt.
    #[must_use]
    pub(crate) fn new_child_safe(executor: Arc<SubagentExecutor>, cwd: PathBuf) -> Self {
        Self {
            allow_mutating_management: false,
            description: CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION.to_string(),
            ..Self::new(executor, cwd)
        }
    }

    /// The executor this tool dispatches through.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn executor(&self) -> &Arc<SubagentExecutor> {
        &self.executor
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        &self.description
    }

    /// pi `label: "Subagent"` (`pi-subagents/src/extension/index.ts:598`, and the identical
    /// child-safe variant at `src/extension/fanout-child.ts:177`).
    ///
    /// Not optional in practice: `Tool::label`'s default is `None`
    /// (`cyrup-core/src/tool.rs:103-106`) and the transcript's tool-row renderer falls back to
    /// `name()` when it gets one, so omitting this made the row read `subagent` rather than
    /// "Subagent". Every sibling tool in this crate already overrides it — [`crate::extension::WaitTool`],
    /// `SubagentSupervisorTool`, `StructuredOutputTool` — which is precisely why the omission was
    /// invisible sitting next to them.
    fn label(&self) -> Option<&str> {
        Some("Subagent")
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: SubagentToolParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid subagent tool call: {e}")))?;

        // Observe the full parsed pi-union once, keeping every field live under the workspace's
        // `-D warnings` (`dead_code`) without a non-`#[cfg(test)]` `#[allow]` — the same liveness
        // pattern the per-item `ToolTaskItem::provided_keys` calls above use.
        //
        // The list this comment used to carry ("fields no dispatch arm consumes yet: output/
        // outputMode/skill/acceptance/artifacts/includeProgress/share/sessionDir/clarify/control/
        // timeoutMs/maxRuntimeMs/chainDir, wire-ups are Tiers 3/5") is GONE because it went stale
        // — every one of those is wired now — and a stale inventory here is actively harmful: it
        // reads as license for the next unwired param to sit unnoticed.
        //
        // Note the cost this call carries. Suppressing `dead_code` also suppresses the only
        // AUTOMATIC signal that an advertised param reaches no dispatch arm, which is how
        // `chainDir` stayed silently dropped. The replacement detector is deliberate, not free:
        // `every_advertised_schema_property_is_read_outside_provided_keys` excises this very
        // function and re-checks the whole advertised set. If you add a field here to quiet a
        // warning, that test is what will stop you.
        let _ = parsed.provided_keys();

        // pi `resolveRequestedCwd(ctx.cwd, params.cwd)` (`subagent-executor.ts:2801`): resolved ONCE
        // up front and threaded into every dispatch arm below — management/control CRUD, the
        // background-control actions, AND execution (PARALLEL/CHAIN/SINGLE) all see the SAME
        // `effectiveCwd`/`requestCwd`, not this tool's construction-time `self.cwd` unconditionally.
        let effective_cwd = self.resolve_requested_cwd(parsed.cwd.as_deref());

        // R-SA-128 / C8 dispatch: the `subagent` tool is a discriminated union over pi's full
        // parameter surface. Mode is selected exactly as pi's `subagent-executor` selects it — a
        // present `action` is a management/control call; otherwise `tasks[]` is top-level PARALLEL,
        // `chain[]` is CHAIN, and the bare `{agent, task?}` shape is SINGLE. All four families route
        // to real execution (the management/control CRUD via `route_action`, and the tool-driven
        // PARALLEL/CHAIN via `route_parallel_mode`/`route_chain_mode`).
        // pi's PUBLIC execution boundary — `executor.executePublic(...)`, which is what BOTH
        // model-facing registrations call at v0.43.0 (`extension/index.ts:508,532` for the root
        // orchestrator and `extension/fanout-child.ts:184` for the child-safe fanout tool), in
        // place of the `executor.execute(...)` they called at v0.34.0. `Tool::execute` is cyrup's
        // single equivalent of that seam: both registrations are one `SubagentTool` differing only
        // in `description`/`allow_mutating_management`, so applying the normalization here applies
        // it to exactly the two surfaces upstream applies it to, and to nothing else. See
        // [`normalize_public_subagent_execution`] for what is and is not ported out of upstream's
        // `extension/public-execution.ts`.
        let action = normalize_public_subagent_execution(parsed.action.as_deref())?;
        if let Some(action) = action {
            return self.route_action(action, &parsed, &effective_cwd).await;
        }

        // R-SA-069 single-dispatch guard (pi `executeWithSingleDispatchGuard`,
        // `subagent-executor.ts:5327-5348`): a second non-`action` subagent call arriving while one
        // is still in flight from this tool instance is rejected outright (never queued), with pi's
        // exact text; the slot is released once this dispatch fully completes, including on error
        // (the RAII `DispatchToken`'s `Drop` — pi's `finally { subagentInProgress = false }`).
        let Some(_dispatch_token) = self.dispatch_guard.try_acquire() else {
            return Err(ToolError::new(duplicate_subagent_call_text()));
        };

        // pi `validateExecutionInput`'s mode-exclusivity gate (`subagent-executor.ts:1736-1754`,
        // `hasChain`/`hasTasks`/`hasSingle` computed at `2995-2997`): a mode is selected by a
        // NON-EMPTY `chain`/`tasks` array, not merely by the field being present — an explicit
        // `tasks: []` or `chain: []` MUST fall through to this "provide exactly one mode" error
        // rather than silently executing as an empty parallel run / empty chain.
        let has_chain = parsed.chain.as_ref().is_some_and(|c| !c.is_empty());
        let has_tasks = parsed.tasks.as_ref().is_some_and(|t| !t.is_empty());
        let has_single = !has_chain && !has_tasks && parsed.agent.is_some();
        if usize::from(has_chain) + usize::from(has_tasks) + usize::from(has_single) != 1 {
            return Err(ToolError::new(format!(
                "Provide exactly one mode. Agents: {}",
                self.discovered_agent_names_joined(&effective_cwd).await
            )));
        }

        // pi `validateExecutionInput`'s acceptance gate (`subagent-executor.ts:1757-1762`), in pi's
        // own position: immediately after the mode-exclusivity check and before agent resolution or
        // any spawn. Covers the top-level SINGLE `acceptance` AND every `tasks[]`/`chain[]` item's
        // own — SUBA-N04, since those items' policies now really do reach their children.
        let acceptance_errors = validate_execution_acceptance(&parsed);
        if !acceptance_errors.is_empty() {
            return Err(ToolError::new(acceptance_errors.join(" ")));
        }

        // pi `reserveSubagentSpawns` (`subagent-executor.ts:266-282`, called at `:3434-3441` right
        // after the mode is settled and before any `ExecutionContextData` is built): charge this
        // dispatch's worst-case spawn count against the SESSION-wide budget
        // (`config.maxSubagentSpawnsPerSession`, default 40) and reject the whole call — never a
        // partial fan-out — once the session has exhausted it. The budget is per SESSION, not per
        // turn, and the reservation is billed up front, so a run that fails later still counts.
        //
        // [CYRUP-DELTA — UNPORTED, not accepted] pi runs
        // `validateExecutionChainBindings` immediately BEFORE this
        // reserve; in this crate that validation lives inside `route_chain_mode`, so a structurally
        // invalid chain is billed here and rejected a moment later. Moving the reserve past the
        // routing call would instead bill each mode arm separately (and twice for a chain that
        // re-enters), which is the worse divergence; the over-charge only affects a call that was
        // going to error anyway.
        let cfg = self.executor.config_snapshot().await;
        // SUBA-002 follow-up — the DEPTH guard must precede the charge. pi checks the recursion
        // ceiling at `subagent-executor.ts:3297-3312`, well ahead of `reserveSubagentSpawns`
        // (`:3434-3441`), so a dispatch the ceiling will refuse never spends a spawn from the
        // per-SESSION budget. cyrup's own R-SA-055 guard lives one level down — inside
        // `run_foreground`/`spawn_background`/`run_or_background_graph`, i.e. strictly AFTER this
        // charge — so without this rung a depth-blocked call was billed and then rejected, and a
        // subagent pinned at max depth could drain its parent session's budget by repeatedly asking
        // for children it can never have. The downstream guards stay exactly as they are (they are
        // the SAFETY-CRITICAL ones, ahead of discovery/IO); this is a pure env+config read that
        // makes the ordering match pi's. It changes no charge COUNT: this remains the tool path's
        // one and only reserve.
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(ToolError::new(
                SubagentError::DepthExceeded {
                    current: depth.current_depth,
                    max: depth.max_depth,
                }
                .to_string(),
            ));
        }
        if let Err(limit_notice) = self.executor.reserve_subagent_spawns(
            count_requested_subagent_spawns(&parsed, &cfg),
            cfg.max_subagent_spawns_per_session,
        ) {
            return Err(ToolError::new(limit_notice));
        }

        // pi `canonicalizeExecutionParams` (`subagent-executor.ts:4923-4925`), in pi's own position:
        // after the mode is settled and the budget charged, immediately before the mode arm runs. An
        // alias is rewritten to the agent's canonical name HERE so every downstream surface (persona
        // maps, chain bookkeeping, status rows, result summaries) names the real agent; an ambiguous
        // alias aborts the whole dispatch.
        let canonicalized = self.canonicalize_execution_params(&parsed, &effective_cwd).await?;
        let parsed: &SubagentToolParams = canonicalized.as_ref().unwrap_or(&parsed);

        // DURABLE MISSIONS (pi `subagent-executor.ts:5100-5127` @v0.43.0): resolve or create the
        // mission BEFORE the run starts, then fold the settled result back onto it. Upstream wraps
        // its whole execution path in exactly this pair, and this is cyrup's single equivalent
        // seam — all three mode arms below settle through it.
        //
        // The error discipline is upstream's, not an invention: an EXPLICIT `mission`/`missionId`
        // makes a mission failure fatal to the call (the caller asked for mission tracking and did
        // not get it), while an automatic binding degrades to a non-fatal `details.missionWarning`
        // so mission bookkeeping can never take down a run that would otherwise have succeeded.
        let explicit_mission = parsed.mission_id.is_some() || parsed.mission.is_some();
        let mission_config = cfg.missions.clone();
        let (mission_binding, mission_warning) = prepare_mission_binding_for_dispatch(
            &parsed.mission_launch_params(),
            &effective_cwd,
            mission_config.as_ref(),
            self.executor.host_services().and_then(|s| s.session_id()).as_deref(),
            explicit_mission,
        )?;

        let outcome = if has_tasks {
            self.route_parallel_mode(parsed, &effective_cwd, cancel).await
        } else if has_chain {
            self.route_chain_mode(parsed, &effective_cwd, cancel).await
        } else {
            // C19: SINGLE mode is the one shape wired for live progress today — its foreground
            // child's NDJSON stream is folded and forwarded through `on_update` (`route_single` ->
            // `run_foreground_streaming`). The tool-driven PARALLEL/CHAIN shapes still surface
            // progress only on completion; streaming their fan-out is the remaining live-progress
            // work (their per-child folds would multiplex through the same
            // `SubagentUpdatePayload.progress[]`).
            self.route_single(parsed, &effective_cwd, on_update, cancel).await
        };

        attach_mission_to_tool_outcome(outcome, mission_binding.as_ref(), mission_warning, explicit_mission)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    /// T6 parity regression (pi `extension/fanout-child.ts:174-186` @v0.43.0): the fanout child's
    /// restricted tool must advertise pi's exact 3-line child-safe description — NOT the full
    /// orchestrator prompt — so the model inside a fanout child is told up front which actions are
    /// blocked. Pre-fix, `SubagentTool::description()` returned `SUBAGENT_TOOL_DESCRIPTION`
    /// unconditionally regardless of mode, so this assertion fails against the pre-fix behavior.
    ///
    /// The expected text is transcribed from `fanout-child.ts:178-180` @v0.43.0 and is byte-exact.
    /// It previously carried two divergences the old assertion pinned rather than caught: a `stop`
    /// in the allowed list that upstream has at NEITHER tag, and a missing eighth blocked verb
    /// `grant-spawn-budget` (with v0.34.0's "Agent config mutation actions" lead-in instead of
    /// v0.43.0's "Mutating management actions").
    /// A `SubagentToolParams` with every field absent — the struct has no `Default` (it is
    /// deserialize-only), so the empty JSON object is how a test builds one.
    fn empty_params() -> SubagentToolParams {
        serde_json::from_value(serde_json::json!({})).expect("an empty params object parses")
    }

    /// pi lists `watchdog.configure` in `MUTATING_MANAGEMENT_ACTIONS`
    /// (`subagent-executor.ts:151` @v0.43.0), so a fanout child may READ the watchdog's state but
    /// must not rewrite the parent's watchdog settings — the same rule the CRUD verbs get.
    #[tokio::test]
    async fn a_child_safe_tool_refuses_watchdog_configure_but_allows_the_read_only_verbs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = Arc::new(SubagentExecutor::new());
        let child_safe =
            SubagentTool::new_child_safe(Arc::clone(&executor), dir.path().to_path_buf());
        let refused = child_safe
            .route_watchdog_action("watchdog.configure", &empty_params(), dir.path())
            .expect_err("a child-safe tool must refuse the mutating watchdog verb");
        assert_eq!(
            refused.to_string(),
            "Action 'watchdog.configure' is not available from child-safe subagent fanout mode."
        );
        // The three read-only verbs still answer (with "runtime is unavailable", since this tool
        // has no watchdog bound) rather than being refused for being child-safe.
        for action in ["watchdog.status", "watchdog.check", "watchdog.recommend-model"] {
            let outcome = child_safe.route_watchdog_action(
                action,
                &empty_params(),
                dir.path(),
            );
            let text = match outcome {
                Ok(result) => format!("{:?}", result.content),
                Err(error) => error.to_string(),
            };
            assert!(
                !text.contains("child-safe subagent fanout mode"),
                "{action} must not be refused as child-safe: {text}"
            );
        }
        // The full tool is not restricted at all.
        let full = SubagentTool::new(executor, dir.path().to_path_buf());
        let full_outcome =
            full.route_watchdog_action("watchdog.configure", &empty_params(), dir.path());
        let full_text = match full_outcome {
            Ok(result) => format!("{:?}", result.content),
            Err(error) => error.to_string(),
        };
        assert!(!full_text.contains("child-safe subagent fanout mode"), "{full_text}");
    }

    /// Both variants of the subagent tool must carry pi's `label: "Subagent"`
    /// (`pi-subagents/src/extension/index.ts:598`, `src/extension/fanout-child.ts:177`).
    ///
    /// `Tool::label`'s default is `None` (`cyrup-core/src/tool.rs:103-106`) and the transcript's
    /// tool-row renderer falls back to `name()` on `None`, so before the fix the row read
    /// `subagent` — the raw wire name — instead of "Subagent". The omission was invisible because
    /// every sibling tool in this crate (`WaitTool`, `SubagentSupervisorTool`,
    /// `StructuredOutputTool`) does override it, so the impl looked complete next to them.
    #[test]
    fn both_subagent_tool_variants_carry_pis_subagent_label() {
        let executor = Arc::new(SubagentExecutor::new());
        let full = SubagentTool::new(executor.clone(), PathBuf::from("/tmp"));
        let child_safe = SubagentTool::new_child_safe(executor, PathBuf::from("/tmp"));

        assert_eq!(
            Tool::label(&full),
            Some("Subagent"),
            "a `None` label silently renders the tool row as the raw name `subagent`"
        );
        assert_eq!(Tool::label(&child_safe), Some("Subagent"));
        assert_ne!(
            Tool::label(&full).map(str::to_string),
            Some(Tool::name(&full).to_string()),
            "the label must be the display form, not the wire name the default falls back to"
        );
    }

}
