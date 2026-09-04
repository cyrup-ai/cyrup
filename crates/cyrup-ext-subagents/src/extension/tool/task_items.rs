//! Parsing the tool's `tasks[]`/`chain[]` arrays into lowered [`RunnerStep`] specs, plus the
//! per-item override normalization and the spawn-count billing that reads them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyrup_core::{ModelId, ToolError};

use crate::background::RunId;
use crate::discovery::types::ChainStepConfig;
use crate::extension::executor::paths::{expand_tilde, resolve_against_process_cwd};
use crate::extension::tool::params::SubagentToolParams;
use crate::registration::SubagentExtensionConfig;
use crate::spawn::chain_graph::{GroupStepResult, ParallelGroupSpec, RunnerStep, SingleStepSpec};

// SUBA-041 / SUBA-N04 — the SINGLE-mode `acceptance` param's lowering (pi `AcceptanceOverride`,
// `schemas.ts:80-93`, applied at `subagent-executor.ts:1418`).
//
// The single implementation lives in `crate::exec::acceptance::lower_acceptance_input` so the
// chain/parallel/background STEP path (`background/runner_main.rs::ExecSingleStepExecutor::
// run_single`, pi `chain-execution.ts:421,1401` @v0.43.0) shares it verbatim instead of growing a second,
// drifting parser — SUBA-N04's root cause was exactly a step path that lowered nothing at all. See
// that function for the level mapping and the [CYRUP-DELTA] on pi's richer `AcceptanceConfig`
// fields.

// -------------------------------------------------------------------------------------------------
// Tool-driven PARALLEL (`tasks[]`) and CHAIN (`chain[]`) item parsing + routing (Tier 1)
//
// The `subagent` tool carries `tasks[]`/`chain[]` as raw `Vec<serde_json::Value>` (T0.5 kept the
// per-item shape untyped); this section is the typed lowering into `SingleStepSpec`/`RunnerStep`
// the parallel/chain dispatch arms route through. Faithful port of the pi per-item mapping in
// `subagent-executor.ts` (`params.tasks` -> parallel group, `expandTopLevelTaskCounts`,
// `findDuplicateParallelOutputPath`) and `schemas.ts`'s `TaskItem`/`ParallelTaskSchema`/`ChainItem`.
// -------------------------------------------------------------------------------------------------

/// One parsed `tasks[]` element (top-level PARALLEL) or `parallel[]` element (a static parallel
/// group inside a `chain[]` step) — the union of pi's `TaskItem` (`schemas.ts:78-90`) and
/// `ParallelTaskSchema` (`schemas.ts:133-152`).
///
/// Fields with a [`SingleStepSpec`] home reach the child today: `agent`/`task`/`cwd`/`model`/
/// `skill`/`as`(named output)/`output`/`outputMode`/`reads`/`acceptance`/`outputSchema`, plus
/// `count` (a fan-out WIDTH multiplier applied by expansion, never a per-step spec field).
/// `output` doubles as the input to duplicate-output-path rejection (pi
/// `findDuplicateParallelOutputPath`) and as the step's `output_path`.
///
/// `skill` joined that list with the SUBA-N03-shaped fix in [`tool_task_to_spec`] — it had been
/// parsed and then hardcoded away, which is why the note here used to group it with the genuinely
/// unplumbed fields. `progress`/`phase`/`label` remain parsed (so the shape is accepted) but have
/// no [`SingleStepSpec`] home yet: pi carries `progress` as a per-step behaviour override
/// (`subagent-executor.ts:3176`) and `phase`/`label` as status/graph render labels
/// (`schemas.ts:136-137` @v0.43.0), and neither has a field on this port's step spec to land on.
///
/// `#[serde(default)]` on every optional field keeps parsing permissive, matching pi's TypeBox
/// `Type.Optional` shape.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolTaskItem {
    agent: String,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    output_mode: Option<String>,
    #[serde(default)]
    reads: Option<serde_json::Value>,
    #[serde(default)]
    progress: Option<bool>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    skill: Option<serde_json::Value>,
    #[serde(default)]
    acceptance: Option<serde_json::Value>,
    #[serde(default, rename = "as")]
    as_output: Option<String>,
    #[serde(default)]
    output_schema: Option<serde_json::Value>,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

impl ToolTaskItem {
    /// Read every parsed field at least once so the workspace `dead_code` lint (under `-D warnings`)
    /// stays satisfied for the fields not yet plumbed to the child (`progress`/`phase`/`label` —
    /// `skill` is now threaded by [`tool_task_to_spec`] and no longer needs this to stay live)
    /// — the same self-documenting pattern [`SubagentToolParams::provided_keys`] uses.
    /// Returns the per-item keys actually supplied, for diagnostics.
    pub(crate) fn provided_keys(&self) -> Vec<&'static str> {
        let mut keys = vec!["agent"];
        if self.task.is_some() {
            keys.push("task");
        }
        if self.cwd.is_some() {
            keys.push("cwd");
        }
        if self.count.is_some() {
            keys.push("count");
        }
        if self.output.is_some() {
            keys.push("output");
        }
        if self.output_mode.is_some() {
            keys.push("outputMode");
        }
        if self.reads.is_some() {
            keys.push("reads");
        }
        if self.progress.is_some() {
            keys.push("progress");
        }
        if self.model.is_some() {
            keys.push("model");
        }
        if self.skill.is_some() {
            keys.push("skill");
        }
        if self.acceptance.is_some() {
            keys.push("acceptance");
        }
        if self.as_output.is_some() {
            keys.push("as");
        }
        if self.output_schema.is_some() {
            keys.push("outputSchema");
        }
        if self.phase.is_some() {
            keys.push("phase");
        }
        if self.label.is_some() {
            keys.push("label");
        }
        keys
    }
}

/// Parse a raw `tasks[]`/`parallel[]` JSON array into typed [`ToolTaskItem`]s. When
/// `task_required` (top-level PARALLEL, where pi's `TaskItem.task` is a required string), an
/// element with no non-empty `task` is rejected; inside a chain parallel group `task` is optional
/// (defaults to the prior step's output downstream).
pub(crate) fn parse_tool_task_items(
    raw: &[serde_json::Value],
    task_required: bool,
) -> Result<Vec<ToolTaskItem>, ToolError> {
    let mut items = Vec::with_capacity(raw.len());
    for (i, value) in raw.iter().enumerate() {
        let item: ToolTaskItem = serde_json::from_value(value.clone())
            .map_err(|e| ToolError::new(format!("invalid parallel task at index {i}: {e}")))?;
        // Touch the not-yet-plumbed fields so `dead_code` stays satisfied and a caller can see the
        // exact shape parsed (the fields themselves are Tier 4/5 wire-ups).
        let _ = item.provided_keys();
        if task_required && item.task.as_deref().unwrap_or("").is_empty() {
            return Err(ToolError::new(format!(
                "tasks[{i}] requires a non-empty 'task' (top-level PARALLEL mode)"
            )));
        }
        items.push(item);
    }
    Ok(items)
}

