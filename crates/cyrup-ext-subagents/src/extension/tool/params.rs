//! [`SubagentToolParams`]: the tool call's parameter shape, its deserialization helpers and
//! its mode/validity resolution.

use cyrup_core::ToolError;

use crate::discovery::types::AgentReadScope;
use crate::exec::SingleResult;
use crate::fork_context::ContextMode;
use crate::registration::SubagentExtensionConfig;
use crate::extension::tool::text::BLANK_ACTION_REFUSAL;

/// The PUBLIC (model-facing) execution boundary — pi `normalizePublicSubagentExecution`
/// (`extension/public-execution.ts:26-71` @v0.43.0) folded together with the `action` normalization
/// its caller `executeWithSingleDispatchGuard` performs (`subagent-executor.ts:5327-5348`).
///
/// Returns the TRIMMED action to dispatch, or `None` to fall through to the execution shapes.
///
/// # What upstream's boundary does, and what is ported here
///
/// `executePublic` is not a rename of `execute`. It is the entry point of v0.43.0's **public
/// execution cutover**: the whole of `normalizePublicSubagentExecution` exists to REMOVE direct
/// execution from the model-facing surface in favour of a `workflowScript` string. Its checks split
/// cleanly in two.
///
/// **Ported (independent of the cutover).** An `action` that is present but blank after trimming is
/// refused outright (`public-execution.ts:28-30`), and an `action` that survives is TRIMMED before
/// dispatch (`subagent-executor.ts:5334-5335`), so `" status "` routes to `status`. Both halves
/// were divergences here, and one of them predates v0.43.0: at the ported v0.34.0 baseline
/// `executeWithSingleDispatchGuard`'s gate is the JS-truthiness test `if (requestParams.action)`
/// (`subagent-executor.ts:3594-3613` @v0.34.0), under which `action: ""` is FALSY and falls through to
/// the execution shapes — whereas cyrup's `Option<String>` made `Some("")` a present action and
/// answered `unknown subagent action ''`. Upstream never produces that error for a blank action at
/// either tag.
///
/// **[CYRUP-DELTA] on the refusal text.** Upstream's message ends "…or omit action and use
/// workflowScript."; [`BLANK_ACTION_REFUSAL`] names cyrup's own execution shapes instead, because
/// `workflowScript` is one of the unported cutover fields below and steering a model toward a
/// parameter this tool does not accept would be worse than the divergence it removes. Restore the
/// upstream sentence when `workflowScript` lands.
///
/// **Not ported (blocked on `workflowScript`).** Every remaining check in
/// `normalizePublicSubagentExecution` rejects a shape cyrup still supports and upstream deleted:
/// `clarify` (`:32-34`), top-level `resume` (`:35-37`), `tasks`/`chain`/`parallel`/`concurrency`/
/// `chainDir` (`:38-41`), the legacy `single`/`parallel`/`tasks`/`chain` action aliases (`:42-49`),
/// `schedule.create`'s workflowScript requirement (`:50-58`), the "workflowScript execution must
/// omit action" rule (`:59-61`), and the terminal "Direct execution was removed. Use workflowScript"
/// pair (`:64-69`). All of them presuppose the `workflowScript` runtime, of which this crate has
/// nothing — the identifier appears nowhere in it — so porting them in isolation would not move the
/// port toward upstream, it would delete SINGLE, PARALLEL and CHAIN execution outright and leave no
/// surface to replace them. They belong to the `workflowScript` port, and this function is the one
/// place they go when it happens: upstream calls its boundary from exactly the two model-facing
/// registrations that `Tool::execute` already serves.
///
/// # Errors
///
/// [`BLANK_ACTION_REFUSAL`] when `action` is present and trims to empty.
pub(crate) fn normalize_public_subagent_execution(action: Option<&str>) -> Result<Option<&str>, ToolError> {
    match action {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(ToolError::new(BLANK_ACTION_REFUSAL));
            }
            Ok(Some(trimmed))
        }
    }
}

