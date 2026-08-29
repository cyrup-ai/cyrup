//! Mission launch binding and settle-time reconciliation — a 1:1 port of
//! `pi-subagents/src/missions/lifecycle.ts` (346 lines @v0.43.0).
//!
//! This is the module that connects a mission to an actual subagent run. Four entry points:
//!
//! | function | upstream | when it runs |
//! |---|---|---|
//! | [`prepare_mission_launch`] | `lifecycle.ts:55-87` | BEFORE the run starts |
//! | [`attach_mission_to_launch_result`] | `lifecycle.ts:167-256` | after the run settles |
//! | [`read_mission_binding`] | `lifecycle.ts:280-284` | when inspecting a background run |
//! | [`sync_mission_from_async_completion`] | `lifecycle.ts:286-346` | when a background run completes, possibly in a later process |
//!
//! # The binding file
//!
//! A background run gets [`MISSION_BINDING_FILE`] (`mission.json`) written into its async dir, so
//! the completion path — which may run in a different process, days later, with no access to the
//! launching tool call's state — can find the mission again. That file is the whole reason
//! [`MissionStoreLocation`] is serializable.
//!
//! # [CYRUP-DELTA] `AgentToolResult<Details>` → [`LaunchOutcome`]
//!
//! Upstream operates on an `AgentToolResult<Details>`: `{ content, isError?, details }` where
//! `Details` is the strongly-typed object of `pi-subagents/src/shared/types.ts:950-1010` (**not**
//! `src/missions/types.ts` — see [`super`]'s note on that ambiguity). cyrup has no `Details`
//! struct: `cyrup_core::ToolResult::details` is an opaque `Option<serde_json::Value>`, and for a
//! SINGLE run it is the serialized `SubagentUpdatePayload` (`extension.rs::route_single`), whose
//! JSON keys — `mode`, `runId`, `results[]`, each result's `exitCode`/`usage`/`agent`/
//! `savedOutputPath` — are the same camelCase names upstream reads off `Details`. This module
//! therefore reads `details` as JSON, by those exact key paths, and never depends on a Rust type
//! for it.
//!
//! The second half of that delta is `isError`. cyrup's `ToolResult` carries no such flag — a
//! failed tool call is an `Err(ToolError)` — so [`LaunchOutcome`] carries an explicit `is_error`
//! that the call site sets when converting from `Result<ToolResult, ToolError>`. Every upstream
//! `toolResultIsError(result)` test reads that field.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::actions::validate_mission_launch;
use super::store::{
    create_mission, mission_record_path, read_mission, resolve_mission_store_location,
    update_mission, validate_mission_id_str,
};
use super::{
    MissionArtifact, MissionArtifactKind, MissionCreateInput, MissionError, MissionRecord,
    MissionResult, MissionRunLink, MissionRunMode, MissionStatus, MissionStoreConfig,
    MissionStoreLocation, MissionTokenUsage, MissionUpdateInput, MISSION_SCHEMA_VERSION,
};

/// pi `MISSION_BINDING_FILE` (`lifecycle.ts:10`).
pub const MISSION_BINDING_FILE: &str = "mission.json";

/// The upper bound `firstText` applies to a summary (`lifecycle.ts:148`) and
/// `syncMissionFromAsyncCompletion` applies to an event summary (`lifecycle.ts:332`).
const MAX_SUMMARY_CHARS: usize = 2000;

/// pi `MissionLaunchParams` (`lifecycle.ts:12-18`) — the subset of the `subagent` tool's execution
/// parameters [`prepare_mission_launch`] reads.
#[derive(Clone, Debug, Default)]
pub struct MissionLaunchParams {
    /// `missionId` — attach to an EXISTING mission.
    pub mission_id: Option<String>,
    /// `mission` — the raw parameter. `Some(Value::Bool(false))` is upstream's explicit
    /// "no mission for this run" opt-out (`lifecycle.ts:63`); `None` is "not supplied".
    pub mission: Option<Value>,
    /// SINGLE mode's `task`.
    pub task: Option<String>,
    /// PARALLEL mode's `tasks[]`, as raw JSON — only each item's `task` field is read.
    pub tasks: Option<Vec<Value>>,
    /// CHAIN mode's `chain[]`, as raw JSON — each step's `task`, and each step's `parallel`
    /// children's `task`, are read.
    pub chain: Option<Vec<Value>>,
}

/// pi `MissionLaunchBinding` (`lifecycle.ts:20-25`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionLaunchBinding {
    /// The bound mission.
    pub mission_id: String,
    /// Where its record lives.
    pub location: MissionStoreLocation,
    /// `true` when this call CREATED the mission rather than attaching to an existing one.
    pub auto_created: bool,
    /// `true` when the settled result's last text part should gain a `Mission: <id> (<status>)`
    /// line. Upstream sets it on both branches of `prepareMissionLaunch` and leaves it absent on a
    /// binding read back from disk (`parsePersistedBinding`), which is why it is not a bare
    /// `bool` upstream either.
    pub announce_in_content: bool,
}

/// pi `workflowObjective` (`lifecycle.ts:37-48`): the first non-blank task string found, searching
/// `task`, then `tasks[]`, then `chain[]`, then — only if all three missed — each chain step's
/// `parallel` children.
fn workflow_objective(params: &MissionLaunchParams) -> Option<String> {
    let non_blank = |value: Option<&str>| -> Option<String> {
        value.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
    };
    if let Some(task) = non_blank(params.task.as_deref()) {
        return Some(task);
    }
    if let Some(tasks) = &params.tasks
        && let Some(task) =
            tasks.iter().find_map(|item| non_blank(item.get("task").and_then(Value::as_str)))
    {
        return Some(task);
    }
    if let Some(chain) = &params.chain {
        if let Some(task) =
            chain.iter().find_map(|step| non_blank(step.get("task").and_then(Value::as_str)))
        {
            return Some(task);
        }
        for step in chain {
            let parallel: Vec<&Value> = match step.get("parallel") {
                Some(Value::Array(items)) => items.iter().collect(),
                Some(value @ Value::Object(_)) => vec![value],
                _ => Vec::new(),
            };
            if let Some(task) = parallel
                .into_iter()
                .find_map(|child| non_blank(child.get("task").and_then(Value::as_str)))
            {
                return Some(task);
            }
        }
    }
    None
}

/// pi `conciseTitle` (`lifecycle.ts:50-53`): the first line, or the whole trimmed objective when
/// that first line is blank, ellipsized at 100 characters.
fn concise_title(objective: &str) -> String {
    let first_line = objective.split(['\r', '\n']).next().unwrap_or("").trim();
    let base = if first_line.is_empty() { objective.trim() } else { first_line };
    if base.chars().count() > 100 {
        let head: String = base.chars().take(97).collect();
        format!("{head}...")
    } else {
        base.to_string()
    }
}