/// pi `expandTopLevelTaskCounts` (`subagent-executor.ts:1986-2000`): repeat each task `count` times
/// (default 1), erroring on `count < 1` with pi's exact message. `count` is stripped from each
/// expanded clone (it is a width hint, never carried onto the concrete task).
pub(crate) fn expand_top_level_task_counts(
    items: Vec<ToolTaskItem>,
) -> Result<Vec<ToolTaskItem>, String> {
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let count = item.count.unwrap_or(1);
        if count < 1 {
            return Err(format!("tasks[{i}].count must be an integer >= 1"));
        }
        for _ in 0..count {
            let mut clone = item.clone();
            clone.count = None;
            out.push(clone);
        }
    }
    Ok(out)
}

/// pi `expandChainParallelCounts` (`subagent-executor.ts:2002-2025`): the same per-task `count`
/// fan-out applied to a static parallel group inside a `chain[]` step, with pi's exact per-step
/// error message.
fn expand_chain_parallel_counts(
    items: Vec<ToolTaskItem>,
    step_index: usize,
) -> Result<Vec<ToolTaskItem>, ToolError> {
    let mut out = Vec::with_capacity(items.len());
    for (j, item) in items.into_iter().enumerate() {
        let count = item.count.unwrap_or(1);
        if count < 1 {
            return Err(ToolError::new(format!(
                "chain[{step_index}].parallel[{j}].count must be an integer >= 1"
            )));
        }
        for _ in 0..count {
            let mut clone = item.clone();
            clone.count = None;
            out.push(clone);
        }
    }
    Ok(out)
}

/// pi `findDuplicateParallelOutputPath` (`subagent-executor.ts:2921-2944`): two parallel tasks
/// resolving their output to the same path is rejected BEFORE any child spawns, with pi's exact
/// message. A string `"false"` (or a boolean `false`/null/absent) means "no output file" and never
/// collides (pi treats string `"false"` as disabled — `parallel-execution.test.ts:289`).
pub(crate) fn find_duplicate_parallel_output(items: &[ToolTaskItem]) -> Option<String> {
    let mut seen: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        let Some(path) = tool_output_path_string(item.output.as_ref()) else {
            continue;
        };
        if let Some((prev_i, prev_agent)) = seen.get(&path) {
            return Some(format!(
                "Parallel tasks {} ({}) and {} ({}) resolve output to the same path: {}. Use \
                 distinct output paths.",
                prev_i + 1,
                prev_agent,
                i + 1,
                item.agent,
                path
            ));
        }
        seen.insert(path, (i, item.agent.clone()));
    }
    None
}

/// The output-file path a task's `output` value resolves to, or `None` when it disables output (a
/// boolean, null, empty string, or the string `"false"` sentinel — all "no file", pi's own rule).
fn tool_output_path_string(output: Option<&serde_json::Value>) -> Option<String> {
    match output {
        Some(serde_json::Value::String(s)) if !s.is_empty() && s != "false" => Some(s.clone()),
        _ => None,
    }
}

/// Lower one [`ToolTaskItem`] to a [`SingleStepSpec`] — the fields with a spec home only. The
/// per-task `model` override reaches the child via `SingleStepSpec::model` (honored by
/// `ExecSingleStepExecutor::run_single`'s `model_override`), exactly as the slash `[model=…]` path.
pub(crate) fn tool_task_to_spec(item: &ToolTaskItem) -> SingleStepSpec {
    SingleStepSpec {
        // SUBA-N03 shape, third path: pi's `ParallelTaskSchema` advertises `skill`
        // (`extension/schemas.ts:146` @v0.43.0, the same `SkillOverride` the top-level `skill`
        // param uses) and HONOURS it — `const skillOverrides = params.tasks.map((task) =>
        // normalizeSkillInput(task.skill))` feeds the per-task step override at
        // `runs/foreground/subagent-executor.ts:2399,2404` (the `runInBackground` lowering) and
        // again at `:3169-3170,:3177` (`...(skillOverrides[index] !== undefined ? { skills:
        // skillOverrides[index] } : {})`, the foreground parallel lowering).
        //
        // This was hardcoded `None`, so every `tasks[]` element's `skill` — and every
        // `chain[].parallel[]` element's — was deserialized into `ToolTaskItem::skill`, counted by
        // `ToolTaskItem::provided_keys`, and then dropped on the floor: the child ran with its
        // persona's own `skills:` list no matter what the call asked for, and `skill: false` (pi's
        // "no skills at all" form) could not turn them off. Exactly the advertised-and-silently-
        // dropped defect SUBA-041/SUBA-N03 exist to remove, surviving on a third path.
        //
        // `normalize_skill_input` is the SAME normalization the SINGLE path uses (pi's own
        // `normalizeSkillInput`), so the tri-state survives intact: `None` = omitted, inherit the
        // persona's `skills:`; `Some(vec![])` = explicit `skill: false`, no skills; `Some(names)` =
        // replace the persona's list. `SingleStepSpec::skills` is read on BOTH outcomes — the
        // foreground walk (`extension.rs` graph lowering) and the detached hop-2 runner
        // (`background/runner_main.rs:2549`) — and lands on `RunOptions::skills`, whose
        // `opts.skills ?? agent.skills` fallthrough (`exec/mod.rs:3650`, pi `execution.ts:1413`)
        // is what makes the empty list mean "none" rather than "unset".
        skills: normalize_skill_input(item.skill.as_ref()),
        session_dir: None,
        agent: item.agent.clone(),
        task: item.task.clone().unwrap_or_default(),
        cwd: item.cwd.as_ref().map(PathBuf::from),
        model: item.model.clone().map(ModelId::from),
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: item.output_schema.clone(),
        output: item.as_output.clone(),
        // pi's `output` (the output FILE path) vs `as` (the registry KEY, mapped just above):
        // `tool_output_path_string` normalizes the boolean/`"false"`/empty "no file" sentinels to
        // `None` (the same normalization `find_duplicate_parallel_output` uses), so a task with a
        // real output path reaches the child and drives the file-output handoff.
        output_path: tool_output_path_string(item.output.as_ref()),
        output_mode: parse_tool_output_mode(item.output_mode.as_deref()),
        reads: parse_tool_reads(item.reads.as_ref()),
        acceptance: parse_tool_acceptance(item.acceptance.as_ref()),
        context: None,
        agent_scope: None,
    }
}

/// pi `resolveSingleRunOutputBaseDir` (`runs/foreground/subagent-executor.ts:2838-2842` @v0.43.0):
/// the base directory a RELATIVE SINGLE-mode `output` path resolves against — the configured
/// `singleRunOutputBaseDir` (tilde-expanded, `path.resolve`d) when set, else
/// `<artifactsDir>/outputs/<runId>`.
///
/// Deliberately NOT the run cwd, so a bare `report.md` never lands in the user's repo.
///
/// SUBA-N03 extracted this out of `run_foreground_impl`. Upstream calls the SAME function from
/// three sites — its foreground path (`:2882`), its `runInBackground` async-single branch
/// (`:2861`), and its collapsed-fanout branch (`:1946`) — all passing that call's own `runId`. Now
/// that cyrup's async SINGLE path also resolves an output path, one implementation with a `run_id`
/// parameter is the only way the two paths cannot drift.
pub(crate) fn resolve_single_run_output_base_dir(
    cfg: &SubagentExtensionConfig,
    artifacts_dir: &Path,
    run_id: &RunId,
) -> PathBuf {
    match cfg.single_run_output_base_dir.as_deref() {
        Some(configured) => {
            let expanded = expand_tilde(&configured.to_string_lossy());
            resolve_against_process_cwd(&expanded).unwrap_or(expanded)
        }
        None => artifacts_dir.join("outputs").join(run_id.as_str()),
    }
}