/// The `subagent` tool's full discriminated-union parameter surface (R-SA-128, C8) — the Rust parse
/// target for pi's `SubagentParamsSchema` (`src/extension/schemas.ts:257-357`). Every top-level pi
/// field is represented so the tool can drive SINGLE (`agent`/`task`), PARALLEL (`tasks`/
/// `concurrency`/`worktree`), CHAIN (`chain`), management (`action` ∈ list/get/models/create/update/
/// delete), control (`action` ∈ status/interrupt/resume/append-step), and diagnostics (`action:
/// "doctor"`). Parsing is deliberately permissive (DI-SA-11): every field is optional, unknown keys
/// are ignored, and the union's genuinely-open sub-shapes (`config`/`control`/`output`/`skill`/
/// `acceptance`, and the per-item `tasks[]`/`chain[]` element shapes) are captured as raw
/// [`serde_json::Value`] here — the LLM-facing JSON Schema in [`crate::extension::tool::schema::subagent_tool_parameters`] carries
/// the full per-field structural detail, while typed per-item parsing/routing of `tasks[]`/`chain[]`
/// lands in P1 (this tier owns the schema + dispatch skeleton, not the sub-executor routing).
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubagentToolParams {
    pub(crate) agent: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) action: Option<String>,
    pub(crate) id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) dir: Option<String>,
    pub(crate) index: Option<u64>,
    /// G92 (pi `extension/schemas.ts:233-236` @v0.34.0): the optional `status` VIEW selector —
    /// `"fleet"` for the read-only in-flight fleet surface, `"transcript"` to tail one run's (or one
    /// child's) transcript. Anything else is rejected with pi's own
    /// `Unknown status view: X. Valid: fleet, transcript.` (`run-status.ts:193-198`).
    pub(crate) view: Option<String>,
    /// G92 (pi `extension/schemas.ts:237` @v0.34.0): the transcript tail's line budget, defaulted to
    /// 80 and clamped into `1..=500` by
    /// [`crate::background::fleet_view::transcript_line_limit`].
    pub(crate) lines: Option<i64>,
    /// SUBA-055 / pi `params.topic` (`extension/schemas.ts:281` @v0.47.1) — which packaged guide
    /// page `action='guide'` returns. Upstream declares it as a bare optional string with NO
    /// `enum` and no description, and that is ported exactly: the valid set is enforced by
    /// [`crate::registration::guide::read_subagent_guide`], which answers an unknown topic with the
    /// list rather than a schema rejection, so a model that guesses gets told the answer instead of
    /// a validation error.
    pub(crate) topic: Option<String>,
    pub(crate) message: Option<String>,
    /// SUBA-049 / pi `params.mode` (`extension/schemas.ts:283` @v0.43.0) — the `action='steer'`
    /// delivery mode. Carried as a raw `String` rather than an enum for the same reason `additional`
    /// is an `i64`: an unrecognised value must reach
    /// [`crate::background::control::SteerDeliveryMode::parse`] and be refused with a sentence the
    /// model can act on, not rejected by serde with a deserialization error it cannot.
    pub(crate) mode: Option<String>,
    pub(crate) chain_name: Option<String>,
    pub(crate) config: Option<serde_json::Value>,
    pub(crate) tasks: Option<Vec<serde_json::Value>>,
    pub(crate) concurrency: Option<u64>,
    pub(crate) worktree: Option<bool>,
    pub(crate) chain: Option<Vec<serde_json::Value>>,
    pub(crate) context: Option<String>,
    pub(crate) chain_dir: Option<String>,
    #[serde(rename = "async")]
    pub(crate) r#async: Option<bool>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) max_runtime_ms: Option<u64>,
    pub(crate) agent_scope: Option<String>,
    /// SUBA-046 / pi `params.additional` (`extension/schemas.ts:283` @v0.43.0) — the launches to
    /// add with `action='grant-spawn-budget'`. Typed `i64` rather than `u32` deliberately: pi
    /// validates `Number.isInteger(additional) && additional > 0` INSIDE
    /// `preflightSpawnBudgetGrant` and answers with its own message, so a `0`/negative value must
    /// reach that validator instead of being rejected by deserialization with a serde error the
    /// model cannot act on.
    pub(crate) additional: Option<i64>,
    /// `action='watchdog.configure'` write scope — `session` (the default, and the one that touches
    /// no file), `user`, or `project` (pi `extension/schemas.ts:285`).
    pub(crate) scope: Option<String>,
    /// Which watchdog endpoint a `watchdog.*` action targets — `main`, `children` or `child`
    /// (pi `extension/schemas.ts:286`).
    pub(crate) target: Option<String>,
    /// The reasoning level for `action='watchdog.configure'`: a level, `inherit`, or `false`
    /// (pi `extension/schemas.ts:288`). Accepts the JSON boolean `false` as well as the strings,
    /// which is why it deserializes through an untagged helper rather than as a bare `String`.
    #[serde(default, deserialize_with = "deserialize_watchdog_thinking")]
    pub(crate) thinking: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) artifacts: Option<bool>,
    pub(crate) include_progress: Option<bool>,
    pub(crate) share: Option<bool>,
    pub(crate) session_dir: Option<String>,
    pub(crate) clarify: Option<bool>,
    pub(crate) control: Option<serde_json::Value>,
    pub(crate) output: Option<serde_json::Value>,
    pub(crate) output_mode: Option<String>,
    pub(crate) skill: Option<serde_json::Value>,
    pub(crate) model: Option<String>,
    /// SUBA-043 / pi `params.outputSchema` (`extension/schemas.ts:351` @v0.43.0), read by
    /// `runSinglePath` at `runs/foreground/subagent-executor.ts:3651,3671` and written into the
    /// child's structured-output env pair by `runs/shared/pi-args.ts:759-762`. Carried raw here for
    /// the same reason every other open sub-shape is: the capture runtime
    /// ([`crate::exec::structured::create_structured_output_runtime`]) writes the schema verbatim to
    /// the file the child reads, so nothing on this side needs it typed.
    pub(crate) output_schema: Option<serde_json::Value>,
    /// SUBA-047 / pi `params.toolBudget` (`extension/schemas.ts:354` @v0.43.0). Raw here and
    /// validated at the dispatch boundary through
    /// [`crate::exec::tool_budget::validate_tool_budget_config`] — pi's own
    /// `validateToolBudgetConfig(toolBudgetInput, "toolBudget")`
    /// (`runs/background/async-execution.ts:1298-1299`) — so a malformed budget is refused before
    /// any child spawns rather than degrading to "no budget".
    pub(crate) tool_budget: Option<serde_json::Value>,
    /// SUBA-008 / pi `params.turnBudget` (`extension/schemas.ts:328` @v0.43.0). Raw here and
    /// validated at the dispatch boundary through
    /// [`crate::exec::turn_budget::resolve_turn_budget_config`] — pi's own
    /// `resolveTurnBudgetConfig(effectiveParams.turnBudget ?? deps.config.turnBudget)`
    /// (`runs/foreground/subagent-executor.ts:4928-4929`) — so a malformed budget is refused
    /// before any child spawns rather than degrading to "no budget".
    pub(crate) turn_budget: Option<serde_json::Value>,
    /// SUBA-021 / pi `params.usageBudget` (`extension/schemas.ts:330` @v0.43.0). Raw here and
    /// validated at the dispatch boundary through
    /// [`crate::exec::usage_budget::validate_usage_budget_config`] — pi's own
    /// `validateUsageBudgetConfig` — so a malformed budget is refused before any child spawns.
    pub(crate) usage_budget: Option<serde_json::Value>,
    pub(crate) acceptance: Option<serde_json::Value>,
    /// The six mission parameters (`extension/schemas.ts:297-301` + `:302-304` @v0.43.0), read by
    /// [`crate::missions`]: `missionId`/`mission` bind a launch to a mission and are also the
    /// `mission.*` action targets; `missionUpdate`/`missionStatus`/`missionScope` are
    /// action-only; `runMode`/`runStatus`/`summary` are `mission.attach-run`/`mission.close`
    /// arguments.
    pub(crate) mission_id: Option<String>,
    pub(crate) mission: Option<serde_json::Value>,
    pub(crate) mission_update: Option<serde_json::Value>,
    pub(crate) mission_status: Option<String>,
    pub(crate) mission_scope: Option<String>,
    pub(crate) run_mode: Option<String>,
    pub(crate) run_status: Option<String>,
    pub(crate) summary: Option<String>,
}