/// pi `prepareMissionLaunch` (`lifecycle.ts:55-87`).
///
/// Returns `Ok(None)` — "this run has no mission" — in exactly three cases: `mission: false`, or
/// no `missionId` AND nothing that would trigger a create (`mission` absent AND either missions
/// disabled or no objective found).
///
/// # Errors
///
/// [`MissionError::Invalid`] when both `missionId` and `mission` are supplied, when the `mission`
/// object fails [`validate_mission_launch`], or when the id is malformed;
/// [`MissionError::NotFound`] when an explicit `missionId` does not exist.
pub fn prepare_mission_launch(
    params: &MissionLaunchParams,
    project_root: &Path,
    config: Option<&MissionStoreConfig>,
    owner_session_id: Option<&str>,
) -> MissionResult<Option<MissionLaunchBinding>> {
    let has_mission_id = params.mission_id.is_some();
    if has_mission_id && params.mission.is_some() {
        return Err(MissionError::invalid("Use missionId or mission, not both"));
    }
    if params.mission == Some(Value::Bool(false)) {
        return Ok(None);
    }
    let objective = workflow_objective(params);
    // pi: `input.config?.enabled !== false` — an ABSENT flag means enabled.
    let missions_enabled = config.and_then(|c| c.enabled) != Some(false);
    let should_create =
        params.mission.is_some() || (missions_enabled && objective.is_some());
    if !has_mission_id && !should_create {
        return Ok(None);
    }
    let location = resolve_mission_store_location(project_root, config, None);
    if let Some(raw_id) = params.mission_id.as_deref() {
        let mission_id = validate_mission_id_str(raw_id, "missionId")?;
        // The read is what turns a bogus id into a MissionNotFoundError before anything is
        // mutated; the update is what marks an attached-to mission ACTIVE.
        read_mission(&location, &mission_id)?;
        update_mission(
            &location,
            &mission_id,
            &MissionUpdateInput { status: Some(MissionStatus::Active), ..Default::default() },
            crate::time::now_epoch_millis(),
            None,
        )?;
        return Ok(Some(MissionLaunchBinding {
            mission_id,
            location,
            auto_created: false,
            announce_in_content: true,
        }));
    }
    let mission = match &params.mission {
        None => None,
        Some(value) => Some(validate_mission_launch(value)?),
    };
    // `mission?.title || conciseTitle(objective!)`: upstream asserts `objective` is present here
    // via `!`, and it is — `shouldCreate` was true and `hasMissionId` false, so either `mission`
    // was supplied (giving a title) or an objective was found.
    let title = match mission.as_ref().map(|m| m.title.clone()).filter(|t| !t.is_empty()) {
        Some(title) => title,
        None => concise_title(objective.as_deref().unwrap_or_default()),
    };
    let record = create_mission(
        &location,
        &MissionCreateInput {
            title: title.clone(),
            // `mission?.objective || objective || title`
            objective: mission
                .as_ref()
                .and_then(|m| m.objective.clone())
                .filter(|o| !o.is_empty())
                .or_else(|| objective.clone().filter(|o| !o.is_empty()))
                .unwrap_or(title),
            goal: mission.as_ref().and_then(|m| m.goal.then_some(true)),
            budget: mission.as_ref().and_then(|m| m.budget),
            status: Some(MissionStatus::Active),
            labels: mission.as_ref().and_then(|m| m.labels.clone()),
            owner_session_id: owner_session_id.map(str::to_string),
        },
        crate::time::now_epoch_millis(),
        config.and_then(|c| c.retain_terminal),
    )?;
    Ok(Some(MissionLaunchBinding {
        mission_id: record.id,
        location,
        auto_created: true,
        announce_in_content: true,
    }))
}

// =================================================================================================
// Settle-time reconciliation (lifecycle.ts:89-256)
// =================================================================================================

/// The launch result [`attach_mission_to_launch_result`] folds a mission onto — cyrup's stand-in
/// for upstream's `AgentToolResult<Details>` (see the module's `[CYRUP-DELTA]` note).
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchOutcome {
    /// The tool result's content parts.
    pub content: Vec<cyrup_core::Content>,
    /// The tool result's `details` object, read by JSON key path.
    pub details: Option<Value>,
    /// Whether this outcome is an ERROR result (upstream's `isError === true`).
    pub is_error: bool,
}

/// `result.details.<key>` as a `&str`.
fn detail_str<'a>(outcome: &'a LaunchOutcome, key: &str) -> Option<&'a str> {
    outcome.details.as_ref()?.get(key)?.as_str()
}