/// pi's SINGLE-mode session-ROOT resolution (`runs/foreground/subagent-executor.ts:5044-5052`
/// @v0.34.0): an explicit `sessionDir` param is tilde-expanded and `path.resolve`d and becomes the
/// root VERBATIM; a configured `default_session_dir` is instead scoped per run
/// (`path.join(base, runId)`) so two runs sharing one configured base cannot share a session store.
///
/// Returns the ROOT, not a child's directory — the caller appends pi's own per-index leaf
/// (`sessionDirForIndex(i)` → `run-<i>`), because that leaf differs between the foreground path
/// (which knows it is index 0) and a background run (whose steps are numbered by the hop-2 walker).
///
/// **[CYRUP-DELTA]** pi's third rung — `deps.getSubagentSessionRoot(parentSessionFile)`, an
/// always-present default derived from the PARENT session file — has no analog at this seam (no
/// parent-session-file plumbing reaches the extension), so with neither an explicit `sessionDir`
/// nor a configured default this returns `None` and [`crate::exec::build_attempt_spawn_plan`] falls
/// to pi's own `--no-session` branch (`runs/shared/pi-args.ts:105-106`). The isolation outcome is
/// the same one pi's scoped root buys: the child never writes into the orchestrator's session store.
///
/// SUBA-N03 extracted this out of `run_foreground_impl` so the async SINGLE path resolves the
/// identical root from the identical inputs rather than growing a second copy.
pub(crate) fn resolve_single_run_session_root(
    cfg: &SubagentExtensionConfig,
    requested: Option<&str>,
    run_id: &RunId,
) -> Option<PathBuf> {
    requested
        .filter(|raw| !raw.is_empty())
        .map(|raw| {
            let expanded = expand_tilde(raw);
            resolve_against_process_cwd(&expanded).unwrap_or(expanded)
        })
        .or_else(|| {
            cfg.default_session_dir
                .as_deref()
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| {
                    let expanded = expand_tilde(&path.to_string_lossy());
                    resolve_against_process_cwd(&expanded)
                        .unwrap_or(expanded)
                        .join(run_id.as_str())
                })
        })
}

pub(crate) fn parse_tool_output_mode(
    raw: Option<&str>,
) -> Option<crate::discovery::types::OutputMode> {
    match raw {
        Some("inline") => Some(crate::discovery::types::OutputMode::Inline),
        Some("file-only") => Some(crate::discovery::types::OutputMode::FileOnly),
        _ => None,
    }
}

fn parse_tool_reads(raw: Option<&serde_json::Value>) -> Option<Vec<PathBuf>> {
    match raw {
        Some(serde_json::Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect(),
        ),
        // `false`/null/absent (disabled) — no pre-declared read paths.
        _ => None,
    }
}

/// Carry a `tasks[]`/`chain[]` item's raw `acceptance` value onto its [`SingleStepSpec`] (pi keeps
/// `task.acceptance`/`step.acceptance` raw on the step and hands it to `runSync` unmodified,
/// `chain-execution.ts:333,1195` @v0.34.0).
///
/// SUBA-N04: this used to keep ONLY the level-string form, so `{ level: "verified", verify: [{
/// command: "cargo test" }] }` and the `false` shorthand were both discarded at the tool boundary —
/// and the string form that did survive was then dropped again in the runner. Every form pi's
/// `AcceptanceOverride` union admits now reaches
/// [`crate::exec::acceptance::lower_acceptance_input`] at dispatch. Only JSON `null` and the empty
/// string (neither of which is a policy) normalize to `None`, i.e. pi's `undefined`.
fn parse_tool_acceptance(raw: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    match raw {
        Some(serde_json::Value::String(s)) if s.is_empty() => None,
        Some(serde_json::Value::Null) | None => None,
        Some(value) => Some(value.clone()),
    }
}

// -------------------------------------------------------------------------------------------------
// SUBA-041: the SINGLE-mode override normalizers (`output`/`outputMode`/`skill`/`acceptance`).
//
// pi's `runSinglePath` runs each raw tool param through one small shared normalizer before it ever
// reaches `runSync` (`single-output.ts:54-77`, `skills.ts:716-740`, `acceptance.ts:176-303`); these
// are those normalizers, ported 1:1 so the top-level SINGLE surface and the `tasks[]`/`chain[]` item
// surface agree on what a given value means.
// -------------------------------------------------------------------------------------------------