impl SubagentToolParams {
    /// The mission-action argument projection (`missions/actions.ts:69-82`'s
    /// `MissionActionParams`), built from the SAME parsed tool call the execution arms read — a
    /// mission action and an execution call share `id`/`runId`/`dir`/`agent`.
    pub(crate) fn mission_action_params(&self) -> crate::missions::MissionActionParams {
        crate::missions::MissionActionParams {
            mission_id: self.mission_id.clone(),
            mission: self.mission.clone(),
            mission_update: self.mission_update.clone(),
            mission_status: self.mission_status.clone(),
            mission_scope: self.mission_scope.clone(),
            id: self.id.clone(),
            run_id: self.run_id.clone(),
            dir: self.dir.clone(),
            run_mode: self.run_mode.clone(),
            run_status: self.run_status.clone(),
            agent: self.agent.clone(),
            summary: self.summary.clone(),
        }
    }

    /// The launch-binding projection (`missions/lifecycle.ts:12-18`'s `MissionLaunchParams`) — the
    /// objective search reads `task`, then each `tasks[]` item's `task`, then each `chain[]`
    /// step's (and its `parallel` children's).
    pub(crate) fn mission_launch_params(&self) -> crate::missions::MissionLaunchParams {
        crate::missions::MissionLaunchParams {
            mission_id: self.mission_id.clone(),
            mission: self.mission.clone(),
            task: self.task.clone(),
            tasks: self.tasks.clone(),
            chain: self.chain.clone(),
        }
    }
}

/// pi `resolveForegroundTimeout` (`subagent-executor.ts:1951-1968`): `timeoutMs` and `maxRuntimeMs`
/// are ALIASES for one foreground timeout budget. Returns the single effective value (or `None` when
/// neither is supplied), or an `Err` message when a value is non-positive or the two aliases were
/// both supplied with DIFFERENT values. (A negative/fractional value could never have deserialized
/// into `Option<u64>`, so pi's `!Number.isInteger(value) || value <= 0` reduces here to rejecting
/// `0`.)
/// The ONE `watchdog.*` verb pi lists in `MUTATING_MANAGEMENT_ACTIONS`
/// (`subagent-executor.ts:151` @v0.43.0) — `watchdog.status`/`watchdog.check`/
/// `watchdog.recommend-model` are read-only reports and stay available to a child-safe fanout tool.
pub(crate) const WATCHDOG_MUTATING_ACTION: &str = "watchdog.configure";

/// `thinking: Type.Unsafe({ anyOf: [{type:"string"}, {type:"boolean", enum:[false]}] })`
/// (`extension/schemas.ts:288`): a level string, the literal `"inherit"`, or the JSON boolean
/// `false`. The boolean is normalized to the STRING `"false"`, which is exactly what
/// [`crate::watchdog::model_selection::parse_watchdog_thinking_input`] accepts as "reasoning off" —
/// so both wire spellings reach the same decision rather than one of them being a parse error.
fn deserialize_watchdog_thinking<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value)),
        Some(serde_json::Value::Bool(false)) => Ok(Some("false".to_string())),
        Some(other) => Err(serde::de::Error::custom(format!(
            "thinking must be a string or false, got {other}"
        ))),
    }
}

/// pi `MAX_TIMER_DELAY_MS` (`runs/foreground/subagent-executor.ts:2675` @v0.57.0).
///
/// Upstream's stated reason is a Node `setTimeout` overflow that would silently clamp the delay and
/// expire the run almost immediately. Rust has no such clamp, but the bound is load-bearing here
/// too: both foreground seams arm the deadline as `Instant::now() + Duration::from_millis(ms)`
/// (`extension/executor/foreground.rs`, `extension/executor/chain.rs`), and `Instant + Duration`
/// PANICS on overflow. Keeping upstream's ceiling therefore both stops a config value from
/// panicking a run and keeps the same settings file behaving identically in either port.
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

/// pi `resolveConfigDefaultTimeoutMs` (`runs/foreground/subagent-executor.ts:2684` @v0.57.0):
/// the global `subagents.timeoutMs` default, or `None`.
///
/// Silently yields `None` for ANY invalid value — absent, non-integer, non-positive, or above
/// [`MAX_TIMER_DELAY_MS`] — so the caller falls back to the built-in default. Upstream never errors
/// here and neither does this: a malformed global setting must not fail a run that would otherwise
/// have had a perfectly sane deadline.
///
/// [`serde_json::Value::as_u64`] does upstream's `typeof raw !== "number" || !Number.isInteger(raw)
/// || raw <= 0` in one call, rejecting strings, booleans, fractional and negative numbers. It also
/// rejects a JSON `1.0`, where upstream's `Number.isInteger(1.0)` is `true` — not a shape worth
/// widening the port for, and the degrade is to the built-in default rather than to an error.
pub(crate) fn resolve_config_default_timeout_ms(raw: Option<&serde_json::Value>) -> Option<u64> {
    let value = raw?.as_u64()?;
    (value > 0 && value <= MAX_TIMER_DELAY_MS).then_some(value)
}

/// The foreground default rung of pi `resolveSingleAgentLaunchTimeout`
/// (`runs/foreground/subagent-executor.ts:2719-2725` @v0.57.0):
/// `configDefaultTimeoutMs ?? DEFAULT_FOREGROUND_TIMEOUT_MS`, with the agent's own frontmatter
/// `timeoutMs:` ahead of both.
///
/// Upstream folds the agent rung into `params.timeoutMs` before its ladder runs
/// (`applySingleAgentLaunchDefaults`), so it never appears as a separate argument there; this port
/// resolves it separately ([`crate::extension::SubagentExecutor::single_agent_launch_defaults`]),
/// which is why it is the first rung here.
///
/// Returns what [`resolve_foreground_timeout`] should be GIVEN as its default — not the final
/// timeout. An explicit call-site `timeoutMs`/`maxRuntimeMs` still outranks all three rungs.
///
/// `background` is upstream's `!async` arm (`:2724`), carried as a parameter rather than left as an
/// `if` at each of the four call sites: an ASYNC launch gets NO built-in backstop from here,
/// because `extension/executor/background.rs` applies
/// [`crate::background::DEFAULT_ASYNC_CHILD_TIMEOUT_MS`] itself through a `timeout_ms.unwrap_or(…)`.
/// Handing that `unwrap_or` a `Some` on every run would silently retire it — harmless while the two
/// constants agree, a trap the moment either moves. The agent rung still applies on both arms.
#[must_use]
pub(crate) fn foreground_timeout_default(
    background: bool,
    agent_default_ms: Option<u64>,
    config_timeout_ms: Option<&serde_json::Value>,
) -> Option<u64> {
    if background {
        return agent_default_ms;
    }
    agent_default_ms
        .or_else(|| resolve_config_default_timeout_ms(config_timeout_ms))
        .or(Some(crate::exec::DEFAULT_FOREGROUND_TIMEOUT_MS))
}