/// `result.details.results` — the per-child array, empty when absent.
fn detail_results(outcome: &LaunchOutcome) -> &[Value] {
    outcome
        .details
        .as_ref()
        .and_then(|d| d.get("results"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// pi `missionRunModeForResult` (`lifecycle.ts:93-95`): `"management"` becomes `external`; any
/// other `Details["mode"]` maps through by name. An unrecognized mode also lands on `external`,
/// which is the only total mapping available for an open string.
fn mission_run_mode_for_result(mode: Option<&str>) -> MissionRunMode {
    match mode {
        Some("management") | None => MissionRunMode::External,
        Some(other) => MissionRunMode::from_wire(other).unwrap_or(MissionRunMode::External),
    }
}

/// pi `runStatusForResult` (`lifecycle.ts:97-102`), in its own order — an async run is `active`
/// regardless of anything else, then paused, then failed, then completed.
fn run_status_for_result(outcome: &LaunchOutcome) -> &'static str {
    if detail_str(outcome, "asyncDir").is_some() {
        return "active";
    }
    let results = detail_results(outcome);
    if results.iter().any(|child| {
        child.get("interrupted") == Some(&Value::Bool(true))
            || child.get("detached") == Some(&Value::Bool(true))
    }) {
        return "paused";
    }
    if outcome.is_error
        || results
            .iter()
            .any(|child| child.get("exitCode").and_then(Value::as_i64).unwrap_or(0) != 0)
    {
        return "failed";
    }
    "completed"
}

/// pi `missionStatusForRun` (`lifecycle.ts:104-114`) — the mission status one run's status
/// implies, in upstream's exact branch order. Note branch three: a GOAL mission stays `active`
/// whatever the run did, so the turn-end driver keeps considering it.
fn mission_status_for_run(
    record: &MissionRecord,
    run_id: &str,
    run_status: &str,
) -> MissionStatus {
    if record.status.is_terminal() {
        return record.status;
    }
    if matches!(run_status, "active" | "queued" | "running") {
        return MissionStatus::Active;
    }
    if record.goal.is_some() {
        return MissionStatus::Active;
    }
    if run_status == "paused" {
        return MissionStatus::Waiting;
    }
    let other_active = record.runs.iter().any(|run| {
        run.run_id != run_id
            && run.status.as_deref().is_some_and(|s| matches!(s, "active" | "queued" | "running"))
    });
    if other_active {
        return MissionStatus::Active;
    }
    match run_status {
        "completed" | "complete" => MissionStatus::Completed,
        "stopped" | "rejected" | "cancelled" => MissionStatus::Cancelled,
        _ => MissionStatus::Failed,
    }
}

/// pi `usageForResult` (`lifecycle.ts:116-119`): summed `usage.input + usage.output` across every
/// child, `None` when the total is zero.
fn usage_for_result(outcome: &LaunchOutcome) -> Option<MissionTokenUsage> {
    let tokens: u64 = detail_results(outcome)
        .iter()
        .map(|child| {
            let usage = child.get("usage");
            let read = |key: &str| {
                usage.and_then(|u| u.get(key)).and_then(Value::as_u64).unwrap_or(0)
            };
            read("input") + read("output")
        })
        .sum();
    (tokens > 0).then_some(MissionTokenUsage { tokens })
}

/// pi `usageFromUnknown` (`lifecycle.ts:121-125`): `{ total: <non-negative safe integer> }`.
fn usage_from_unknown(value: Option<&Value>) -> Option<MissionTokenUsage> {
    let total = value?.as_object()?.get("total")?.as_i64()?;
    // `Number.isSafeInteger(total) && total >= 0` — the upper bound is JS's 2^53-1.
    (0..=9_007_199_254_740_991)
        .contains(&total)
        .then_some(MissionTokenUsage { tokens: total.unsigned_abs() })
}

/// pi `artifactsForResult` (`lifecycle.ts:127-143`) — everything a settled run points at.
fn artifacts_for_result(outcome: &LaunchOutcome) -> Vec<MissionArtifact> {
    let mut artifacts = Vec::new();
    if let Some(async_dir) = detail_str(outcome, "asyncDir") {
        let dir = Path::new(async_dir);
        artifacts.push(MissionArtifact {
            kind: MissionArtifactKind::Status,
            path: dir.join("status.json").to_string_lossy().into_owned(),
            description: None,
        });
        artifacts.push(MissionArtifact {
            kind: MissionArtifactKind::Other,
            path: dir.join("events.jsonl").to_string_lossy().into_owned(),
            description: Some("Lifecycle events".to_string()),
        });
    }
    for child in detail_results(outcome) {
        let string_at = |path: &[&str]| -> Option<String> {
            let mut cursor = child;
            for key in path {
                cursor = cursor.get(key)?;
            }
            cursor.as_str().map(str::to_string)
        };
        if let Some(path) = string_at(&["artifactPaths", "outputPath"]) {
            artifacts.push(MissionArtifact {
                kind: MissionArtifactKind::Output,
                path,
                description: None,
            });
        }
        if let Some(path) = string_at(&["savedOutputPath"]) {
            artifacts.push(MissionArtifact {
                kind: MissionArtifactKind::Output,
                path,
                description: None,
            });
        }
        if let Some(path) = string_at(&["transcriptPath"]) {
            artifacts.push(MissionArtifact {
                kind: MissionArtifactKind::Other,
                path,
                description: Some("Child transcript".to_string()),
            });
        }
        if let Some(path) = string_at(&["structuredOutputPath"]) {
            artifacts.push(MissionArtifact {
                kind: MissionArtifactKind::Output,
                path,
                description: Some("Structured output".to_string()),
            });
        }
    }
    if let Some(path) = outcome
        .details
        .as_ref()
        .and_then(|d| d.get("parallelHandoff"))
        .and_then(|h| h.get("path"))
        .and_then(Value::as_str)
    {
        artifacts.push(MissionArtifact {
            kind: MissionArtifactKind::Manifest,
            path: path.to_string(),
            description: None,
        });
    }
    artifacts
}

/// The text of a [`cyrup_core::Content`] part, when it is a text part.
fn content_text(part: &cyrup_core::Content) -> Option<&str> {
    match part {
        cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
        _ => None,
    }
}

/// pi `firstText` (`lifecycle.ts:145-149`): the FIRST text part, trimmed, `None` when blank,
/// ellipsized at 2000 characters.
fn first_text(outcome: &LaunchOutcome) -> Option<String> {
    let text = outcome.content.iter().find_map(content_text)?.trim();
    if text.is_empty() {
        return None;
    }
    if text.chars().count() > MAX_SUMMARY_CHARS {
        let head: String = text.chars().take(MAX_SUMMARY_CHARS - 3).collect();
        Some(format!("{head}..."))
    } else {
        Some(text.to_string())
    }
}

/// pi's `PersistedMissionBinding` (`lifecycle.ts:27-35`) — the on-disk `mission.json` shape.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedMissionBinding<'a> {
    schema_version: u8,
    mission_id: &'a str,
    project_root: String,
    mission_dir: String,
    global_index_dir: String,
    write_global_index: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    retain_terminal: Option<u64>,
}

/// pi `persistedBinding` (`lifecycle.ts:151-161`).
fn persisted_binding(binding: &MissionLaunchBinding) -> PersistedMissionBinding<'_> {
    PersistedMissionBinding {
        schema_version: MISSION_SCHEMA_VERSION,
        mission_id: &binding.mission_id,
        project_root: binding.location.project_root.to_string_lossy().into_owned(),
        mission_dir: binding.location.mission_dir.to_string_lossy().into_owned(),
        global_index_dir: binding.location.global_index_dir.to_string_lossy().into_owned(),
        write_global_index: binding.location.write_global_index,
        retain_terminal: binding.location.retain_terminal,
    }
}

/// pi `writeAsyncBinding` (`lifecycle.ts:163-165`).
fn write_async_binding(async_dir: &Path, binding: &MissionLaunchBinding) -> MissionResult<()> {
    super::write_private_atomic_json(
        &async_dir.join(MISSION_BINDING_FILE),
        &persisted_binding(binding),
    )
}