/// pi `normalizeSingleOutputOverride` (`runs/shared/single-output.ts:54-62`) composed with
/// `runSinglePath`'s own `rawOutput = params.output !== undefined ? params.output :
/// agentConfig.output` (`subagent-executor.ts:3562`).
///
/// Returns the effective output FILE name/path, or `None` for every "no output file" form: an
/// explicit `false`/`"false"`, an empty string, a non-string/non-boolean value, and — for
/// `true`/`"true"` — a persona that declares no `output:` of its own. `true`/`"true"` means "use the
/// persona's own declared output", which is why `default_output` is threaded in.
pub(crate) fn normalize_single_output_override(
    output: Option<&serde_json::Value>,
    default_output: Option<&str>,
) -> Option<String> {
    // pi's `params.output !== undefined ? params.output : agentConfig.output`: an OMITTED param
    // falls back to the persona's own declared output path, which then re-enters the same
    // normalizer as a plain string.
    let Some(raw) = output else {
        return default_output.filter(|s| !s.is_empty()).map(str::to_string);
    };
    match raw {
        serde_json::Value::Bool(false) => None,
        serde_json::Value::Bool(true) => {
            default_output.filter(|s| !s.is_empty()).map(str::to_string)
        }
        serde_json::Value::String(s) if s == "false" => None,
        serde_json::Value::String(s) if s == "true" => {
            default_output.filter(|s| !s.is_empty()).map(str::to_string)
        }
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// pi `resolveSingleOutputPath` (`runs/shared/single-output.ts:64-77`), specialized to the one call
/// shape `runSinglePath` uses (`subagent-executor.ts:3666`): a `relativeBaseDir` is ALWAYS supplied
/// there — `resolveSingleRunOutputBaseDir`'s configured `singleRunOutputBaseDir` or
/// `<artifactsDir>/outputs/<runId>` (`:2203-2207`) — so the runtime-cwd / requested-cwd fallback
/// rungs of the upstream function are unreachable on this path and are not reproduced. An ABSOLUTE
/// output is used verbatim; a relative one resolves against `base_dir`, NOT against the run cwd.
pub(crate) fn resolve_single_output_path(output: Option<&str>, base_dir: &Path) -> Option<PathBuf> {
    let output = output.filter(|s| !s.is_empty() && *s != "false" && *s != "true")?;
    let candidate = Path::new(output);
    if candidate.is_absolute() {
        Some(candidate.to_path_buf())
    } else {
        Some(base_dir.join(candidate))
    }
}

/// pi `normalizeSkillInput` (`agents/skills.ts:716-740`): `false` → the explicit "no skills at all"
/// form (`Some(vec![])`, which `runSinglePath` spells `effectiveSkills = []`,
/// `subagent-executor.ts:3676-3680`); `true`/absent → `None` (inherit the persona's own `skills:`);
/// an array or a comma-separated string → the trimmed, non-empty, order-preserving de-duplicated
/// names. A string that opens on `[` is first tried as JSON (models routinely serialize the array
/// form as a string, and a naive comma-split would embed brackets and quotes into the names).
pub(crate) fn normalize_skill_input(raw: Option<&serde_json::Value>) -> Option<Vec<String>> {
    fn dedup(names: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for name in names {
            let trimmed = name.trim();
            if !trimmed.is_empty() && !out.iter().any(|existing| existing == trimmed) {
                out.push(trimmed.to_string());
            }
        }
        out
    }
    match raw {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(true)) => None,
        Some(serde_json::Value::Bool(false)) => Some(Vec::new()),
        Some(serde_json::Value::Array(items)) => Some(dedup(
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>(),
        )),
        Some(serde_json::Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.starts_with('[')
                && let Ok(serde_json::Value::Array(items)) =
                    serde_json::from_str::<serde_json::Value>(trimmed)
            {
                return Some(dedup(
                    items
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                ));
            }
            Some(dedup(s.split(',').map(str::to_string)))
        }
        // Any other JSON shape is not a skill selector at all — pi's TypeBox union would have
        // rejected it; degrade to "inherit the persona's list" rather than inventing names.
        Some(_) => None,
    }
}

/// Translate the tool's `chain[]` array into a `Vec<RunnerStep>`: a sequential step for a
/// `{agent, task, …}` element, a [`RunnerStep::ParallelGroup`] for a `{parallel: [...]}` element
/// (with per-task `count` expanded), or a [`RunnerStep::DynamicGroup`] for an `{expand, parallel:
/// {...}, collect}` element (C16) — the SAME `ChainStepConfig` -> [`RunnerStep`] structural bridge
/// [`crate::discovery::chains::chain_step_to_runner_step`] already applies to a saved chain file's
/// steps, reused here so a tool-authored dynamic step gets byte-identical shape validation
/// (`validate_dynamic_step_shape`) and materialization behavior.
pub(crate) fn parse_tool_chain_items(
    raw: &[serde_json::Value],
    default_concurrency: u32,
) -> Result<Vec<RunnerStep>, ToolError> {
    let mut graph = Vec::with_capacity(raw.len());
    for (i, value) in raw.iter().enumerate() {
        let obj = value.as_object();
        if obj.is_some_and(|o| o.contains_key("expand") || o.contains_key("collect")) {
            // pi `dynamic-fanout.ts::hasDynamicFanoutFields`/`validateDynamicStepShape`: an `expand`
            // or `collect` key commits this element to the dynamic-fanout shape — `display` is
            // `i + 1` (1-based), matching every other chain-step diagnostic's own numbering.
            crate::discovery::chains::validate_dynamic_step_shape(value, i + 1, u64::MAX)
                .map_err(ToolError::new)?;
            let config: ChainStepConfig = serde_json::from_value(value.clone()).map_err(|e| {
                ToolError::new(format!("invalid dynamic chain step at index {i}: {e}"))
            })?;
            graph.push(crate::discovery::chains::chain_step_to_runner_step(
                &config,
                default_concurrency,
            ));
            continue;
        }
        match obj.and_then(|o| o.get("parallel")) {
            Some(serde_json::Value::Array(tasks)) => {
                let items = parse_tool_task_items(tasks, false)?;
                let expanded = expand_chain_parallel_counts(items, i)?;
                let steps: Vec<SingleStepSpec> = expanded.iter().map(tool_task_to_spec).collect();
                let concurrency = obj
                    .and_then(|o| o.get("concurrency"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|c| u32::try_from(c).ok())
                    .filter(|c| *c > 0)
                    .unwrap_or(default_concurrency);
                let fail_fast = obj
                    .and_then(|o| o.get("failFast"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let worktree = obj
                    .and_then(|o| o.get("worktree"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                graph.push(RunnerStep::ParallelGroup(ParallelGroupSpec {
                    steps,
                    concurrency,
                    fail_fast,
                    worktree,
                }));
            }
            Some(_) => {
                return Err(ToolError::new(format!(
                    "chain[{i}].parallel must be an array of tasks; the single dynamic-template \
                     form is not wired via the tool in this build yet (Tier 4, C16)."
                )));
            }
            None => {
                let item: ToolTaskItem = serde_json::from_value(value.clone())
                    .map_err(|e| ToolError::new(format!("invalid chain step at index {i}: {e}")))?;
                let _ = item.provided_keys();
                graph.push(RunnerStep::SingleStep(tool_task_to_spec(&item)));
            }
        }
    }
    Ok(graph)
}

/// Render a top-level PARALLEL run's result summary in pi's shape: an `N/M succeeded` header
/// (`subagent-executor.ts:2446`) followed by each task's own output under a `=== Task i: agent ===`
/// section header (`subagent-executor.ts:2443`), in input order (R-SA-051).
pub(crate) fn render_parallel_tool_summary(group: &GroupStepResult, agents: &[String]) -> String {
    let total = group.children.len();
    let ok = group
        .children
        .iter()
        .filter(|c| matches!(c, Some(r) if r.success))
        .count();
    let mut body = String::new();
    for (i, child) in group.children.iter().enumerate() {
        let agent = agents.get(i).map(String::as_str).unwrap_or("?");
        body.push_str(&format!("=== Task {}: {} ===\n", i + 1, agent));
        match child {
            Some(r) if r.success => {
                body.push_str(r.final_output.as_deref().unwrap_or("(no text output)"));
            }
            Some(r) => {
                let err = r.error.as_deref().unwrap_or("unknown error");
                if err.contains("timed out") {
                    body.push_str(&format!("TIMED OUT: {err}"));
                } else {
                    body.push_str(&format!("FAILED: {err}"));
                }
            }
            None => body.push_str("(skipped)"),
        }
        body.push('\n');
        if i + 1 != total {
            body.push('\n');
        }
    }
    format!("{ok}/{total} succeeded\n\n{body}")
}

/// The complete LLM-facing JSON Schema for the `subagent` tool (C8) — a faithful port of pi's
/// exported `SubagentParams` (`schemas.ts:257-357`, after `keepTopLevelParameterDescriptions`
/// pruning). Every top-level parameter pi advertises is present with its top-level description; as
/// of SUBA-N06 there are no withholds left. The nested `tasks[]`/`chain[]`
/// element shapes carry their full structural detail (types, enums, `minimum`s, `items`, `anyOf`
/// unions) with per-node descriptions pruned to keep the provider payload compact, exactly as pi
/// ships it.
///
/// # The SUBA-041 invariant, stated accurately
///
/// This schema must never advertise a parameter [`crate::extension::SubagentTool::route_single`] refuses
/// UNCONDITIONALLY — and it no longer refuses any. All nine SINGLE-mode overrides (`output`,
/// `outputMode`, `skill`, `acceptance`, `share`, `sessionDir`, `artifacts`, `control`,
/// `includeProgress`) reach [`crate::exec::RunOptions`] on the FOREGROUND path and are advertised for that
/// reason.
///
/// SUBA-N06 restored `includeProgress`, the last withhold. It was absent for exactly one reason —
/// [`crate::exec::SingleResult`] carried no progress object for it to include or omit — and that
/// reason is gone: [`crate::exec::AgentProgress::snapshot`] projects the winning attempt's fold
/// into pi's `AgentProgress` wire shape and [`crate::exec::run_sync`] publishes it on
/// [`crate::exec::SingleResult::progress`] under pi's own truthiness gate
/// (`progress: params.includeProgress ? allProgress : undefined`, `subagent-executor.ts:3819`
/// @v0.34.0). It is honoured on the ASYNC path too — the flag rides to the detached hop-2 runner on
/// [`crate::background::runner_main::RunnerConfig::include_progress`] and lands on every step's
/// `RunOptions`, so the persisted `ResultFile`'s `SingleResult`s carry their snapshots. That is
/// STRICTLY MORE than upstream, which never passes `includeProgress` into `executeAsyncSingle`
/// (`subagent-executor.ts:2845-2874` @v0.34.0) because its async return is a "started" message
/// with no results to attach progress to; cyrup's async run does produce a retrievable
/// `SingleResult`, so honouring the flag there is the only reading that is not a silent drop.
///
/// SUBA-N05 restored `control`. It was withheld for exactly one reason — this port had
/// [`crate::registration::ControlConfig`]'s shape but no `resolveControlConfig` and no notice
/// pipeline — and that reason is gone: [`crate::exec::control`] ports
/// `runs/shared/subagent-control.ts` in full, [`crate::exec::control::ControlMonitor`] raises real
/// `ControlEvent`s off the child's NDJSON stream, and
/// [`crate::extension::SubagentExecutor::foreground_control_notifier`] delivers them through
/// [`crate::tui::notices::ControlNoticeState`]. It is honoured on the ASYNC path too — the resolved
/// config rides to the detached hop-2 runner on [`crate::background::runner_main::RunnerConfig`],
/// matching upstream's `executeAsyncSingle(id, { …, controlConfig, … })`
/// (`subagent-executor.ts:2845,2868-2870` @v0.34.0).
///
/// # SUBA-N03: the mode-conditional refusal is gone too
///
/// All nine also reach hop 2 on the BACKGROUND path now (`async: true`, or `asyncByDefault`/
/// `forceTopLevelAsync` making a top-level call background). SIX of them — `output`, `outputMode`,
/// `skill`, `share`, `sessionDir`, `artifacts` — used to be refused loudly and by name there,
/// alongside a seventh refusal covering `timeoutMs`/`maxRuntimeMs`.
///
/// **The justification for that refusal was a fabricated upstream citation.** The comment claimed
/// it mirrored "pi's own precedent of erroring on timeoutMs + async
/// (`subagent-executor.ts:3022`)". At v0.34.0 `subagent-executor.ts:3015-3030` is FOREGROUND
/// intercom-receipt construction, and `git grep` over the whole of v0.34.0 `src/` finds no
/// timeout-vs-async refusal anywhere. Upstream does the OPPOSITE: `extension/schemas.ts:265-266`
/// and `extension/tool-description.ts:25,:73` all say `timeoutMs`/`maxRuntimeMs` apply to
/// "foreground and async/background runs", `runs/background/async-execution.ts:924` arms
/// `deadlineAt = Date.now() + params.timeoutMs` for `executeAsyncSingle`, and `:677` does the same
/// for `executeAsyncChain`.
///
/// The other six were refused for one honest, now-removed reason: the second-hop `RunnerConfig`
/// boundary was strictly NARROWER than the foreground `RunOptions`, so accepting them would have
/// meant the detached runner silently dropping them — the exact defect SUBA-041 names. That
/// boundary now carries upstream's own field set (`spawnRunner({ …, share, sessionDir,
/// artifactsDir, artifactConfig, timeoutMs, deadlineAt, … })` plus the step's own `outputPath`/
/// `outputMode`/`skills`, `runs/background/async-execution.ts:930-996` @v0.34.0):
///
/// * `output`/`outputMode` — resolved parent-side against the run-scoped output base dir
///   ([`resolve_single_run_output_base_dir`], pi `resolveSingleRunOutputBaseDir` `:2203-2207`) onto
///   [`crate::spawn::chain_graph::SingleStepSpec::output_path`]/`output_mode`;
/// * `skill` — [`crate::spawn::chain_graph::SingleStepSpec::skills`];
/// * `sessionDir` — resolved parent-side onto
///   [`crate::spawn::chain_graph::SingleStepSpec::session_dir`];
/// * `share`, `artifacts`, `timeoutMs` — [`crate::background::runner_main::RunnerConfig::share`],
///   `artifacts_dir`/`artifact_config`, and `timeout_ms`/`deadline_at_ms`.
///
/// `acceptance` (SUBA-N04), `control` (SUBA-N05) and `includeProgress` (SUBA-N06) were wired to hop
/// 2 by their own units. The async path additionally now writes the SAME artifact quadruple the
/// foreground path does (pi `runs/background/subagent-runner.ts:879-890,1117-1125` @v0.34.0), which is what
/// gives `artifacts: false` something real to switch off.
///
/// The former refusal was pinned by a test; that test was RE-SCOPED, not deleted — see
/// `tests::a_background_single_run_honours_the_nine_single_mode_overrides`, which asserts the
/// replacement behaviour at the `runner-config.json` filesystem boundary (the entire hop-1 -> hop-2
/// contract), so this schema-vs-behaviour contract still cannot drift silently in either direction.
/// Resolve a chain run's artifact directory, pi's
/// `chainDir: params.chainDir ?? getProjectChainRunsDir(effectiveCwd)`
/// (`subagent-executor.ts:2623` @v0.43.0).
///
/// An explicit caller value WINS and is used verbatim — pi does not rewrite it either (a step's
/// relative `output` is what gets joined against it, at `chain-execution.ts:283`), so a relative
/// `chainDir` stays relative here exactly as it does upstream.
///
/// SUBA-048 / PARITY-GAPS PB-13: the fallback ROOT is now
/// [`crate::artifacts::resolve_chain_runs_dir`], pi's `getChainRunsDir(effectiveCwd,
/// artifactConfig.dir)` (`shared/artifacts.ts:145-158` @v0.43.0) — so the default `project`
/// preference resolves to `<cwd>/.cyrup-subagents/chain-runs` as upstream's does, and `session` /
/// `temp` collapse onto the scoped temp root. Before this, the fallback was unconditionally the
/// temp root: a chain run's artifacts landed under `$TMPDIR/.../chain-runs/<cwd_key>/<runId>`,
/// invisible to the project and swept by OS tmp cleanup, and `project_chain_runs_dir` had zero
/// references anywhere in the crate.
///
/// [CYRUP-DELTA] the fallback is a PER-RUN subdirectory rather than pi's flat chain-runs dir, so
/// `{chain_dir}` is unique per run and `artifacts::cleanup_old_chain_dirs` can housekeep by age.
/// Only that extra level differs; the ROOT and the override path are now both pi-identical.
pub(crate) fn resolve_chain_dir(
    override_dir: Option<PathBuf>,
    cwd: &Path,
    run_id: &RunId,
    preference: crate::artifacts::ArtifactDirPreference,
) -> PathBuf {
    override_dir.unwrap_or_else(|| {
        crate::artifacts::resolve_chain_runs_dir(cwd, preference).join(run_id.as_str())
    })
}

/// pi `countRequestedSubagentSpawns` (`runs/foreground/subagent-executor.ts:439-447`): how many
/// subagent spawns ONE accepted execution dispatch will charge against the session budget.
///
/// * PARALLEL (`tasks[]`) → one spawn per task.
/// * CHAIN (`chain[]`) → per step: a **dynamic-parallel** step (pi `isDynamicParallelStep`:
///   `expand` + `collect` + a NON-array `parallel`) is billed its worst case, `expand.maxItems ??
///   config.chain.dynamicFanout.maxItems ?? 0`; any other step is billed
///   `getStepAgents(step).length` — the length of its `parallel[]` task array for a static parallel
///   step, otherwise `1` for the single `agent` a sequential step names (pi returns `[step.agent]`,
///   length 1, even when `agent` is absent).
/// * SINGLE → `1` when an `agent` was named, else `0`.
///
/// Saturating throughout: a caller cannot overflow the counter by declaring an absurd `maxItems`.
pub(crate) fn count_requested_subagent_spawns(
    params: &SubagentToolParams,
    cfg: &SubagentExtensionConfig,
) -> u32 {
    if let Some(tasks) = params.tasks.as_ref() {
        return u32::try_from(tasks.len()).unwrap_or(u32::MAX);
    }
    if let Some(chain) = params.chain.as_ref() {
        return chain.iter().fold(0u32, |total, step| {
            total.saturating_add(chain_step_requested_spawns(step, cfg))
        });
    }
    u32::from(params.agent.is_some())
}

/// One chain step's spawn charge — the body of [`count_requested_subagent_spawns`]'s `chain` fold
/// (pi's `chain.reduce(...)`, `subagent-executor.ts:286-291`), kept separate so the dynamic-fanout
/// worst case and the static `getStepAgents` count stay individually readable.
fn chain_step_requested_spawns(step: &serde_json::Value, cfg: &SubagentExtensionConfig) -> u32 {
    // pi `isDynamicParallelStep` (`shared/settings.ts:161-163`), the same predicate
    // `discovery::chains` already ports: `expand` + `collect` + a NON-array `parallel`.
    let is_dynamic = step.get("expand").is_some()
        && step.get("collect").is_some()
        && step.get("parallel").is_some_and(|p| !p.is_array());
    if is_dynamic {
        return step
            .get("expand")
            .and_then(|expand| expand.get("maxItems"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .or_else(|| cfg.dynamic_fanout_max_items())
            .unwrap_or(0);
    }
    // pi `getStepAgents(step).length` (`shared/settings.ts:136-144`): a static parallel step's
    // `parallel[]` length, else exactly one agent.
    step.get("parallel")
        .and_then(serde_json::Value::as_array)
        .map_or(1, |tasks| u32::try_from(tasks.len()).unwrap_or(u32::MAX))
}

/// The SAME charge as [`count_requested_subagent_spawns`], counted over an ALREADY-LOWERED
/// [`RunnerStep`] graph — the shape this crate's slash surface (`/chain`, `/parallel`,
/// `/run-chain`) hands to [`crate::extension::SubagentExecutor::run_or_background_graph`] (SUBA-002).
///
/// pi needs no lowered-form counter because every slash handler funnels back into the very same
/// `executor.execute` the tool uses (`slash/slash-commands.ts` `runSlashSubagent` ->
/// `requestSlashRun` -> the bridge wired at `extension/index.ts:512-517` ->
/// `executeSubagentCollapsed` -> `executor.execute`), so its single `reserveSubagentSpawns`
/// (`subagent-executor.ts:266-282`, called at `:3434-3441`) always sees the RAW `SubagentParamsLike`
/// and counts it with `countRequestedSubagentSpawns` (`:284-292`). This crate's slash surface parses
/// and lowers to [`RunnerStep`] before it reaches execution, so pi's per-step rule is applied to the
/// lowered form instead — arm for arm:
///
/// * [`RunnerStep::SingleStep`] → `1` (pi `getStepAgents(step).length` for a sequential step).
/// * [`RunnerStep::ParallelGroup`] → its static width (pi's `parallel[]` array length).
/// * [`RunnerStep::DynamicGroup`] → its worst case, `max_items` else
///   `config.chain.dynamicFanout.maxItems`, else `0` — pi's `isDynamicParallelStep` arm.
/// * [`RunnerStep::ImportAsyncRoot`] → `0`. [CYRUP-DELTA, no upstream analog] R-SA-097's
///   chain-root attachment POLLS an already-launched async run and never spawns a child of its own,
///   so billing it would charge a spawn that provably cannot happen.
///
/// Saturating throughout, exactly like [`count_requested_subagent_spawns`].
pub(crate) fn count_graph_requested_spawns(
    graph: &[RunnerStep],
    cfg: &SubagentExtensionConfig,
) -> u32 {
    graph.iter().fold(0u32, |total, step| {
        let step_charge = match step {
            RunnerStep::SingleStep(_) => 1,
            RunnerStep::ParallelGroup(group) => {
                u32::try_from(group.steps.len()).unwrap_or(u32::MAX)
            }
            RunnerStep::DynamicGroup(group) => group
                .max_items
                .or_else(|| cfg.dynamic_fanout_max_items())
                .unwrap_or(0),
            RunnerStep::ImportAsyncRoot(_) => 0,
        };
        total.saturating_add(step_charge)
    })
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
    use crate::exec::acceptance::lower_acceptance_input as parse_single_acceptance;

    /// Regression (C16, dossier "Dynamic fanout unusable via the subagent tool"): a `chain[]`
    /// element carrying pi's `expand`/`parallel`/`collect` dynamic-fanout shape must now parse into
    /// a real [`RunnerStep::DynamicGroup`] — pre-fix, `parse_tool_chain_items` rejected ANY
    /// `expand`/`collect` key outright with `"not wired via the tool in this build yet (Tier 4,
    /// C16)"`, so a tool caller could never express dynamic fanout at all, only saved chain files
    /// could (`crate::discovery::chains::chain_step_to_runner_step`, `/run-chain`).
    #[test]
    fn parse_tool_chain_items_parses_a_dynamic_expand_collect_item_into_a_dynamic_group() {
        let raw = vec![serde_json::json!({
            "expand": {
                "from": { "output": "targets", "path": "/items" },
                "item": "target",
                "key": "/path",
                "maxItems": 4
            },
            "parallel": { "agent": "reviewer", "task": "Review {target.path}" },
            "collect": { "as": "reviews" }
        })];

        let graph = parse_tool_chain_items(&raw, 4).expect(
            "a well-formed expand/parallel/collect chain[] item must now parse into a \
             RunnerStep::DynamicGroup rather than erroring — the pre-fix 'not wired via the tool' \
             rejection",
        );
        assert_eq!(graph.len(), 1);
        match &graph[0] {
            RunnerStep::DynamicGroup(spec) => {
                assert_eq!(spec.expand, "outputs.targets/items");
                assert_eq!(spec.collect, "reviews");
                assert_eq!(spec.item.as_deref(), Some("target"));
                assert_eq!(spec.key.as_deref(), Some("/path"));
                assert_eq!(spec.max_items, Some(4));
                assert_eq!(spec.template.agent, "reviewer");
                assert_eq!(spec.template.task, "Review {target.path}");
            }
            other => panic!("expected RunnerStep::DynamicGroup, got: {other:?}"),
        }
    }

    /// Companion to the test above: a MALFORMED dynamic-fanout shape (missing `expand.from`) must
    /// still be rejected with pi's exact `validateDynamicStepShape` diagnostic, not silently
    /// mis-parsed into a bogus sequential/step-less graph — proving the new tool-parsing path
    /// reuses the SAME shape validation `discovery::chains::validate_dynamic_step_shape` already
    /// applies to saved chain files, rather than a looser, unvalidated conversion.
    #[test]
    fn parse_tool_chain_items_rejects_a_malformed_dynamic_item_with_pis_shape_error() {
        let raw = vec![serde_json::json!({
            "expand": { "item": "target" },
            "parallel": { "agent": "reviewer", "task": "Review {target}" },
            "collect": { "as": "reviews" }
        })];

        let err = parse_tool_chain_items(&raw, 4)
            .expect_err("a dynamic item missing expand.from must still be rejected");
        let message = err.to_string();
        assert!(
            message.contains("requires expand.from"),
            "must surface pi's exact shape-validation diagnostic: {message}"
        );
    }

    // ---- SUBA-041: the SINGLE-mode override normalizers ----

    /// pi `normalizeSingleOutputOverride` (`single-output.ts:54-62`) + `runSinglePath`'s persona
    /// fallback (`subagent-executor.ts:3562`), rule by rule.
    #[test]
    fn normalize_single_output_override_ports_pis_five_cases() {
        // Omitted param → the persona's own declared output.
        assert_eq!(
            normalize_single_output_override(None, Some("persona.md")),
            Some("persona.md".to_string())
        );
        assert_eq!(normalize_single_output_override(None, None), None);
        // Explicit disable, both spellings.
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!(false)), Some("persona.md")),
            None
        );
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!("false")), Some("persona.md")),
            None
        );
        // `true`/`"true"` means "the persona's own output".
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!(true)), Some("persona.md")),
            Some("persona.md".to_string())
        );
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!("true")), None),
            None
        );
        // A real path wins over the persona default.
        assert_eq!(
            normalize_single_output_override(
                Some(&serde_json::json!("report.md")),
                Some("persona.md")
            ),
            Some("report.md".to_string())
        );
        // An empty string is "no output".
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!("")), None),
            None
        );
    }

    /// pi `resolveSingleOutputPath` (`single-output.ts:64-77`) as `runSinglePath` calls it: a
    /// RELATIVE output resolves against the run's own scoped base dir, never the run cwd; an
    /// absolute one is used verbatim; the disable sentinels never produce a path.
    #[test]
    fn resolve_single_output_path_resolves_relative_against_the_run_output_base_dir() {
        let base = Path::new("/scoped/outputs/run123");
        assert_eq!(
            resolve_single_output_path(Some("report.md"), base),
            Some(PathBuf::from("/scoped/outputs/run123/report.md"))
        );
        assert_eq!(
            resolve_single_output_path(Some("/abs/report.md"), base),
            Some(PathBuf::from("/abs/report.md"))
        );
        assert_eq!(resolve_single_output_path(None, base), None);
        assert_eq!(resolve_single_output_path(Some("false"), base), None);
        assert_eq!(resolve_single_output_path(Some(""), base), None);
    }

    /// pi `normalizeSkillInput` (`agents/skills.ts:716-740`) — including the JSON-encoded-array
    /// guard models routinely trip, and the `false` → "no skills at all" form.
    #[test]
    fn normalize_skill_input_ports_pis_union() {
        assert_eq!(normalize_skill_input(None), None);
        assert_eq!(normalize_skill_input(Some(&serde_json::json!(true))), None);
        assert_eq!(
            normalize_skill_input(Some(&serde_json::json!(false))),
            Some(Vec::new())
        );
        assert_eq!(
            normalize_skill_input(Some(&serde_json::json!(["a", " b ", "", "a"]))),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            normalize_skill_input(Some(&serde_json::json!("rust, testing ,rust"))),
            Some(vec!["rust".to_string(), "testing".to_string()])
        );
        // A JSON-encoded array arriving as a string must NOT be comma-split into `["a"` / `"b"]`.
        assert_eq!(
            normalize_skill_input(Some(&serde_json::json!(r#"["a","b"]"#))),
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    /// SUBA-041: the `acceptance` param lowers onto a real [`crate::exec::acceptance::AcceptanceContract`]
    /// (never the heuristic fallback) for every explicit level, defers for `"auto"`, and refuses a
    /// malformed policy with pi's own `validateAcceptanceInput` text.
    #[test]
    fn parse_single_acceptance_lowers_levels_and_validates() {
        use crate::exec::acceptance::AcceptanceStatus;

        // "auto" is pi's "omitted means auto-inferred" — defer to the heuristic default.
        assert_eq!(
            parse_single_acceptance(&serde_json::json!("auto")),
            Ok(None)
        );

        let checked = parse_single_acceptance(&serde_json::json!("checked"))
            .expect("valid level")
            .expect("an explicit level yields a contract");
        assert_eq!(checked.required_level, AcceptanceStatus::Checked);
        assert!(
            checked.explicit,
            "an explicit param arms R-SA-033's exit-code correction"
        );

        // `false` is pi's `level: "none"` shorthand: explicit, but nothing to gate.
        let disabled = parse_single_acceptance(&serde_json::json!(false))
            .expect("valid")
            .expect("a contract");
        assert_eq!(disabled.required_level, AcceptanceStatus::NotRequired);
        assert!(disabled.explicit);
        assert!(disabled.is_no_op());

        // The object form carries `verify[].command` onto the contract.
        let verified = parse_single_acceptance(&serde_json::json!({
            "level": "verified",
            "verify": [{ "id": "t", "command": "cargo test" }]
        }))
        .expect("valid")
        .expect("a contract");
        assert_eq!(verified.required_level, AcceptanceStatus::Verified);
        assert_eq!(verified.verify, vec!["cargo test".to_string()]);

        // pi's verbatim validation failures (`acceptance.ts:182,193`).
        assert_eq!(
            parse_single_acceptance(&serde_json::json!("nope")),
            Err("acceptance has invalid level 'nope'.".to_string())
        );
        assert!(
            parse_single_acceptance(&serde_json::json!({ "bogus": 1 }))
                .expect_err("an unsupported key is rejected")
                .contains("acceptance.bogus is not supported.")
        );
    }

    /// SUBA-N04: a `tasks[]`/`chain[]` item's `acceptance` reaches its [`SingleStepSpec`] WHOLE.
    ///
    /// Pre-fix `parse_tool_acceptance` returned `Option<String>` and kept only the bare-level form,
    /// so `{ level: "verified", verify: [{ command }] }` and the `false` shorthand — the only forms
    /// that can declare a `verify[]` command at all — were discarded at the tool boundary, before
    /// the runner got its own chance to drop the survivor. Both losses were silent.
    #[test]
    fn a_tasks_item_carries_every_acceptance_form_onto_its_step_spec() {
        let item: ToolTaskItem = serde_json::from_value(serde_json::json!({
            "agent": "builder",
            "task": "fix it",
            "acceptance": { "level": "verified", "verify": [{ "command": "cargo test" }] }
        }))
        .expect("a well-formed tasks[] item");
        assert_eq!(
            tool_task_to_spec(&item).acceptance,
            Some(serde_json::json!({
                "level": "verified",
                "verify": [{ "command": "cargo test" }]
            })),
            "the object policy must survive whole — it is the only form carrying verify[]"
        );

        // The `false` shorthand (pi's `level: "none"`) and the bare level string both survive too.
        assert_eq!(
            parse_tool_acceptance(Some(&serde_json::json!(false))),
            Some(serde_json::json!(false))
        );
        assert_eq!(
            parse_tool_acceptance(Some(&serde_json::json!("checked"))),
            Some(serde_json::json!("checked"))
        );
        // Only "no policy at all" normalizes to `None` (pi's `undefined`).
        assert_eq!(parse_tool_acceptance(None), None);
        assert_eq!(parse_tool_acceptance(Some(&serde_json::Value::Null)), None);
        assert_eq!(parse_tool_acceptance(Some(&serde_json::json!(""))), None);
    }

    /// SUBA-N03 shape, third path: a `tasks[]` / `chain[].parallel[]` item's advertised `skill`
    /// reaches its [`SingleStepSpec`], in every one of pi's three states.
    ///
    /// pi advertises `skill` on `ParallelTaskSchema` (`extension/schemas.ts:146` @v0.43.0) and
    /// honours it — `params.tasks.map((task) => normalizeSkillInput(task.skill))` feeds the
    /// per-task step override at `runs/foreground/subagent-executor.ts:2399,2404` and again at
    /// `:3169-3170,:3177`. `tool_task_to_spec` hardcoded `skills: None`, so the field was
    /// deserialized, counted by `provided_keys`, and then dropped: the child ran with its persona's
    /// own `skills:` regardless, and `skill: false` could not switch them off.
    ///
    /// The tri-state is the whole point, so all three states are asserted — and PRESENCE first:
    /// a fix that mapped every input to `Some(vec![])` would silence a persona's skills everywhere
    /// and still pass a `skill: false`-only test.
    #[test]
    fn a_tasks_item_carries_its_skill_override_onto_its_step_spec() {
        let spec_for = |skill: serde_json::Value| {
            let mut obj = serde_json::json!({ "agent": "builder", "task": "fix it" });
            obj.as_object_mut()
                .expect("object")
                .insert("skill".to_string(), skill);
            let item: ToolTaskItem =
                serde_json::from_value(obj).expect("a well-formed tasks[] item");
            tool_task_to_spec(&item).skills
        };

        // PRESENCE: an explicit list replaces the persona's own `skills:` (pi `Some(names)`).
        assert_eq!(
            spec_for(serde_json::json!(["rust", "testing"])),
            Some(vec!["rust".to_string(), "testing".to_string()]),
            "an explicit per-task skill list must reach the step spec"
        );
        // pi's comma-string form normalizes identically on this path, via `normalize_skill_input`.
        assert_eq!(
            spec_for(serde_json::json!("rust, testing")),
            Some(vec!["rust".to_string(), "testing".to_string()])
        );
        // ABSENCE: `skill: false` is pi's explicit "no skills at all" — an EMPTY list, which is
        // what `opts.skills ?? agent.skills` needs to see to suppress the persona's own list.
        assert_eq!(
            spec_for(serde_json::json!(false)),
            Some(Vec::new()),
            "`skill: false` must be the explicit empty list, not `None`"
        );
        // Omitted stays `None` — "no override, inherit the persona's `skills:`". Distinct from the
        // empty list above; collapsing the two would break the fallthrough in the other direction.
        let omitted: ToolTaskItem =
            serde_json::from_value(serde_json::json!({ "agent": "builder", "task": "fix it" }))
                .expect("a well-formed tasks[] item");
        assert_eq!(tool_task_to_spec(&omitted).skills, None);
    }

    /// pi `chainDir: params.chainDir ?? getProjectChainRunsDir(effectiveCwd)`
    /// (`subagent-executor.ts:2623` @v0.43.0), which lives in `runChainPath`.
    ///
    /// Regression: `chainDir` was advertised with pi's description copied verbatim
    /// (`schemas.ts:263`), deserialized into `SubagentToolParams::chain_dir`, counted by
    /// `provided_keys()` — and then dropped, because `run_or_background_graph` had no parameter to
    /// carry it and built its own path unconditionally. A caller setting `chainDir` got silence.
    #[test]
    fn an_explicit_chain_dir_wins_over_the_default_scratch_dir() {
        let cwd = std::path::Path::new("/tmp/cyrup-chain-dir-parity");
        let run = RunId::new();

        use crate::artifacts::ArtifactDirPreference;

        let explicit = PathBuf::from("/somewhere/the/caller/picked");
        assert_eq!(
            resolve_chain_dir(
                Some(explicit.clone()),
                cwd,
                &run,
                ArtifactDirPreference::Project
            ),
            explicit,
            "an explicit chainDir must be used EXACTLY as given — pi does not rewrite it either"
        );

        // A RELATIVE value also stays verbatim: upstream joins a step's relative `output` against
        // this dir (`chain-execution.ts:283`); it never normalizes the dir itself.
        let relative = PathBuf::from("artifacts/chain");
        assert_eq!(
            resolve_chain_dir(
                Some(relative.clone()),
                cwd,
                &run,
                ArtifactDirPreference::Temp
            ),
            relative
        );

        // SUBA-048 / PARITY-GAPS PB-13: the DEFAULT (`project`) fallback root is the PROJECT
        // chain-runs dir, pi's `getChainRunsDir(cwd, "project") -> getProjectChainRunsDir(cwd)`
        // (`shared/artifacts.ts:145-158` @v0.43.0). RED before the fix: `resolve_chain_dir` took no
        // preference and always used the temp root, so this assertion could not even be written.
        let fallback = resolve_chain_dir(None, cwd, &run, ArtifactDirPreference::Project);
        assert_eq!(
            fallback,
            crate::artifacts::project_chain_runs_dir(cwd).join(run.as_str()),
            "the default preference must put chain runs inside the project, as upstream does"
        );
        assert_ne!(fallback, explicit);

        // `session` and `temp` both collapse onto the scoped temp root — upstream's own two-case
        // arm (`case "session": case "temp": return CHAIN_RUNS_DIR;`), which deliberately does NOT
        // mirror `getArtifactsDir`'s session-sibling branch.
        for pref in [ArtifactDirPreference::Session, ArtifactDirPreference::Temp] {
            assert_eq!(
                resolve_chain_dir(None, cwd, &run, pref),
                crate::artifacts::chain_runs_dir(cwd).join(run.as_str()),
                "{pref:?} must fall back to the temp chain-runs root"
            );
        }
    }
}