/// pi `resolveForegroundTimeout` (`runs/foreground/subagent-executor.ts:2689` @v0.57.0).
///
/// `default_timeout_ms` is applied LAST, so it is reached only when the call supplied neither
/// `timeoutMs` nor `maxRuntimeMs` — exactly upstream's `rawTimeout === undefined && rawMaxRuntime
/// === undefined` early return. The validation below still runs first, so an invalid explicit value
/// errors instead of being silently replaced by a default.
pub(crate) fn resolve_foreground_timeout(
    p: &SubagentToolParams,
    default_timeout_ms: Option<u64>,
) -> Result<Option<u64>, String> {
    for (name, value) in [("timeoutMs", p.timeout_ms), ("maxRuntimeMs", p.max_runtime_ms)] {
        if value == Some(0) {
            return Err(format!("{name} must be a positive integer."));
        }
    }
    if let (Some(a), Some(b)) = (p.timeout_ms, p.max_runtime_ms)
        && a != b
    {
        return Err(
            "timeoutMs and maxRuntimeMs are aliases; provide only one value or use the same \
             value for both."
                .to_string(),
        );
    }
    Ok(p.timeout_ms.or(p.max_runtime_ms).or(default_timeout_ms))
}

/// pi `resolveExecutionAgentScope` (`pi-subagents/src/agents/agent-scope.ts:3-6`): `"user"`/
/// `"project"`/`"both"` pass through verbatim; anything else (absent, or any other garbage
/// string) coerces to `Both` with no error. Every execution entry point (single/parallel/chain
/// dispatch, resume, append-step) calls this on the raw `agentScope` tool param before threading
/// the result into agent discovery, so an unrecognized value is never rejected — it silently
/// yields the unnarrowed (both user- and project-scope) view, exactly like an absent value.
pub(crate) fn resolve_execution_agent_scope(raw: Option<&str>) -> AgentReadScope {
    match raw {
        Some("user") => AgentReadScope::User,
        Some("project") => AgentReadScope::Project,
        _ => AgentReadScope::Both,
    }
}

/// pi `formatFailedSingleRunOutput` (`subagent-executor.ts:1569-1580`): the delivered content for a
/// FAILED single run — the error text (`result.error` or `"Failed"`), followed, ONLY when the run
/// produced distinct output, by an `Output:` block carrying that output. This is what
/// [`crate::extension::SubagentTool::route_single`] hands to `ToolError` (cyrup's error channel; pi's `isError: true`),
/// so an LLM caller sees the failure reason in the model-facing CONTENT rather than only buried in
/// `details` JSON. (pi additionally appends an `Output artifact:` line from
/// `result.artifactPaths?.outputPath`; this crate's [`SingleResult`] carries no such field — the
/// saved-output reference is already folded into `final_output` — so that line has no analogue here.)
pub(crate) fn format_failed_single_run_output(result: &SingleResult, display_output: &str) -> String {
    let error = result
        .error
        .as_deref()
        .filter(|e| !e.is_empty())
        .unwrap_or("Failed");
    let output = display_output.trim();
    let mut lines = vec![error.to_string()];
    if !output.is_empty() && output != error.trim() {
        lines.push(String::new());
        lines.push("Output:".to_string());
        lines.push(output.to_string());
    }
    lines.join("\n")
}