/// pi `attachMissionToLaunchResult` (`lifecycle.ts:167-256`) — fold a settled run onto its
/// mission and stamp the mission back onto the result.
///
/// Two shapes:
///
/// * **No run id** (a management/refused result). The mission is only touched when the outcome is
///   an ERROR, and even then it goes to `failed` only if no other run is still live. No run link,
///   no artifacts, no content announcement.
/// * **A run id.** A run link is upserted with the derived status/usage/agent, the run's artifacts
///   are recorded, the summary is taken from the first text part when the run is not still active,
///   a single-child acceptance ledger is carried across, and — for a background run — the binding
///   file is written and any ALREADY-terminal `status.json` is reconciled immediately.
///
/// # Errors
///
/// [`MissionError::NotFound`] when the bound mission has disappeared, [`MissionError::Invalid`]
/// for a validation failure or an unreadable terminal `status.json`, [`MissionError::Io`] for a
/// persistence failure. Upstream's caller catches all of these and degrades to a
/// `missionWarning` on the result rather than failing the run.
pub fn attach_mission_to_launch_result(
    binding: &MissionLaunchBinding,
    outcome: LaunchOutcome,
) -> MissionResult<LaunchOutcome> {
    let run_id = detail_str(&outcome, "runId")
        .or_else(|| detail_str(&outcome, "asyncId"))
        .map(str::to_string);
    let mission_path = mission_record_path(&binding.location, &binding.mission_id)?;

    let Some(run_id) = run_id else {
        let current = read_mission(&binding.location, &binding.mission_id)?;
        let active_run_exists = current.runs.iter().any(|run| {
            run.status.as_deref().is_some_and(|s| matches!(s, "active" | "queued" | "running"))
        });
        let mission = if outcome.is_error {
            update_mission(
                &binding.location,
                &binding.mission_id,
                &MissionUpdateInput {
                    status: Some(if active_run_exists {
                        MissionStatus::Active
                    } else {
                        MissionStatus::Failed
                    }),
                    summary: first_text(&outcome),
                    ..Default::default()
                },
                crate::time::now_epoch_millis(),
                None,
            )?
        } else {
            current
        };
        return Ok(LaunchOutcome {
            details: Some(stamp_mission(outcome.details, &mission, &mission_path)),
            ..outcome
        });
    };

    let run_status = run_status_for_result(&outcome);
    let current = read_mission(&binding.location, &binding.mission_id)?;
    let started_at = super::format_iso8601_millis(crate::time::now_epoch_millis());
    let usage = usage_for_result(&outcome);
    let results = detail_results(&outcome);
    let single_child = (results.len() == 1).then(|| results.first()).flatten();
    let run = MissionRunLink {
        run_id: run_id.clone(),
        mode: mission_run_mode_for_result(detail_str(&outcome, "mode")),
        status: Some(run_status.to_string()),
        started_at: Some(started_at.clone()),
        async_dir: detail_str(&outcome, "asyncDir").map(str::to_string),
        child_index: None,
        agent: single_child
            .and_then(|child| child.get("agent"))
            .and_then(Value::as_str)
            .map(str::to_string),
        completed_at: (run_status != "active").then(|| started_at.clone()),
        usage,
    };
    let summary_text = first_text(&outcome);
    let mut mission = update_mission(
        &binding.location,
        &binding.mission_id,
        &MissionUpdateInput {
            status: Some(mission_status_for_run(&current, &run_id, run_status)),
            add_runs: vec![run],
            add_artifacts: artifacts_for_result(&outcome),
            summary: if run_status == "active" { None } else { summary_text.clone() },
            // pi guards this on TRUTHINESS (`results[0]?.acceptance ? … : {}`, `lifecycle.ts:210`),
            // so a `null`/`false` acceptance is NOT carried across — only a real ledger is.
            acceptance: single_child
                .and_then(|child| child.get("acceptance"))
                .filter(|v| !matches!(v, Value::Null | Value::Bool(false)))
                .cloned(),
            ..Default::default()
        },
        crate::time::now_epoch_millis(),
        None,
    )?;

    if let Some(async_dir) = detail_str(&outcome, "asyncDir") {
        let async_dir = Path::new(async_dir);
        write_async_binding(async_dir, binding)?;
        let status_path = async_dir.join("status.json");
        if status_path.exists() {
            let terminal_state = std::fs::read_to_string(&status_path)
                .map_err(|e| e.to_string())
                .and_then(|raw| {
                    serde_json::from_str::<Value>(&raw).map_err(|e| e.to_string())
                })
                .map_err(|e| {
                    MissionError::invalid(format!(
                        "Failed to reconcile mission from terminal async status '{}': {e}",
                        status_path.display()
                    ))
                })?
                .get("state")
                .and_then(Value::as_str)
                .filter(|state| !matches!(*state, "queued" | "running"))
                .map(str::to_string);
            if let Some(state) = terminal_state {
                let mut event = serde_json::Map::new();
                event.insert("runId".to_string(), Value::String(run_id.clone()));
                event.insert(
                    "asyncDir".to_string(),
                    Value::String(async_dir.to_string_lossy().into_owned()),
                );
                if let Some(mode) = detail_str(&outcome, "mode") {
                    event.insert("mode".to_string(), Value::String(mode.to_string()));
                }
                event.insert("state".to_string(), Value::String(state));
                if let Some(summary) = &summary_text {
                    event.insert("summary".to_string(), Value::String(summary.clone()));
                }
                // Upstream's `try` wraps the sync call as well as the read/parse
                // (`lifecycle.ts:216-229`), so a failure INSIDE the reconciliation also surfaces
                // under the `Failed to reconcile mission from terminal async status '<path>'`
                // wrapper rather than bare.
                let synced = sync_mission_from_async_completion(&Value::Object(event))
                    .map_err(|e| {
                        MissionError::invalid(format!(
                            "Failed to reconcile mission from terminal async status '{}': {e}",
                            status_path.display()
                        ))
                    })?;
                if let Some(synced) = synced {
                    mission = synced;
                }
            }
        }
    }

    // pi's content announcement (`lifecycle.ts:232-248`): append `Mission: <id> (<status>)` to the
    // LAST text part, but only when announcing is on, there IS a text part, no child produced
    // structured output, and that last text is not itself JSON (appending prose to a JSON payload
    // would corrupt a machine-readable result).
    let last_text_index = outcome
        .content
        .iter()
        .enumerate()
        .filter(|(_, part)| content_text(part).is_some())
        .map(|(index, _)| index)
        .next_back();
    // [CYRUP-DELTA] upstream keys this guard on `child.structuredOutputPath !== undefined`
    // (`lifecycle.ts:233`), the FILE its structured-output runtime persisted the child's value to
    // (`shared/types.ts:914`, written beside `structuredOutput` at `:912`). cyrup's own
    // [`crate::exec::SingleResult`] carries the VALUE and nothing else — `structured_output:
    // Option<serde_json::Value>` (`exec/mod.rs:759`), serialized as `structuredOutput`; there is no
    // `structuredOutputPath` field anywhere in this crate. Reading only upstream's key therefore
    // made this guard permanently `false` on every result cyrup itself produces, and the
    // `Mission: <id> (<status>)` announcement was appended to structured-output runs that upstream
    // deliberately leaves untouched. Both keys are accepted: the value key is the one cyrup's
    // producer writes, the path key keeps an upstream-shaped `details` working unchanged.
    let has_structured_output = detail_results(&outcome).iter().any(|child| {
        ["structuredOutputPath", "structuredOutput"]
            .iter()
            .any(|key| child.get(key).is_some_and(|value| !value.is_null()))
    });
    let text_is_json = last_text_index
        .and_then(|index| outcome.content.get(index))
        .and_then(content_text)
        .is_some_and(|text| serde_json::from_str::<Value>(text).is_ok());

    let content = match last_text_index {
        Some(index)
            if binding.announce_in_content && !has_structured_output && !text_is_json =>
        {
            outcome
                .content
                .iter()
                .enumerate()
                .map(|(i, part)| match (i == index, content_text(part)) {
                    (true, Some(text)) => cyrup_core::Content::text(format!(
                        "{text}\nMission: {} ({})",
                        mission.id,
                        mission.status.as_str()
                    )),
                    _ => part.clone(),
                })
                .collect()
        }
        _ => outcome.content.clone(),
    };

    Ok(LaunchOutcome {
        content,
        details: Some(stamp_mission(outcome.details, &mission, &mission_path)),
        is_error: outcome.is_error,
    })
}

/// pi's `details: { ...result.details, missionId, missionPath, mission }` spread — applied to a
/// possibly-absent `details`, in which case a fresh object is created (a `null` details with a
/// bound mission would otherwise silently drop the binding).
fn stamp_mission(details: Option<Value>, mission: &MissionRecord, mission_path: &Path) -> Value {
    let mut object = match details {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    object.insert("missionId".to_string(), Value::String(mission.id.clone()));
    object.insert(
        "missionPath".to_string(),
        Value::String(mission_path.to_string_lossy().into_owned()),
    );
    object.insert(
        "mission".to_string(),
        serde_json::to_value(mission).unwrap_or(Value::Null),
    );
    Value::Object(object)
}

/// pi `parsePersistedBinding` (`lifecycle.ts:258-278`). Note `autoCreated` is hardcoded `false`
/// and `announceInContent` is left absent — a binding read back from disk never re-announces.
fn parse_persisted_binding(value: &Value, source: &str) -> MissionResult<MissionLaunchBinding> {
    let input = value
        .as_object()
        .ok_or_else(|| MissionError::invalid(format!("{source} must be an object")))?;
    if input.get("schemaVersion").and_then(Value::as_u64) != Some(u64::from(MISSION_SCHEMA_VERSION))
    {
        return Err(MissionError::invalid(format!("{source}.schemaVersion must be 1")));
    }
    let mut fields = Vec::with_capacity(3);
    for field in ["projectRoot", "missionDir", "globalIndexDir"] {
        let value = input
            .get(field)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                MissionError::invalid(format!("{source}.{field} must be a non-empty string"))
            })?;
        fields.push(value);
    }
    let write_global_index = match input.get("writeGlobalIndex") {
        Some(Value::Bool(b)) => *b,
        _ => {
            return Err(MissionError::invalid(format!(
                "{source}.writeGlobalIndex must be boolean"
            )));
        }
    };
    let retain_terminal = match input.get("retainTerminal") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_i64() {
            Some(n) if n >= 1 => Some(n.unsigned_abs()),
            _ => {
                return Err(MissionError::invalid(format!(
                    "{source}.retainTerminal must be a positive integer"
                )));
            }
        },
    };
    let mission_id = match input.get("missionId").and_then(Value::as_str) {
        Some(id) => validate_mission_id_str(id, &format!("{source}.missionId"))?,
        None => {
            return Err(MissionError::invalid(format!(
                "{source}.missionId must be a non-empty string"
            )));
        }
    };
    Ok(MissionLaunchBinding {
        mission_id,
        auto_created: false,
        announce_in_content: false,
        location: MissionStoreLocation {
            project_root: PathBuf::from(fields.first().copied().unwrap_or_default()),
            mission_dir: PathBuf::from(fields.get(1).copied().unwrap_or_default()),
            global_index_dir: PathBuf::from(fields.get(2).copied().unwrap_or_default()),
            write_global_index,
            retain_terminal,
        },
    })
}

/// pi `readMissionBinding` (`lifecycle.ts:280-284`): the `mission.json` a background run's async
/// dir carries, or `None` when the run has no mission.
///
/// # Errors
///
/// [`MissionError::Io`] when the file exists but cannot be read, [`MissionError::Invalid`] when
/// it does not parse or does not validate.
pub fn read_mission_binding(async_dir: &Path) -> MissionResult<Option<MissionLaunchBinding>> {
    let binding_path = async_dir.join(MISSION_BINDING_FILE);
    let raw = match std::fs::read_to_string(&binding_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(MissionError::io(&binding_path, err)),
    };
    let value: Value = serde_json::from_str(&raw).map_err(|e| {
        MissionError::invalid(format!("{}: {e}", binding_path.to_string_lossy()))
    })?;
    parse_persisted_binding(&value, &binding_path.to_string_lossy()).map(Some)
}