impl SubagentToolParams {
    /// Whether this call requested background/detached execution (pi
    /// `subagent-executor.ts:3318-3322,3382` @v0.34.0).
    ///
    /// pi resolves this in two steps: first `applyForceTopLevelAsyncOverride`
    /// (`runs/background/top-level-async.ts:5-12`) forces `async: true, clarify: false` onto the
    /// effective params when this is a top-level call (`depth === 0`) AND
    /// `config.forceTopLevelAsync === true` — overriding whatever the call itself requested. Then
    /// `requestedAsync = effectiveParams.async ?? deps.asyncByDefault` (an omitted `async` falls
    /// back to the config's `asyncByDefault`, not a hardcoded `false`), and finally
    /// `effectiveAsync = requestedAsync && effectiveParams.clarify !== true` (an explicit
    /// `clarify: true` always keeps the run foreground so its supervisor prompt can be seen,
    /// regardless of the async request).
    pub(crate) fn is_background(&self, cfg: &SubagentExtensionConfig, depth: u32) -> bool {
        let force_override = depth == 0 && cfg.force_top_level_async;
        let async_param = if force_override { Some(true) } else { self.r#async };
        let clarify = if force_override { Some(false) } else { self.clarify };
        let requested_async = async_param.unwrap_or(cfg.async_by_default);
        requested_async && clarify != Some(true)
    }

    /// The requested fork/fresh context OVERRIDE (pi `context`), as an `Option` that preserves the
    /// "omitted" case: `Some(Fork)`/`Some(Fresh)` for an explicit value, `None` when the caller left
    /// `context` off entirely. An omitted (`None`) value is what lets each requested agent fall back
    /// to ITS OWN persona `default_context` downstream (pi `resolveAgentDefaultContextPolicy`,
    /// `subagent-executor.ts:1875-1891`) rather than being forced to `Fresh` — the collapse-to-`Fresh`
    /// that the pre-Tier-2 `context_mode` did. Any non-`"fork"` explicit string still resolves to
    /// `Some(Fresh)` (pi treats only the literal `"fork"` as fork).
    pub(crate) fn context_override(&self) -> Option<ContextMode> {
        match self.context.as_deref() {
            None => None,
            Some("fork") => Some(ContextMode::Fork),
            Some(_) => Some(ContextMode::Fresh),
        }
    }

    /// The parameter keys actually supplied on this call, in pi's own camelCase spelling — surfaced
    /// in the labeled placeholder text of the not-yet-wired management/control/parallel/chain
    /// dispatch arms so a caller (and P1's implementer) can see exactly what shape was parsed.
    /// Reading every field here is also what lets the full pi-union struct above compile under the
    /// workspace's `-D warnings` (`dead_code`) without any non-`#[cfg(test)]` `#[allow]`.
    pub(crate) fn provided_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.agent.is_some() { keys.push("agent"); }
        if self.task.is_some() { keys.push("task"); }
        if self.action.is_some() { keys.push("action"); }
        if self.id.is_some() { keys.push("id"); }
        if self.run_id.is_some() { keys.push("runId"); }
        if self.dir.is_some() { keys.push("dir"); }
        if self.index.is_some() { keys.push("index"); }
        if self.view.is_some() { keys.push("view"); }
        if self.lines.is_some() { keys.push("lines"); }
        if self.message.is_some() { keys.push("message"); }
        if self.chain_name.is_some() { keys.push("chainName"); }
        if self.config.is_some() { keys.push("config"); }
        if self.tasks.is_some() { keys.push("tasks"); }
        if self.concurrency.is_some() { keys.push("concurrency"); }
        if self.worktree.is_some() { keys.push("worktree"); }
        if self.chain.is_some() { keys.push("chain"); }
        if self.context.is_some() { keys.push("context"); }
        if self.chain_dir.is_some() { keys.push("chainDir"); }
        if self.r#async.is_some() { keys.push("async"); }
        if self.timeout_ms.is_some() { keys.push("timeoutMs"); }
        if self.max_runtime_ms.is_some() { keys.push("maxRuntimeMs"); }
        if self.agent_scope.is_some() { keys.push("agentScope"); }
        if self.cwd.is_some() { keys.push("cwd"); }
        if self.artifacts.is_some() { keys.push("artifacts"); }
        if self.include_progress.is_some() { keys.push("includeProgress"); }
        if self.share.is_some() { keys.push("share"); }
        if self.session_dir.is_some() { keys.push("sessionDir"); }
        if self.clarify.is_some() { keys.push("clarify"); }
        if self.control.is_some() { keys.push("control"); }
        if self.output.is_some() { keys.push("output"); }
        if self.output_mode.is_some() { keys.push("outputMode"); }
        if self.skill.is_some() { keys.push("skill"); }
        if self.model.is_some() { keys.push("model"); }
        if self.output_schema.is_some() { keys.push("outputSchema"); }
        if self.tool_budget.is_some() { keys.push("toolBudget"); }
        if self.turn_budget.is_some() { keys.push("turnBudget"); }
        if self.usage_budget.is_some() { keys.push("usageBudget"); }
        if self.additional.is_some() { keys.push("additional"); }
        if self.acceptance.is_some() { keys.push("acceptance"); }
        if self.mission_id.is_some() { keys.push("missionId"); }
        if self.mission.is_some() { keys.push("mission"); }
        if self.mission_update.is_some() { keys.push("missionUpdate"); }
        if self.mission_status.is_some() { keys.push("missionStatus"); }
        if self.mission_scope.is_some() { keys.push("missionScope"); }
        if self.run_mode.is_some() { keys.push("runMode"); }
        if self.run_status.is_some() { keys.push("runStatus"); }
        if self.summary.is_some() { keys.push("summary"); }
        keys
    }
}