/// pi `syncMissionFromAsyncCompletion` (`lifecycle.ts:286-346`) — reconcile a mission from a
/// completed background run, possibly in a process that never launched it.
///
/// `Ok(None)` for every "nothing to do" case: a non-object event, an event with no `asyncDir`, an
/// async dir with no binding file, or a binding whose record has since been deleted (in which case
/// a `subagent.mission.sync.skipped` breadcrumb is appended to the run's `events.jsonl`, because
/// mission bookkeeping must never destroy a completed async result).
///
/// # Errors
///
/// [`MissionError::Invalid`] when the event carries no run id at all, or when the binding does not
/// validate; [`MissionError::Io`]/[`MissionError::NotFound`] as [`update_mission`] raises them.
pub fn sync_mission_from_async_completion(event: &Value) -> MissionResult<Option<MissionRecord>> {
    let Some(map) = event.as_object() else { return Ok(None) };
    let Some(async_dir) =
        map.get("asyncDir").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
    else {
        return Ok(None);
    };
    let async_dir_path = Path::new(async_dir);
    let Some(binding) = read_mission_binding(async_dir_path)? else { return Ok(None) };
    let run_id = map
        .get("runId")
        .and_then(Value::as_str)
        .or_else(|| map.get("id").and_then(Value::as_str))
        .ok_or_else(|| MissionError::invalid("Async mission completion is missing runId"))?
        .to_string();
    let run_status = match map.get("state").and_then(Value::as_str) {
        Some(state) => state.to_string(),
        None => {
            if map.get("success") == Some(&Value::Bool(true)) { "completed" } else { "failed" }
                .to_string()
        }
    };
    let current = match read_mission(&binding.location, &binding.mission_id) {
        Ok(record) => record,
        Err(err) if err.is_not_found() => {
            append_sync_skipped_breadcrumb(async_dir_path, &run_id, &binding);
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    let completed_at = super::format_iso8601_millis(crate::time::now_epoch_millis());
    let mut artifacts = vec![
        MissionArtifact {
            kind: MissionArtifactKind::Status,
            path: async_dir_path.join("status.json").to_string_lossy().into_owned(),
            description: None,
        },
        MissionArtifact {
            kind: MissionArtifactKind::Other,
            path: async_dir_path.join("events.jsonl").to_string_lossy().into_owned(),
            description: Some("Lifecycle events".to_string()),
        },
    ];
    if let Some(path) = map
        .get("parallelHandoff")
        .and_then(|h| h.as_object())
        .and_then(|h| h.get("path"))
        .and_then(Value::as_str)
    {
        artifacts.push(MissionArtifact {
            kind: MissionArtifactKind::Manifest,
            path: path.to_string(),
            description: None,
        });
    }
    let event_results = map.get("results").and_then(Value::as_array);
    if let Some(results) = event_results {
        for result in results {
            let Some(child) = result.as_object() else { continue };
            if let Some(path) = child.get("artifactPath").and_then(Value::as_str) {
                artifacts.push(MissionArtifact {
                    kind: MissionArtifactKind::Output,
                    path: path.to_string(),
                    description: None,
                });
            }
            if let Some(path) = child
                .get("artifactPaths")
                .and_then(|p| p.as_object())
                .and_then(|p| p.get("outputPath"))
                .and_then(Value::as_str)
            {
                artifacts.push(MissionArtifact {
                    kind: MissionArtifactKind::Output,
                    path: path.to_string(),
                    description: None,
                });
            }
        }
    }
    let summary = map
        .get("summary")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.chars().take(MAX_SUMMARY_CHARS).collect::<String>());
    let usage = usage_from_unknown(map.get("totalTokens")).or_else(|| {
        event_results.map(|results| MissionTokenUsage {
            tokens: results
                .iter()
                .map(|result| {
                    result
                        .as_object()
                        .and_then(|child| usage_from_unknown(child.get("tokens")))
                        .map_or(0, |u| u.tokens)
                })
                .sum(),
        })
    });
    // pi's `["single","parallel","chain"].includes(event.mode)` — a `workflow`/`scheduled` mode on
    // an async completion event is NOT trusted through, it lands on `external`.
    let mode = match map.get("mode").and_then(Value::as_str) {
        Some("single") => MissionRunMode::Single,
        Some("parallel") => MissionRunMode::Parallel,
        Some("chain") => MissionRunMode::Chain,
        _ => MissionRunMode::External,
    };
    let updated = update_mission(
        &binding.location,
        &binding.mission_id,
        &MissionUpdateInput {
            status: Some(mission_status_for_run(&current, &run_id, &run_status)),
            add_runs: vec![MissionRunLink {
                run_id,
                mode,
                async_dir: Some(async_dir.to_string()),
                child_index: None,
                agent: None,
                status: Some(run_status),
                started_at: None,
                completed_at: Some(completed_at),
                usage: usage.filter(|u| u.tokens > 0),
            }],
            add_artifacts: artifacts,
            summary,
            ..Default::default()
        },
        crate::time::now_epoch_millis(),
        None,
    )?;
    Ok(Some(updated))
}

/// pi's `subagent.mission.sync.skipped` breadcrumb (`lifecycle.ts:301-310`): a completed async run
/// whose mission record has been deleted appends one line to its own `events.jsonl` and moves on.
/// Entirely best-effort — "Mission bookkeeping is secondary to preserving the completed async
/// result."
fn append_sync_skipped_breadcrumb(
    async_dir: &Path,
    run_id: &str,
    binding: &MissionLaunchBinding,
) {
    use std::io::Write as _;
    let Ok(mission_path) = mission_record_path(&binding.location, &binding.mission_id) else {
        return;
    };
    let line = serde_json::json!({
        "type": "subagent.mission.sync.skipped",
        "ts": crate::time::now_epoch_millis(),
        "runId": run_id,
        "missionId": binding.mission_id,
        "reason": "mission-record-missing",
        "missionPath": mission_path.to_string_lossy(),
    });
    let Ok(mut file) =
        std::fs::OpenOptions::new().create(true).append(true).open(async_dir.join("events.jsonl"))
    else {
        return;
    };
    let _ = writeln!(file, "{line}");
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
    use crate::missions::store::{list_missions, read_mission};

    fn outcome(details: Value, text: &str) -> LaunchOutcome {
        LaunchOutcome {
            content: vec![cyrup_core::Content::text(text)],
            details: Some(details),
            is_error: false,
        }
    }

    /// The GLOBAL pointer index defaults to `agent_dir()/missions/index` — i.e. the developer's
    /// real `~/.cyrup/agent` (`store.rs::resolve_mission_store_location`, faithful to pi
    /// `store.ts:265`). [`prepare_mission_launch`] takes no agent-dir override (upstream's
    /// `prepareMissionLaunch` does not pass one either, `lifecycle.ts:68`), so a test scopes the
    /// index the only way production can: through `config.missions.globalIndexDir`. Every test in
    /// this module MUST launch through a config carrying it, or it writes into live user config.
    fn scoped(root: &Path) -> MissionStoreConfig {
        MissionStoreConfig {
            global_index_dir: Some(
                root.join("agent").join("missions").join("index").to_string_lossy().into_owned(),
            ),
            ..Default::default()
        }
    }

    fn prepared(root: &Path, params: MissionLaunchParams) -> Option<MissionLaunchBinding> {
        prepare_mission_launch(&params, root, Some(&scoped(root)), Some("sess-1")).unwrap()
    }

    #[test]
    fn a_task_bearing_launch_auto_creates_an_active_mission() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams {
                task: Some("  Refactor the parser\nand ship it  ".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(binding.auto_created);
        assert!(binding.announce_in_content);
        let record = read_mission(&binding.location, &binding.mission_id).unwrap();
        assert_eq!(record.status, MissionStatus::Active);
        // The title is the FIRST LINE; the objective is the whole (trimmed) task.
        assert_eq!(record.title, "Refactor the parser");
        assert_eq!(record.objective, "Refactor the parser\nand ship it");
        assert_eq!(record.owner_session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn mission_false_opts_out_entirely() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(prepared(
            tmp.path(),
            MissionLaunchParams {
                task: Some("do it".to_string()),
                mission: Some(Value::Bool(false)),
                ..Default::default()
            },
        )
        .is_none());
        assert!(list_missions(&resolve_mission_store_location(
            tmp.path(),
            Some(&scoped(tmp.path())),
            None
        ))
        .records
        .is_empty());
    }

    #[test]
    fn a_launch_with_no_objective_and_no_mission_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(prepared(tmp.path(), MissionLaunchParams::default()).is_none());
    }

    #[test]
    fn missions_disabled_suppresses_the_automatic_create_but_not_an_explicit_one() {
        let tmp = tempfile::tempdir().unwrap();
        let disabled = MissionStoreConfig { enabled: Some(false), ..scoped(tmp.path()) };
        let params = MissionLaunchParams { task: Some("do it".to_string()), ..Default::default() };
        assert!(
            prepare_mission_launch(&params, tmp.path(), Some(&disabled), None).unwrap().is_none()
        );
        let explicit = MissionLaunchParams {
            mission: Some(serde_json::json!({"title": "Explicit"})),
            ..params
        };
        assert!(
            prepare_mission_launch(&explicit, tmp.path(), Some(&disabled), None)
                .unwrap()
                .is_some(),
            "an explicit mission object is not gated on config.enabled"
        );
    }

    #[test]
    fn mission_id_and_mission_together_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let err = prepare_mission_launch(
            &MissionLaunchParams {
                mission_id: Some("m".to_string()),
                mission: Some(serde_json::json!({"title": "t"})),
                ..Default::default()
            },
            tmp.path(),
            Some(&scoped(tmp.path())),
            None,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "Use missionId or mission, not both");
    }

    #[test]
    fn an_explicit_mission_id_attaches_and_marks_it_active() {
        let tmp = tempfile::tempdir().unwrap();
        let created = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("first".to_string()), ..Default::default() },
        )
        .unwrap();
        let attached = prepared(
            tmp.path(),
            MissionLaunchParams {
                mission_id: Some(created.mission_id.clone()),
                task: Some("second".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!attached.auto_created);
        assert_eq!(attached.mission_id, created.mission_id);

        let missing = prepare_mission_launch(
            &MissionLaunchParams { mission_id: Some("nope".to_string()), ..Default::default() },
            tmp.path(),
            Some(&scoped(tmp.path())),
            None,
        )
        .unwrap_err();
        assert!(missing.is_not_found(), "{missing}");
    }

    #[test]
    fn a_chain_objective_falls_back_through_steps_and_parallel_children() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams {
                chain: Some(vec![
                    serde_json::json!({"agent": "a"}),
                    serde_json::json!({"parallel": [{"agent": "b"}, {"task": "nested work"}]}),
                ]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            read_mission(&binding.location, &binding.mission_id).unwrap().objective,
            "nested work"
        );
    }

    #[test]
    fn a_settled_foreground_run_records_a_link_usage_and_the_announcement() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("build".to_string()), ..Default::default() },
        )
        .unwrap();
        let attached = attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single",
                    "runId": "run-a",
                    "results": [{
                        "agent": "builder",
                        "exitCode": 0,
                        "usage": {"input": 10, "output": 15},
                        "savedOutputPath": "/tmp/out.md",
                    }],
                }),
                "all done",
            ),
        )
        .unwrap();

        let record = read_mission(&binding.location, &binding.mission_id).unwrap();
        assert_eq!(record.status, MissionStatus::Completed);
        assert_eq!(record.runs.len(), 1);
        assert_eq!(record.runs[0].run_id, "run-a");
        assert_eq!(record.runs[0].mode, MissionRunMode::Single);
        assert_eq!(record.runs[0].status.as_deref(), Some("completed"));
        assert_eq!(record.runs[0].agent.as_deref(), Some("builder"));
        assert_eq!(record.runs[0].usage.unwrap().tokens, 25);
        assert!(record.runs[0].completed_at.is_some());
        assert_eq!(record.summary.as_deref(), Some("all done"));
        assert_eq!(record.artifacts.len(), 1);
        assert_eq!(record.artifacts[0].path, "/tmp/out.md");

        let text = match &attached.content[0] {
            cyrup_core::Content::Text { text, .. } => text.to_string(),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(text, format!("all done\nMission: {} (completed)", record.id));
        assert_eq!(attached.details.as_ref().unwrap()["missionId"], record.id.as_str());
        assert_eq!(attached.details.as_ref().unwrap()["mission"]["id"], record.id.as_str());
    }

    #[test]
    fn a_failing_run_marks_the_mission_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("build".to_string()), ..Default::default() },
        )
        .unwrap();
        attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single",
                    "runId": "run-b",
                    "results": [{"agent": "builder", "exitCode": 2, "usage": {"input": 1, "output": 1}}],
                }),
                "it broke",
            ),
        )
        .unwrap();
        let record = read_mission(&binding.location, &binding.mission_id).unwrap();
        assert_eq!(record.status, MissionStatus::Failed);
        assert_eq!(record.runs[0].status.as_deref(), Some("failed"));
    }

    #[test]
    fn an_interrupted_run_pauses_the_mission_into_waiting() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("build".to_string()), ..Default::default() },
        )
        .unwrap();
        attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single",
                    "runId": "run-c",
                    "results": [{"agent": "builder", "exitCode": 0, "interrupted": true, "usage": {"input": 0, "output": 0}}],
                }),
                "paused",
            ),
        )
        .unwrap();
        let record = read_mission(&binding.location, &binding.mission_id).unwrap();
        assert_eq!(record.status, MissionStatus::Waiting);
        assert_eq!(record.runs[0].status.as_deref(), Some("paused"));
        // `lifecycle.ts:202` stamps `completedAt` for every status that is not "active" — a
        // PAUSED run is stamped, only a still-running async one is not.
        assert!(record.runs[0].completed_at.is_some());
    }

    #[test]
    fn the_announcement_is_suppressed_for_json_text_and_structured_output() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("build".to_string()), ..Default::default() },
        )
        .unwrap();
        let json_result = attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single", "runId": "run-json",
                    "results": [{"agent": "a", "exitCode": 0, "usage": {"input": 0, "output": 0}}],
                }),
                r#"{"ok":true}"#,
            ),
        )
        .unwrap();
        let text = match &json_result.content[0] {
            cyrup_core::Content::Text { text, .. } => text.to_string(),
            other => panic!("{other:?}"),
        };
        assert_eq!(text, r#"{"ok":true}"#, "a JSON payload must not be prose-appended to");

        let structured = attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single", "runId": "run-struct",
                    "results": [{"agent": "a", "exitCode": 0, "usage": {"input": 0, "output": 0}, "structuredOutputPath": "/tmp/s.json"}],
                }),
                "plain text",
            ),
        )
        .unwrap();
        let text = match &structured.content[0] {
            cyrup_core::Content::Text { text, .. } => text.to_string(),
            other => panic!("{other:?}"),
        };
        assert_eq!(text, "plain text");
    }

    /// The same suppression, keyed on the field cyrup's OWN producer writes:
    /// [`crate::exec::SingleResult::structured_output`] serializes as `structuredOutput` and there
    /// is no `structuredOutputPath` anywhere in this crate, so reading only upstream's key left the
    /// guard permanently false and appended `Mission: …` to every structured-output run.
    #[test]
    fn the_announcement_is_suppressed_by_the_structured_output_value_cyrup_actually_emits() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("build".to_string()), ..Default::default() },
        )
        .unwrap();
        let structured = attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single", "runId": "run-struct-value",
                    "results": [{
                        "agent": "a", "exitCode": 0, "usage": {"input": 0, "output": 0},
                        "structuredOutput": {"ok": true},
                    }],
                }),
                "plain text",
            ),
        )
        .unwrap();
        let text = match &structured.content[0] {
            cyrup_core::Content::Text { text, .. } => text.to_string(),
            other => panic!("{other:?}"),
        };
        assert_eq!(text, "plain text");

        // …and an ABSENT structured output (cyrup serializes `None` as an explicit `null`, since
        // `SingleResult::structured_output` carries no `skip_serializing_if`) must NOT suppress it.
        let plain = attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single", "runId": "run-plain",
                    "results": [{
                        "agent": "a", "exitCode": 0, "usage": {"input": 0, "output": 0},
                        "structuredOutput": serde_json::Value::Null,
                    }],
                }),
                "plain text",
            ),
        )
        .unwrap();
        let text = match &plain.content[0] {
            cyrup_core::Content::Text { text, .. } => text.to_string(),
            other => panic!("{other:?}"),
        };
        assert!(text.starts_with("plain text\nMission: "), "{text}");
    }

    #[test]
    fn a_background_run_writes_a_binding_file_that_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("background".to_string()), ..Default::default() },
        )
        .unwrap();
        let async_dir = tmp.path().join("async").join("run-bg");
        std::fs::create_dir_all(&async_dir).unwrap();
        attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single",
                    "asyncId": "run-bg",
                    "asyncDir": async_dir.to_string_lossy(),
                    "results": [],
                }),
                "started",
            ),
        )
        .unwrap();

        let read_back = read_mission_binding(&async_dir).unwrap().unwrap();
        assert_eq!(read_back.mission_id, binding.mission_id);
        assert_eq!(read_back.location, binding.location);
        assert!(!read_back.auto_created);
        assert!(!read_back.announce_in_content, "a persisted binding never re-announces");

        let record = read_mission(&binding.location, &binding.mission_id).unwrap();
        assert_eq!(record.status, MissionStatus::Active);
        assert_eq!(record.runs[0].status.as_deref(), Some("active"));
        assert!(record.runs[0].completed_at.is_none());
        assert_eq!(record.artifacts.len(), 2);
        assert!(record.artifacts.iter().any(|a| a.path.ends_with("status.json")));
        assert!(record.artifacts.iter().any(|a| a.path.ends_with("events.jsonl")));
        // An active run contributes no summary.
        assert!(record.summary.is_none());
    }

    #[test]
    fn read_mission_binding_is_none_when_there_is_no_binding() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_mission_binding(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn sync_from_async_completion_closes_the_mission() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("background".to_string()), ..Default::default() },
        )
        .unwrap();
        let async_dir = tmp.path().join("async").join("run-bg2");
        std::fs::create_dir_all(&async_dir).unwrap();
        attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single",
                    "asyncId": "run-bg2",
                    "asyncDir": async_dir.to_string_lossy(),
                    "results": [],
                }),
                "started",
            ),
        )
        .unwrap();

        let synced = sync_mission_from_async_completion(&serde_json::json!({
            "runId": "run-bg2",
            "asyncDir": async_dir.to_string_lossy(),
            "state": "complete",
            "mode": "single",
            "summary": "finished the job",
            "totalTokens": {"total": 512},
        }))
        .unwrap()
        .unwrap();
        assert_eq!(synced.status, MissionStatus::Completed);
        assert_eq!(synced.runs.len(), 1, "the same runId is a MERGE, not a second link");
        assert_eq!(synced.runs[0].status.as_deref(), Some("complete"));
        assert_eq!(synced.runs[0].usage.unwrap().tokens, 512);
        // The RECORD-level `usage` is written only for a GOAL mission
        // (`store.ts:465`'s `...(goal ? { goal, usage } : {})`); a plain mission carries its run
        // usage on the run link alone.
        assert!(synced.usage.is_none());
        assert_eq!(synced.summary.as_deref(), Some("finished the job"));
    }

    #[test]
    fn sync_is_a_no_op_without_an_async_dir_or_a_binding() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(sync_mission_from_async_completion(&Value::Null).unwrap().is_none());
        assert!(sync_mission_from_async_completion(&serde_json::json!({})).unwrap().is_none());
        assert!(
            sync_mission_from_async_completion(&serde_json::json!({
                "asyncDir": tmp.path().to_string_lossy(),
                "runId": "r",
            }))
            .unwrap()
            .is_none(),
            "an async dir with no mission.json has no mission to sync"
        );
    }

    #[test]
    fn sync_without_a_run_id_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("bg".to_string()), ..Default::default() },
        )
        .unwrap();
        let async_dir = tmp.path().join("async").join("run-noid");
        std::fs::create_dir_all(&async_dir).unwrap();
        write_async_binding(&async_dir, &binding).unwrap();
        let err = sync_mission_from_async_completion(&serde_json::json!({
            "asyncDir": async_dir.to_string_lossy(),
            "state": "complete",
        }))
        .unwrap_err();
        assert_eq!(err.to_string(), "Async mission completion is missing runId");
    }

    #[test]
    fn sync_leaves_a_breadcrumb_when_the_record_was_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("bg".to_string()), ..Default::default() },
        )
        .unwrap();
        let async_dir = tmp.path().join("async").join("run-gone");
        std::fs::create_dir_all(&async_dir).unwrap();
        write_async_binding(&async_dir, &binding).unwrap();
        std::fs::remove_file(mission_record_path(&binding.location, &binding.mission_id).unwrap())
            .unwrap();

        let synced = sync_mission_from_async_completion(&serde_json::json!({
            "asyncDir": async_dir.to_string_lossy(),
            "runId": "run-gone",
            "success": true,
        }))
        .unwrap();
        assert!(synced.is_none());
        let events = std::fs::read_to_string(async_dir.join("events.jsonl")).unwrap();
        assert!(events.contains("subagent.mission.sync.skipped"), "{events}");
        assert!(events.contains("mission-record-missing"), "{events}");
    }

    #[test]
    fn an_error_result_with_no_run_id_fails_the_mission() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams { task: Some("bg".to_string()), ..Default::default() },
        )
        .unwrap();
        let attached = attach_mission_to_launch_result(
            &binding,
            LaunchOutcome {
                content: vec![cyrup_core::Content::text("refused before launch")],
                details: Some(serde_json::json!({"mode": "single", "results": []})),
                is_error: true,
            },
        )
        .unwrap();
        let record = read_mission(&binding.location, &binding.mission_id).unwrap();
        assert_eq!(record.status, MissionStatus::Failed);
        assert_eq!(record.summary.as_deref(), Some("refused before launch"));
        assert!(record.runs.is_empty());
        // The no-run-id branch never announces in content.
        assert_eq!(attached.content.len(), 1);
        let text = match &attached.content[0] {
            cyrup_core::Content::Text { text, .. } => text.to_string(),
            other => panic!("{other:?}"),
        };
        assert_eq!(text, "refused before launch");
        assert_eq!(attached.details.as_ref().unwrap()["missionId"], record.id.as_str());
    }

    #[test]
    fn a_goal_mission_stays_active_after_a_completed_run() {
        let tmp = tempfile::tempdir().unwrap();
        let binding = prepared(
            tmp.path(),
            MissionLaunchParams {
                task: Some("keep going".to_string()),
                mission: Some(serde_json::json!({
                    "title": "Goal", "goal": true, "budget": {"tokens": 10_000}
                })),
                ..Default::default()
            },
        )
        .unwrap();
        attach_mission_to_launch_result(
            &binding,
            outcome(
                serde_json::json!({
                    "mode": "single", "runId": "run-goal",
                    "results": [{"agent": "a", "exitCode": 0, "usage": {"input": 5, "output": 5}}],
                }),
                "step done",
            ),
        )
        .unwrap();
        let record = read_mission(&binding.location, &binding.mission_id).unwrap();
        assert_eq!(record.status, MissionStatus::Active, "a goal mission does not self-close");
        assert_eq!(record.usage.unwrap().tokens, 10);
    }

    #[test]
    fn concise_title_ellipsizes_at_one_hundred_characters() {
        assert_eq!(concise_title("short"), "short");
        let long = "x".repeat(150);
        let title = concise_title(&long);
        assert_eq!(title.chars().count(), 100);
        assert!(title.ends_with("..."));
        assert_eq!(concise_title("\n\nsecond line wins when the first is blank"), "second line wins when the first is blank");
    }
}