/// pi `validateExecutionAcceptance` (`runs/shared/acceptance.ts:288-310` @v0.34.0, called from
/// `validateExecutionInput` at `subagent-executor.ts:1757` immediately after the mode-exclusivity
/// gate and BEFORE agent resolution): run `validateAcceptanceInput` over EVERY `acceptance` the
/// dispatch declares — the top-level SINGLE param, each `tasks[i]`, each `chain[i]`, and each
/// `chain[i].parallel[j]` (array form) or `chain[i].parallel` (dynamic-template object form) — using
/// pi's own per-site path labels, and return the collected messages.
///
/// SUBA-N04: cyrup validated only the top-level SINGLE `acceptance` (inside `route_single`), so a
/// malformed policy on a `tasks[]`/`chain[]` item was never refused. Now that those items' policies
/// actually reach the child (they used to be silently dropped, which is what made the missing
/// validation invisible), the refusal has to be up front for the same reason upstream puts it there:
/// a fan-out must be rejected whole, never half-run and then failed on step 3.
pub(crate) fn validate_execution_acceptance(params: &SubagentToolParams) -> Vec<String> {
    use crate::exec::acceptance::model::validate_acceptance_input;

    /// `undefined` for `validate_acceptance_input`'s purposes — a missing key reads as `Null`.
    fn field<'a>(value: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
        value.get(key).unwrap_or(&serde_json::Value::Null)
    }

    let mut errors = validate_acceptance_input(
        params.acceptance.as_ref().unwrap_or(&serde_json::Value::Null),
        "acceptance",
    );
    for (index, task) in params.tasks.iter().flatten().enumerate() {
        errors.extend(validate_acceptance_input(
            field(task, "acceptance"),
            &format!("tasks[{index}].acceptance"),
        ));
    }
    for (step_index, step) in params.chain.iter().flatten().enumerate() {
        errors.extend(validate_acceptance_input(
            field(step, "acceptance"),
            &format!("chain[{step_index}].acceptance"),
        ));
        match step.get("parallel") {
            Some(serde_json::Value::Array(tasks)) => {
                for (task_index, task) in tasks.iter().enumerate() {
                    errors.extend(validate_acceptance_input(
                        field(task, "acceptance"),
                        &format!("chain[{step_index}].parallel[{task_index}].acceptance"),
                    ));
                }
            }
            // pi's `else if (step.parallel)`: the dynamic-fanout TEMPLATE object. A JSON `null`/
            // `false` `parallel` is falsy upstream and is skipped here for the same reason.
            Some(template) if !template.is_null() && template.as_bool() != Some(false) => {
                errors.extend(validate_acceptance_input(
                    field(template, "acceptance"),
                    &format!("chain[{step_index}].parallel.acceptance"),
                ));
            }
            _ => {}
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::extension::testsupport::scoped_tool;

    // ---------------------------------------------------------------------------------------
    // SUBA-077 — the foreground wall-clock deadline ladder
    // (pi `resolveSingleAgentLaunchTimeout`, `subagent-executor.ts:2719-2725` @v0.57.0)
    // ---------------------------------------------------------------------------------------

    const BUILTIN: u64 = crate::exec::DEFAULT_FOREGROUND_TIMEOUT_MS;

    fn params(value: serde_json::Value) -> SubagentToolParams {
        serde_json::from_value(value).expect("params parse")
    }

    /// The whole ladder, in one place, in precedence order. The rung that matters most is the LAST:
    /// before SUBA-077 a foreground run with nothing set resolved `None` — no wall-clock deadline at
    /// all — so a child whose bash tool blocked forever hung the orchestrator's turn open-endedly.
    #[test]
    fn the_foreground_timeout_ladder_runs_explicit_then_agent_then_config_then_builtin() {
        let cfg = serde_json::json!(60_000);
        let agent = Some(120_000_u64);

        // 1. an explicit call-site value outranks every default.
        assert_eq!(
            resolve_foreground_timeout(
                &params(serde_json::json!({"timeoutMs": 5_000})),
                foreground_timeout_default(false, agent, Some(&cfg))
            ),
            Ok(Some(5_000))
        );
        // ...and so does its alias.
        assert_eq!(
            resolve_foreground_timeout(
                &params(serde_json::json!({"maxRuntimeMs": 5_000})),
                foreground_timeout_default(false, agent, Some(&cfg))
            ),
            Ok(Some(5_000))
        );

        // 2. the agent's own frontmatter `timeoutMs:` outranks the config value and the built-in.
        //    This rung is the one a naive fix kills: it used to be a trailing `.or(launch_defaults.1)`
        //    on the RESULT, which a default applied inside the resolver would render unreachable.
        assert_eq!(
            resolve_foreground_timeout(
                &params(serde_json::json!({})),
                foreground_timeout_default(false, agent, Some(&cfg))
            ),
            Ok(Some(120_000)),
            "the agent rung must still be live once the default rungs exist"
        );

        // 3. `subagents.timeoutMs` replaces the built-in backstop.
        assert_eq!(
            foreground_timeout_default(false, Option::None, Some(&cfg)),
            Some(60_000)
        );

        // 4. and with nothing set at all, the built-in backstop applies.
        assert_eq!(
            resolve_foreground_timeout(
                &params(serde_json::json!({})),
                foreground_timeout_default(false, Option::None, Option::None)
            ),
            Ok(Some(BUILTIN)),
            "a foreground run with nothing set must be BOUNDED, not open-ended"
        );
        assert_eq!(BUILTIN, 1_800_000, "pi `DEFAULT_FOREGROUND_TIMEOUT_MS` is 30 minutes");
    }

    /// pi `resolveConfigDefaultTimeoutMs` (`:2684`) returns `undefined` for ANY invalid value and
    /// never errors, so a malformed global setting degrades to the built-in default rather than
    /// failing a run that would otherwise have had a perfectly sane deadline.
    #[test]
    fn an_invalid_config_timeout_is_ignored_rather_than_erroring() {
        for bad in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!("abc"),
            serde_json::json!(true),
            serde_json::json!(null),
            // Above pi's `MAX_TIMER_DELAY_MS`. Upstream rejects it to avoid a `setTimeout`
            // overflow; here it also keeps a config value from panicking `Instant + Duration`.
            serde_json::json!(2_147_483_648_u64),
        ] {
            assert_eq!(
                resolve_config_default_timeout_ms(Some(&bad)),
                Option::None,
                "{bad} must not resolve to a config default"
            );
            assert_eq!(
                foreground_timeout_default(false, Option::None, Some(&bad)),
                Some(BUILTIN),
                "{bad} must fall through to the built-in backstop, not disarm the deadline"
            );
        }

        // The boundary itself is accepted — the ceiling is inclusive upstream (`raw > MAX`).
        assert_eq!(
            resolve_config_default_timeout_ms(Some(&serde_json::json!(2_147_483_647_u64))),
            Some(2_147_483_647)
        );
        assert_eq!(
            resolve_config_default_timeout_ms(Option::None),
            Option::None,
            "an omitted key is simply no config rung"
        );
    }

    /// pi's `!async` arm (`:2724`). An ASYNC launch gets no built-in backstop from this ladder,
    /// because `extension/executor/background.rs` applies `DEFAULT_ASYNC_CHILD_TIMEOUT_MS` itself
    /// through a `timeout_ms.unwrap_or(…)`. Handing that `unwrap_or` a `Some` on every run would
    /// silently retire it — harmless while the two constants agree, a trap the moment either moves.
    #[test]
    fn an_async_launch_gets_no_foreground_backstop_but_keeps_its_agent_rung() {
        assert_eq!(
            foreground_timeout_default(true, Option::None, Option::None),
            Option::None,
            "the async path must reach its own default with `None` in hand"
        );
        assert_eq!(
            foreground_timeout_default(true, Option::None, Some(&serde_json::json!(60_000))),
            Option::None,
            "not even a config value may pre-empt the async default from this seam (SUBA-051's)"
        );
        assert_eq!(
            foreground_timeout_default(true, Some(120_000), Option::None),
            Some(120_000),
            "the agent's own frontmatter rung still applies on the async arm, as it did before"
        );
    }

    /// The defaults are applied only when the call supplied NEITHER param — upstream's early
    /// return. Validation still runs first, so an invalid explicit value errors instead of being
    /// silently replaced by a default that would mask it.
    #[test]
    fn an_invalid_explicit_timeout_still_errors_rather_than_taking_the_default() {
        let default = foreground_timeout_default(false, Option::None, Option::None);
        assert_eq!(
            resolve_foreground_timeout(&params(serde_json::json!({"timeoutMs": 0})), default),
            Err("timeoutMs must be a positive integer.".to_string())
        );
        assert_eq!(
            resolve_foreground_timeout(&params(serde_json::json!({"maxRuntimeMs": 0})), default),
            Err("maxRuntimeMs must be a positive integer.".to_string())
        );
        assert_eq!(
            resolve_foreground_timeout(
                &params(serde_json::json!({"timeoutMs": 10, "maxRuntimeMs": 20})),
                default
            ),
            Err("timeoutMs and maxRuntimeMs are aliases; provide only one value or use the same \
                 value for both."
                .to_string())
        );
    }

    /// SUBA-N04: pi `validateExecutionAcceptance` (`runs/shared/acceptance.ts:288-310` @v0.34.0)
    /// validates EVERY declared acceptance in one dispatch, with pi's own per-site path labels —
    /// not just the top-level SINGLE param, which was all this crate checked while the item-level
    /// policies were being dropped anyway.
    #[test]
    fn validate_execution_acceptance_labels_every_declared_policy_site() {
        let params: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "tasks": [
                { "agent": "a", "acceptance": "checked" },
                { "agent": "b", "acceptance": "bogus" }
            ]
        }))
        .expect("params parse");
        assert_eq!(
            validate_execution_acceptance(&params),
            vec!["tasks[1].acceptance has invalid level 'bogus'.".to_string()],
            "the failing item is named by its own index; the valid one contributes nothing"
        );

        let chain: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "chain": [
                { "agent": "a", "acceptance": { "nope": 1 } },
                { "parallel": [{ "agent": "b" }, { "agent": "c", "acceptance": "bad" }] },
                { "expand": { "from": { "output": "x" } },
                  "parallel": { "agent": "d", "acceptance": "worse" },
                  "collect": { "as": "y" } }
            ]
        }))
        .expect("params parse");
        assert_eq!(
            validate_execution_acceptance(&chain),
            vec![
                "chain[0].acceptance.nope is not supported.".to_string(),
                "chain[1].parallel[1].acceptance has invalid level 'bad'.".to_string(),
                "chain[2].parallel.acceptance has invalid level 'worse'.".to_string(),
            ],
            "static parallel tasks are labelled by index; a dynamic template is labelled bare"
        );

        // A dispatch declaring no acceptance anywhere is silent.
        let bare: SubagentToolParams =
            serde_json::from_value(serde_json::json!({ "agent": "a", "task": "t" }))
                .expect("params parse");
        assert!(validate_execution_acceptance(&bare).is_empty());
    }

    /// T6 parity regression (pi `subagent-executor.ts:2968` + `fanout-child.ts:148`): an OMITTED
    /// `async` on a foreground-dispatched call must default to the extension config's
    /// `asyncByDefault`, exactly as the fanout child threads `config.asyncByDefault` into its
    /// executor. Also re-pins that `SubagentExtensionConfig` deserializes the config-file
    /// `asyncByDefault` camelCase key into `async_by_default`, so a real `config.json` value
    /// actually reaches `is_background` rather than staying stuck at the hardcoded default.
    #[test]
    fn async_by_default_config_key_deserializes_and_is_honored_by_is_background() {
        let cfg: SubagentExtensionConfig =
            serde_json::from_value(serde_json::json!({ "asyncByDefault": true })).unwrap_or_else(|e| {
                panic!("asyncByDefault must deserialize into SubagentExtensionConfig: {e}")
            });
        assert!(cfg.async_by_default);

        let omitted: SubagentToolParams =
            serde_json::from_value(serde_json::json!({ "agent": "worker", "task": "do it" }))
                .expect("single shape parses");
        assert!(
            omitted.is_background(&cfg, 0),
            "an omitted `async` must honor a config-file-sourced asyncByDefault: true"
        );
    }

    /// C8 permissive parsing (DI-SA-11): the full pi-union parse target accepts the SINGLE,
    /// PARALLEL, CHAIN, management, and control shapes, ignores unknown keys, and reports which keys
    /// were supplied — the routing dimension every `execute` dispatch arm branches on.
    #[test]
    fn subagent_tool_params_parse_every_pi_mode_shape() {
        // SINGLE with context/async/model.
        let single: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "worker", "task": "do it", "context": "fork", "async": true,
            "model": "anthropic/claude-sonnet-4", "unknownFutureKey": 42
        }))
        .expect("single shape parses permissively (unknown keys ignored)");
        assert_eq!(single.agent.as_deref(), Some("worker"));
        assert!(single.is_background(&SubagentExtensionConfig::default(), 0));
        assert!(matches!(single.context_override(), Some(ContextMode::Fork)));
        assert!(single.provided_keys().contains(&"model"));
        assert!(!single.provided_keys().contains(&"unknownFutureKey"));

        // PARALLEL.
        let parallel: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "tasks": [{ "agent": "reviewer", "task": "review", "output": "r.md", "reads": ["in.md"], "progress": true }],
            "concurrency": 3, "worktree": true
        }))
        .expect("parallel shape parses");
        assert!(parallel.tasks.is_some());
        assert_eq!(parallel.concurrency, Some(3));
        assert_eq!(parallel.worktree, Some(true));

        // CHAIN.
        let chain: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "chain": [{ "agent": "a", "task": "Analyze {task}" }, { "parallel": [{ "agent": "b", "count": 2 }] }]
        }))
        .expect("chain shape parses");
        assert!(chain.chain.is_some());

        // Management + control actions (camelCase runId/chainName round-trip).
        let mgmt: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "action": "get", "chainName": "release", "agent": "pkg.reviewer"
        }))
        .expect("management shape parses");
        assert_eq!(mgmt.action.as_deref(), Some("get"));
        assert_eq!(mgmt.chain_name.as_deref(), Some("release"));

        let control: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "action": "resume", "runId": "abc", "index": 0, "message": "continue"
        }))
        .expect("control shape parses");
        assert_eq!(control.run_id.as_deref(), Some("abc"));
        assert_eq!(control.index, Some(0));
    }

    /// R-SA parity regression: `config.asyncByDefault`/`forceTopLevelAsync` must actually be
    /// consulted by [`SubagentToolParams::is_background`] (pi `subagent-executor.ts:3318-3322,3382` @v0.34.0,
    /// `runs/background/top-level-async.ts:5-12`), not just parsed and discarded. Before this fix
    /// `is_background` hardcoded `self.r#async.unwrap_or(false)`, so every one of these assertions
    /// would fail pre-fix (an omitted `async` always resolved to foreground, `forceTopLevelAsync`
    /// never flipped anything to background, and `clarify: true` never suppressed an async request).
    #[test]
    fn is_background_honors_async_by_default_and_force_top_level_async() {
        // An omitted `async` falls back to `config.asyncByDefault`, not a hardcoded `false`.
        let omitted: SubagentToolParams =
            serde_json::from_value(serde_json::json!({ "agent": "worker", "task": "do it" }))
                .expect("single shape parses");
        let async_by_default_cfg =
            SubagentExtensionConfig { async_by_default: true, ..SubagentExtensionConfig::default() };
        assert!(
            omitted.is_background(&async_by_default_cfg, 0),
            "an omitted `async` must default to config.asyncByDefault"
        );
        assert!(
            !omitted.is_background(&SubagentExtensionConfig::default(), 0),
            "asyncByDefault: false (the default) must still leave an omitted `async` foreground"
        );

        // An explicit `async: false` still wins over `asyncByDefault: true` (only an OMITTED value
        // falls back to the config default).
        let explicit_false: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "worker", "task": "do it", "async": false
        }))
        .expect("single shape parses");
        assert!(!explicit_false.is_background(&async_by_default_cfg, 0));

        // `forceTopLevelAsync` forces async ON at depth 0 regardless of the call's own `async`
        // value, but has no effect at a nested depth.
        let force_cfg = SubagentExtensionConfig {
            force_top_level_async: true,
            ..SubagentExtensionConfig::default()
        };
        assert!(
            explicit_false.is_background(&force_cfg, 0),
            "forceTopLevelAsync must force a top-level (depth 0) run to background even when the \
             call explicitly requested async: false"
        );
        assert!(
            !explicit_false.is_background(&force_cfg, 1),
            "forceTopLevelAsync must NOT apply at a nested depth"
        );

        // `clarify: true` always keeps the run foreground, even when async was requested.
        let clarify_true: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "worker", "task": "do it", "async": true, "clarify": true
        }))
        .expect("single shape parses");
        assert!(!clarify_true.is_background(&SubagentExtensionConfig::default(), 0));
    }

    /// SUBA-008's refusal half — pi `resolveTurnBudgetConfig(...)` at
    /// `subagent-executor.ts:4928-4929`, whose `error` becomes a `buildRequestedModeError`. A
    /// malformed budget must refuse the call with the validator's own message rather than
    /// silently degrading to "unbudgeted".
    #[test]
    fn a_malformed_turn_budget_param_is_refused_with_upstreams_own_message() {
        let parsed: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "x", "task": "y", "turnBudget": { "maxTurns": 0 }
        }))
        .expect("params parse is permissive; validation happens at dispatch");
        let err = crate::exec::turn_budget::resolve_turn_budget_config(
            parsed.turn_budget.as_ref(),
            "turnBudget",
        )
        .expect_err("maxTurns 0 must be refused");
        assert_eq!(err, "turnBudget.maxTurns must be an integer >= 1.");

        let parsed: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "x", "task": "y", "turnBudget": { "maxTurns": 2, "hard": 3 }
        }))
        .expect("params parse");
        let err = crate::exec::turn_budget::resolve_turn_budget_config(
            parsed.turn_budget.as_ref(),
            "turnBudget",
        )
        .expect_err("an unknown key must be refused, not ignored");
        assert_eq!(
            err, "turnBudget.hard is not supported.",
            "the tool budget's `hard` key is not a turn-budget key, and upstream says so by name"
        );
    }

    /// pi `canonicalizeExecutionParams` (`subagent-executor.ts:1682-1734`): every agent name in the
    /// dispatch — SINGLE, each `tasks[]` entry, each chain step, each static-parallel task, and a
    /// dynamic step's template — is rewritten to the CANONICAL name before the mode arm runs, so no
    /// downstream surface ever reports a run under a name no agent file carries.
    #[tokio::test]
    async fn execution_params_are_canonicalized_across_every_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = scoped_tool(dir.path()).await;

        let params = SubagentToolParams {
            agent: Some("advisor".to_string()),
            tasks: Some(vec![
                serde_json::json!({ "agent": "developer", "task": "a" }),
                serde_json::json!({ "agent": "scout", "task": "b" }),
            ]),
            chain: Some(vec![
                serde_json::json!({ "agent": "coder", "task": "c" }),
                serde_json::json!({ "parallel": [{ "agent": "advisor", "task": "d" }] }),
                serde_json::json!({ "parallel": { "agent": "implementer", "expand": "x" } }),
            ]),
            ..serde_json::from_value(serde_json::json!({})).expect("default params")
        };

        let canonical = tool
            .canonicalize_execution_params(&params, dir.path())
            .await
            .expect("canonicalization succeeds")
            .expect("at least one name changed");

        assert_eq!(canonical.agent.as_deref(), Some("oracle"));
        let tasks = canonical.tasks.as_ref().expect("tasks kept");
        assert_eq!(tasks[0]["agent"], "worker");
        assert_eq!(tasks[0]["task"], "a", "the rest of the task object must survive untouched");
        assert_eq!(tasks[1]["agent"], "scout", "an already-canonical name is left alone");

        let chain = canonical.chain.as_ref().expect("chain kept");
        assert_eq!(chain[0]["agent"], "worker");
        assert_eq!(chain[1]["parallel"][0]["agent"], "oracle");
        assert_eq!(chain[2]["parallel"]["agent"], "worker");
        assert_eq!(chain[2]["parallel"]["expand"], "x");
    }

    /// An ambiguous alias inside a fan-out aborts the WHOLE dispatch, with pi's per-site location
    /// suffix (`subagent-executor.ts:1696,1710,1718,1724`).
    #[tokio::test]
    async fn an_ambiguous_alias_in_a_task_list_aborts_the_dispatch_with_a_location() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project_agents = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&project_agents).expect("mkdir");
        std::fs::write(
            project_agents.join("seer.md"),
            "---\nname: seer\ndescription: Sees\naliases: prophet\n---\n\nBody\n",
        )
        .expect("write");
        std::fs::write(
            project_agents.join("augur.md"),
            "---\nname: augur\ndescription: Augurs\naliases: prophet\n---\n\nBody\n",
        )
        .expect("write");

        let tool = scoped_tool(dir.path()).await;
        let params = SubagentToolParams {
            tasks: Some(vec![
                serde_json::json!({ "agent": "scout", "task": "a" }),
                serde_json::json!({ "agent": "prophet", "task": "b" }),
            ]),
            ..serde_json::from_value(serde_json::json!({})).expect("default params")
        };
        let err = tool
            .canonicalize_execution_params(&params, dir.path())
            .await
            .expect_err("an ambiguous alias must abort the dispatch");
        assert_eq!(
            err.to_string(),
            "Ambiguous agent alias 'prophet': augur, seer (task 2)",
            "the ambiguity must name its position in the fan-out"
        );
    }

}
