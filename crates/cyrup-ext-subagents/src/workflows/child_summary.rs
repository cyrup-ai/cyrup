//! The workflow child inventory builder and reader — ports pi
//! `workflows/workflow-child-summary.ts` (170 LOC @ `df26ebc8`): `workflowChildProgress` /
//! `workflowChildActivity` (`:10-29`), `workflowChildSummary` (`:52-141`, five passes in a fixed
//! order) and `parseWorkflowChildSummary` (`:143-170`, sixteen ordered rejections).
//!
//! Upstream's key-pattern `.test` guards (`:65`, `:84`, `:103`, `:156`) and its positional
//! `bounded(value, maxBytes)` helper (`:41-44`) do not appear here: the grammar is
//! [`WorkflowKey::parse`] applied once at each boundary (SCOPE_3 §A.3), and the per-field limits
//! ride in the row's field TYPES ([`Bounded`], §A.4).

use serde_json::Value;

use crate::background::{StepState, StepStatus};

use super::bounded::Bounded;
use super::key::WorkflowKey;
use super::types::{
    SummaryVersion, WorkflowChildActivity, WorkflowChildRow, WorkflowChildState,
    WorkflowChildSummary, WorkflowScriptChildResult, WorkflowScriptOperation,
    WorkflowScriptTraceEntry, WorkflowScriptTraceState, WorkflowState,
};

/// pi `MAX_REQUIRED_ID_BYTES` (`workflow-child-summary.ts:6`) — the bound on
/// `parentToolCallId`/`workflowRunId`, carried by the summary's `Bounded<4096>` fields.
const MAX_REQUIRED_ID_BYTES: usize = 4_096;

/// A rejected summary — every `throw` in `workflowChildSummary`/`parseWorkflowChildSummary`,
/// carrying upstream's message verbatim.
///
/// Distinct from the builder/reader returning an *absent* summary: SCOPE_3h's wait projection
/// propagates this error (`wait-completions.ts:104-105`) and would otherwise report a CORRUPT
/// inventory as an ABSENT one.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct WorkflowChildSummaryError(String);

impl WorkflowChildSummaryError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The upstream message, verbatim.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

/// pi `requiredId` (`workflow-child-summary.ts:46-50`): a thin `.ok_or(..)` over
/// [`Bounded::parse`], erring — with upstream's message verbatim — where the optional identity
/// fields would merely be dropped. The builder and the reader are its only two call sites, with
/// different labels (`parentToolCallId` vs `workflowChildren.parentToolCallId`), exactly as
/// upstream.
fn required_id(
    value: &str,
    label: &str,
) -> Result<Bounded<MAX_REQUIRED_ID_BYTES>, WorkflowChildSummaryError> {
    Bounded::parse(value).ok_or_else(|| {
        WorkflowChildSummaryError::new(format!(
            "{label} must be a non-empty identifier of at most {MAX_REQUIRED_ID_BYTES} UTF-8 bytes."
        ))
    })
}

/// An in-memory live-progress snapshot for one running workflow child — pi
/// `WorkflowChildLiveProgress` (`workflow-child-summary.ts:8`): a [`WorkflowChildActivity`] plus
/// the four identity fields the summary's live-progress pass overlays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChildLiveProgress {
    /// The child's agent name — bounded-or-**empty** (pi `:13`'s `bounded(...) ?? ""`), never
    /// omitted; the live-progress pass re-applies the bound and skips the empty string.
    pub agent: String,
    /// Display name for the child session, when derived at launch.
    pub session_name: Option<Bounded<256>>,
    /// The model the child is running on.
    pub model: Option<Bounded<256>>,
    /// The child's effective thinking level.
    pub thinking: Option<Bounded<32>>,
    /// The sanitized activity block.
    pub activity: WorkflowChildActivity,
}

/// The raw identity + activity fields `workflowChildProgress` reads off a live progress snapshot —
/// pi's `Pick<AgentProgress, "agent" | "sessionName" | "model" | "thinking">` plus the activity
/// members (`workflow-child-summary.ts:8-18`). A borrow-struct rather than a concrete progress
/// type because cyrup's in-process progress record (`exec/progress.rs`) does not carry pi's wire
/// shape; the workflow runtime (SCOPE_3f) supplies the fields it has.
#[derive(Clone, Copy, Debug)]
pub struct WorkflowChildProgressInput<'a> {
    /// pi `progress.agent`.
    pub agent: &'a str,
    /// pi `progress.sessionName`.
    pub session_name: Option<&'a str>,
    /// pi `progress.model`.
    pub model: Option<&'a str>,
    /// pi `progress.thinking`.
    pub thinking: Option<&'a str>,
    /// The raw activity members (pi reads them off the same progress object).
    pub activity: &'a WorkflowChildActivity,
}

/// pi `workflowChildProgress` (`workflow-child-summary.ts:10-18`): the live snapshot for the
/// summary's progress map — the sanitized activity plus the four bounded identity fields, with
/// `agent` defaulting to `""` rather than being omitted (`:13`).
#[must_use]
pub fn workflow_child_progress(input: WorkflowChildProgressInput<'_>) -> WorkflowChildLiveProgress {
    WorkflowChildLiveProgress {
        agent: Bounded::<256>::parse(input.agent)
            .map(|bounded| bounded.as_str().to_string())
            .unwrap_or_default(),
        session_name: input.session_name.and_then(Bounded::parse),
        model: input.model.and_then(Bounded::parse),
        thinking: input.thinking.and_then(Bounded::parse),
        activity: workflow_child_activity(input.activity),
    }
}

/// pi `workflowChildActivity` (`workflow-child-summary.ts:21-29`): keeps `current_tool` when it
/// passes its bound (structural here — the field is `Bounded<256>` by construction) and each of
/// the eight counters only when it is finite and `>= 0`.
///
/// Fixed field count and a 256 UTF-8 byte tool name keep JSON activity below 2 KiB, including
/// escaping.
#[must_use]
pub fn workflow_child_activity(raw: &WorkflowChildActivity) -> WorkflowChildActivity {
    let keep = |value: &Option<serde_json::Number>| -> Option<serde_json::Number> {
        value
            .as_ref()
            .filter(|number| {
                number
                    .as_f64()
                    .is_some_and(|float| float.is_finite() && float >= 0.0)
            })
            .cloned()
    };
    WorkflowChildActivity {
        current_tool: raw.current_tool.clone(),
        current_tool_started_at: keep(&raw.current_tool_started_at),
        last_activity_at: keep(&raw.last_activity_at),
        duration_ms: keep(&raw.duration_ms),
        tool_count: keep(&raw.tool_count),
        turn_count: keep(&raw.turn_count),
        tokens: keep(&raw.tokens),
        input_tokens: keep(&raw.input_tokens),
        output_tokens: keep(&raw.output_tokens),
    }
}

/// The builder's input — pi `workflowChildSummary`'s single argument
/// (`workflow-child-summary.ts:52-62`); upstream's optional lists are empty slices here.
#[derive(Clone, Copy, Debug)]
pub struct WorkflowChildSummaryInput<'a> {
    /// The parent tool call this workflow answers to (required, ≤ 4096 UTF-8 bytes).
    pub parent_tool_call_id: &'a str,
    /// The workflow run's own id (same requiredness/bound).
    pub workflow_run_id: &'a str,
    /// The run-level workflow state.
    pub workflow_state: WorkflowState,
    /// Whether the inventory is settled (triggers the non-terminal sweep).
    pub inventory_complete: bool,
    /// The script's execution trace (pass 1).
    pub trace: &'a [WorkflowScriptTraceEntry],
    /// The runner's status steps (pass 2).
    pub steps: &'a [StepStatus],
    /// The settled child results (pass 3).
    pub children: &'a [WorkflowScriptChildResult],
    /// In-memory snapshots from synchronous child callbacks, keyed by workflow key, in insertion
    /// order (pi's `ReadonlyMap`) — pass 5.
    pub progress: &'a [(WorkflowKey, WorkflowChildLiveProgress)],
}

/// Replace-in-place if the key is present, else push — JS `rows.set(key, …)` over a `Map`, on the
/// `Vec` that stands in for it so the output preserves **insertion order** (SCOPE_3d §0.16: a
/// `BTreeMap` would silently re-sort the inventory, a `HashMap` would randomize it, and
/// `indexmap` is not a dependency of this crate).
fn upsert(rows: &mut Vec<(WorkflowKey, WorkflowChildRow)>, key: WorkflowKey, row: WorkflowChildRow) {
    if let Some(slot) = rows.iter_mut().find(|(existing, _)| *existing == key) {
        slot.1 = row;
    } else {
        rows.push((key, row));
    }
}

/// pi `workflowChildSummary` (`workflow-child-summary.ts:52-141`) — five passes, in upstream's
/// order, each later pass overwriting the earlier. That ordering is the design: the trace is what
/// the script *did*, the steps are what the runner *recorded*, the children are the *settled*
/// results, the inventory sweep closes the books, and the progress map is *live* truth for a
/// child still running.
///
/// # Errors
///
/// [`WorkflowChildSummaryError`] with upstream's `requiredId` message when
/// `parent_tool_call_id`/`workflow_run_id` is blank or over 4096 UTF-8 bytes (`:134-135`) — the
/// only rejections the builder has; a bad key or over-long identity field in the inputs is
/// dropped, not an error.
pub fn workflow_child_summary(
    input: WorkflowChildSummaryInput<'_>,
) -> Result<WorkflowChildSummary, WorkflowChildSummaryError> {
    let mut rows: Vec<(WorkflowKey, WorkflowChildRow)> = Vec::new();

    // Pass 1 — trace (`:64-81`). Only `operation == "run"`, only parseable keys (`:65`'s
    // key-pattern `.test` is the parse). `previous?.state ?? "running"` (`:71`) is why the fallback
    // reads the EXISTING row: a `started` entry with no terminal follow-up must stay `running`,
    // and a second non-terminal entry must not reset a state an earlier entry already advanced.
    // `runId` comes off the TRACE ENTRY (`:74`) — a later entry without one DROPS it — while
    // agent/sessionName/model/thinking are carried forward from `previous` (`:76-79`); the trace
    // carries `agent` but this pass deliberately ignores it.
    for entry in input.trace {
        if entry.operation != WorkflowScriptOperation::Run {
            continue;
        }
        let Ok(key) = WorkflowKey::parse(&entry.key) else {
            continue;
        };
        let previous = rows
            .iter()
            .find(|(existing, _)| *existing == key)
            .map(|(_, row)| row.clone());
        let state = match entry.state {
            WorkflowScriptTraceState::Completed => WorkflowChildState::Completed,
            WorkflowScriptTraceState::Failed => WorkflowChildState::Failed,
            WorkflowScriptTraceState::Stopped => WorkflowChildState::Stopped,
            WorkflowScriptTraceState::Detached => WorkflowChildState::Detached,
            // 4-way plus the fallback (`:66-71`); `paused` is NOT reachable from a trace entry.
            WorkflowScriptTraceState::Started
            | WorkflowScriptTraceState::Reused
            | WorkflowScriptTraceState::Queued
            | WorkflowScriptTraceState::Delivered
            | WorkflowScriptTraceState::Missed => previous
                .as_ref()
                .map_or(WorkflowChildState::Running, |row| row.state),
        };
        let row = WorkflowChildRow {
            child_id: key.clone(),
            state,
            run_id: entry.run_id.as_deref().and_then(Bounded::parse),
            agent: previous.as_ref().and_then(|row| row.agent.clone()),
            session_name: previous.as_ref().and_then(|row| row.session_name.clone()),
            model: previous.as_ref().and_then(|row| row.model.clone()),
            thinking: previous.as_ref().and_then(|row| row.thinking.clone()),
            activity: None,
        };
        upsert(&mut rows, key, row);
    }

    // Pass 2 — status steps (`:82-101`). The key is `StepStatus::workflow_key`, a `WorkflowKey`
    // by construction, so `:84`'s grammar re-check is structurally absent. The state map is
    // upstream's 7-way over the status word; two of its arms are shaped by the port:
    //   * both spellings of complete — wire `"complete"` (cyrup's `StepState::Complete`) and
    //     `"completed"` — collapse to `Completed` (`:85`, SCOPE_3d §0.19), which the typed match
    //     makes structural;
    //   * the `"rejected"` arm is DEAD here — `StepState` has no `Rejected`; pass 3 is the only
    //     live producer of `WorkflowChildState::Rejected` in cyrup.
    for step in input.steps {
        let Some(key) = step.workflow_key.clone() else {
            continue;
        };
        let state = match step.status {
            StepState::Complete => WorkflowChildState::Completed,
            StepState::Failed => WorkflowChildState::Failed,
            StepState::Paused => WorkflowChildState::Paused,
            StepState::Stopped => WorkflowChildState::Stopped,
            StepState::Pending => WorkflowChildState::Pending,
            StepState::Running => WorkflowChildState::Running,
        };
        // pi `:91`'s four launch witnesses are `async`/`sessionFile`/`runId`/`model`; cyrup's
        // `StepStatus` has no per-step `async` flag (a workflow-runtime product, SCOPE_3f), so
        // the guard is the three witnesses that exist.
        let launch_resolved =
            step.session_file.is_some() || step.run_id.is_some() || step.model.is_some();
        let row = WorkflowChildRow {
            child_id: key.clone(),
            state,
            run_id: step
                .run_id
                .as_ref()
                .and_then(|run_id| Bounded::parse(run_id.as_str())),
            // The guard applies to `agent` ONLY (`:96`): without it a PENDING step reports the
            // agent it INTENDS to run and a reader cannot distinguish "queued to run X" from
            // "ran X". sessionName/model/thinking are copied unconditionally (`:97-99`).
            agent: if launch_resolved {
                Bounded::parse(&step.agent)
            } else {
                None
            },
            session_name: step.session_name.as_deref().and_then(Bounded::parse),
            model: step
                .model
                .as_ref()
                .and_then(|model| Bounded::parse(model.as_str())),
            thinking: step.telemetry.thinking.as_deref().and_then(Bounded::parse),
            activity: None,
        };
        upsert(&mut rows, key, row);
    }

    // Pass 3 — settled children (`:102-115`), five-way in this precedence: detached ⇒ stopped ⇒
    // interrupted (⇒ Paused) ⇒ ok (⇒ Completed) ⇒ acceptance rejected ⇒ Failed. §0.18's
    // two-object field split: `runId`/`agent` come from the CHILD, while
    // `sessionName`/`model`/`thinking` come from the RESULT — the first *object* element of
    // `child.results` (`:104`; pi's `typeof v === "object"` also matches an array, whose field
    // reads then yield nothing, so the find mirrors that and the field reads require a real map).
    for child in input.children {
        let Ok(key) = WorkflowKey::parse(&child.key) else {
            continue;
        };
        let result = child
            .results
            .iter()
            .find(|value| matches!(value, Value::Object(_) | Value::Array(_)))
            .and_then(Value::as_object);
        let rejected = result
            .and_then(|map| map.get("acceptance"))
            .and_then(Value::as_object)
            .and_then(|acceptance| acceptance.get("status"))
            .and_then(Value::as_str)
            == Some("rejected");
        let state = if child.detached {
            WorkflowChildState::Detached
        } else if child.stopped {
            WorkflowChildState::Stopped
        } else if child.interrupted {
            WorkflowChildState::Paused
        } else if child.ok {
            WorkflowChildState::Completed
        } else if rejected {
            WorkflowChildState::Rejected
        } else {
            WorkflowChildState::Failed
        };
        let result_field = |name: &str| -> Option<&str> {
            result.and_then(|map| map.get(name)).and_then(Value::as_str)
        };
        let row = WorkflowChildRow {
            child_id: key.clone(),
            state,
            run_id: child.run_id.as_deref().and_then(Bounded::parse),
            agent: child.agent.as_deref().and_then(Bounded::parse),
            session_name: result_field("sessionName").and_then(Bounded::parse),
            model: result_field("model").and_then(Bounded::parse),
            thinking: result_field("thinking").and_then(Bounded::parse),
            activity: None,
        };
        upsert(&mut rows, key, row);
    }

    // Pass 4 — inventory sweep (`:116-120`): once the inventory is complete, every row still
    // non-terminal (in the six-of-eight `TERMINAL_STATES` sense — see
    // `WorkflowChildState::is_terminal_for_inventory`) is forced to `stopped` when the workflow
    // itself stopped, else `failed`.
    if input.inventory_complete {
        for (_, row) in &mut rows {
            if !row.state.is_terminal_for_inventory() {
                row.state = if input.workflow_state == WorkflowState::Stopped {
                    WorkflowChildState::Stopped
                } else {
                    WorkflowChildState::Failed
                };
            }
        }
    }

    // Pass 5 — live progress (`:121-132`): applied ONLY to rows already `running` (`:123`), and
    // the only pass that sets `activity`. A progress entry for an unknown or settled key is
    // dropped, which is what keeps `activity` and `state` consistent for the reader's `:163`
    // rule. Identity fields overlay only when bounded-and-present (`:125-128`); `activity` is set
    // unconditionally (`:129`).
    for (key, progress) in input.progress {
        let Some((_, row)) = rows.iter_mut().find(|(existing, _)| existing == key) else {
            continue;
        };
        if row.state != WorkflowChildState::Running {
            continue;
        }
        if let Some(agent) = Bounded::parse(&progress.agent) {
            row.agent = Some(agent);
        }
        if let Some(session_name) = &progress.session_name {
            row.session_name = Some(session_name.clone());
        }
        if let Some(model) = &progress.model {
            row.model = Some(model.clone());
        }
        if let Some(thinking) = &progress.thinking {
            row.thinking = Some(thinking.clone());
        }
        row.activity = Some(workflow_child_activity(&progress.activity));
    }

    Ok(WorkflowChildSummary {
        version: SummaryVersion,
        parent_tool_call_id: required_id(input.parent_tool_call_id, "parentToolCallId")?,
        workflow_run_id: required_id(input.workflow_run_id, "workflowRunId")?,
        inventory_complete: input.inventory_complete,
        workflow_state: input.workflow_state,
        children: rows.into_iter().map(|(_, row)| row).collect(),
    })
}

/// The eight activity counter keys — pi `ACTIVITY_COUNTERS` (`workflow-child-summary.ts:7`).
const ACTIVITY_COUNTERS: [&str; 8] = [
    "currentToolStartedAt",
    "lastActivityAt",
    "durationMs",
    "toolCount",
    "turnCount",
    "tokens",
    "inputTokens",
    "outputTokens",
];

/// The top-level allow-list (`parseWorkflowChildSummary`, `:147`) — `deny_unknown_fields` with
/// every field a raw [`Value`], so the ONLY error this shape can produce is rule 2's unknown-key
/// rejection and the source-order rules 3+ stay hand-validated.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawSummaryShape {
    version: Option<Value>,
    parent_tool_call_id: Option<Value>,
    workflow_run_id: Option<Value>,
    inventory_complete: Option<Value>,
    workflow_state: Option<Value>,
    children: Option<Value>,
}

/// The child-row allow-list (`:155`) — same construction as [`RawSummaryShape`], for rule 6.
///
/// `dead_code` is allowed because the fields exist purely so `deny_unknown_fields` names the
/// allow-list; the VALUES are read off the raw map instead, because `Option<Value>` collapses a
/// present-but-`null` field to `None` and the reader must distinguish the two (a `null` `runId`
/// is an ERROR upstream, an absent one is not).
#[expect(
    dead_code,
    reason = "the fields ARE the deny_unknown_fields allow-list; values are read presence-aware off the map"
)]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawChildShape {
    child_id: Option<Value>,
    run_id: Option<Value>,
    agent: Option<Value>,
    session_name: Option<Value>,
    model: Option<Value>,
    thinking: Option<Value>,
    state: Option<Value>,
    activity: Option<Value>,
}

fn err(message: impl Into<String>) -> WorkflowChildSummaryError {
    WorkflowChildSummaryError::new(message)
}

/// pi `parseActivity` (`workflow-child-summary.ts:31-39`): absent ⇒ `None`; not an object (or an
/// array) ⇒ error; each key must be `currentTool` passing its 256-byte bound or one of the eight
/// counters holding a finite `>= 0` number — one message for every violation.
fn parse_activity(
    value: Option<&Value>,
) -> Result<Option<WorkflowChildActivity>, WorkflowChildSummaryError> {
    const INVALID: &str = "workflowChildren child activity is invalid.";
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(map) = value.as_object() else {
        return Err(err(INVALID));
    };
    let mut activity = WorkflowChildActivity::default();
    for (key, entry) in map {
        if key == "currentTool" {
            let tool = entry.as_str().and_then(Bounded::parse);
            match tool {
                Some(tool) => activity.current_tool = Some(tool),
                None => return Err(err(INVALID)),
            }
            continue;
        }
        if !ACTIVITY_COUNTERS.contains(&key.as_str()) {
            return Err(err(INVALID));
        }
        let number = entry
            .as_number()
            .filter(|number| {
                number
                    .as_f64()
                    .is_some_and(|float| float.is_finite() && float >= 0.0)
            })
            .cloned();
        let Some(number) = number else {
            return Err(err(INVALID));
        };
        match key.as_str() {
            "currentToolStartedAt" => activity.current_tool_started_at = Some(number),
            "lastActivityAt" => activity.last_activity_at = Some(number),
            "durationMs" => activity.duration_ms = Some(number),
            "toolCount" => activity.tool_count = Some(number),
            "turnCount" => activity.turn_count = Some(number),
            "tokens" => activity.tokens = Some(number),
            "inputTokens" => activity.input_tokens = Some(number),
            // Exhaustive over `ACTIVITY_COUNTERS` — the membership test above admits nothing
            // else.
            _ => activity.output_tokens = Some(number),
        }
    }
    Ok(Some(activity))
}

/// One optional bounded identity field of a child row (reader rule 9, `:159-161`): absent ⇒
/// `None`; present but non-string, blank or over its limit ⇒ upstream's per-field message.
fn optional_bounded<const N: usize>(
    value: Option<&Value>,
    field: &str,
) -> Result<Option<Bounded<N>>, WorkflowChildSummaryError> {
    match value {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .and_then(Bounded::parse)
            .map(Some)
            .ok_or_else(|| err(format!("workflowChildren child {field} is invalid."))),
    }
}

/// pi `parseWorkflowChildSummary` (`workflow-child-summary.ts:143-170`).
///
/// `Ok(None)` is upstream's `value === undefined` early return (`:144`); `Err` is every `throw`
/// (a JSON `null` is NOT absent and rejects as a non-object, matching upstream's `!value` guard).
/// Do NOT collapse them — SCOPE_3h's wait projection distinguishes them
/// (`wait-completions.ts:104-105` propagates the throw and would otherwise report a CORRUPT
/// inventory as an ABSENT one).
///
/// # Errors
///
/// The sixteen rejections, **in source order** (the order is observable: a bad child row,
/// `:153-163`, errors before a non-string `parentToolCallId`, `:167`), each with upstream's
/// message verbatim.
pub fn parse_workflow_child_summary(
    value: Option<&Value>,
) -> Result<Option<WorkflowChildSummary>, WorkflowChildSummaryError> {
    let Some(value) = value else {
        return Ok(None);
    };
    // Rule 1 (`:145`) — not an object, or an array (a JSON `null` lands here too, matching
    // upstream's `!value`).
    if !value.is_object() {
        return Err(err("workflowChildren must be an object."));
    }
    // Rule 2 (`:148`) — the top-level allow-list; `RawSummaryShape` can fail on nothing else.
    let Ok(shape) = RawSummaryShape::deserialize_from(value) else {
        return Err(err("workflowChildren has unsupported fields."));
    };
    // Rule 3 (`:149`) — `version != 1` (JS numeric equality: `1.0` passes), non-bool
    // `inventoryComplete`, or non-array `children`.
    let version_ok = shape
        .version
        .as_ref()
        .and_then(Value::as_f64)
        .is_some_and(|version| version == 1.0);
    let inventory_complete = shape.inventory_complete.as_ref().and_then(Value::as_bool);
    let children_raw = shape.children.as_ref().and_then(Value::as_array);
    let (true, Some(inventory_complete), Some(children_raw)) =
        (version_ok, inventory_complete, children_raw)
    else {
        return Err(err("workflowChildren is invalid."));
    };
    // Rule 4 (`:151`) — `workflowState` not one of the six.
    let workflow_state = match shape.workflow_state.as_ref().and_then(Value::as_str) {
        Some("queued") => WorkflowState::Queued,
        Some("running") => WorkflowState::Running,
        Some("completed") => WorkflowState::Completed,
        Some("failed") => WorkflowState::Failed,
        Some("paused") => WorkflowState::Paused,
        Some("stopped") => WorkflowState::Stopped,
        _ => return Err(err("workflowChildren.workflowState is invalid.")),
    };
    let mut children: Vec<WorkflowChildRow> = Vec::with_capacity(children_raw.len());
    for row_value in children_raw {
        // Rule 5 (`:153`) — a child row that is not an object, or is an array.
        let Some(row) = row_value.as_object() else {
            return Err(err("workflowChildren child row is invalid."));
        };
        // Rule 6 (`:155`) — the child-row allow-list.
        if RawChildShape::deserialize_from(row_value).is_err() {
            return Err(err("workflowChildren child row has unsupported fields."));
        }
        // Rule 7 (`:156`) — `childId` non-string or failing the grammar.
        let child_id = row
            .get("childId")
            .and_then(Value::as_str)
            .and_then(|raw| WorkflowKey::parse(raw).ok());
        let Some(child_id) = child_id else {
            return Err(err("workflowChildren childId is invalid."));
        };
        // Rule 8 (`:158`) — `state` not one of the eight.
        let state = match row.get("state").and_then(Value::as_str) {
            Some("pending") => WorkflowChildState::Pending,
            Some("running") => WorkflowChildState::Running,
            Some("completed") => WorkflowChildState::Completed,
            Some("failed") => WorkflowChildState::Failed,
            Some("paused") => WorkflowChildState::Paused,
            Some("stopped") => WorkflowChildState::Stopped,
            Some("rejected") => WorkflowChildState::Rejected,
            Some("detached") => WorkflowChildState::Detached,
            _ => return Err(err("workflowChildren child state is invalid.")),
        };
        // Rule 9 (`:159-161`) — the five bounded identity fields, in upstream's table order.
        let run_id = optional_bounded::<256>(row.get("runId"), "runId")?;
        let agent = optional_bounded::<256>(row.get("agent"), "agent")?;
        let session_name = optional_bounded::<256>(row.get("sessionName"), "sessionName")?;
        let model = optional_bounded::<256>(row.get("model"), "model")?;
        let thinking = optional_bounded::<32>(row.get("thinking"), "thinking")?;
        // Rules 10/11 (`:33`/`:36`) — the nested activity validator.
        let activity = parse_activity(row.get("activity"))?;
        // Rule 12 (`:163`) — an `activity` (even an empty one: presence, not non-emptiness) on a
        // child whose state is not `running`.
        if activity.is_some() && state != WorkflowChildState::Running {
            return Err(err("workflowChildren child activity requires running state."));
        }
        children.push(WorkflowChildRow {
            child_id,
            state,
            run_id,
            agent,
            session_name,
            model,
            thinking,
            activity,
        });
    }
    // Rule 13 (`:166`) — duplicate `childId`.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    if !children
        .iter()
        .all(|child| seen.insert(child.child_id.as_str()))
    {
        return Err(err("workflowChildren has duplicate childId values."));
    }
    // Rule 14 (`:167`) — non-string (including absent) `parentToolCallId`.
    let Some(parent_tool_call_id) = shape.parent_tool_call_id.as_ref().and_then(Value::as_str)
    else {
        return Err(err("workflowChildren.parentToolCallId is invalid."));
    };
    // Rule 15 (`:168`) — non-string `workflowRunId`.
    let Some(workflow_run_id) = shape.workflow_run_id.as_ref().and_then(Value::as_str) else {
        return Err(err("workflowChildren.workflowRunId is invalid."));
    };
    // Rule 16 (`:169`) — the two required ids, blank or over 4096 bytes, parent first.
    Ok(Some(WorkflowChildSummary {
        version: SummaryVersion,
        parent_tool_call_id: required_id(
            parent_tool_call_id,
            "workflowChildren.parentToolCallId",
        )?,
        workflow_run_id: required_id(workflow_run_id, "workflowChildren.workflowRunId")?,
        inventory_complete,
        workflow_state,
        children,
    }))
}

impl RawSummaryShape {
    /// `Deserialize` from a borrowed [`Value`] — split out so the call sites above stay
    /// `let`-else shaped.
    fn deserialize_from(value: &Value) -> Result<Self, serde_json::Error> {
        serde::Deserialize::deserialize(value)
    }
}

impl RawChildShape {
    /// See [`RawSummaryShape::deserialize_from`].
    fn deserialize_from(value: &Value) -> Result<Self, serde_json::Error> {
        serde::Deserialize::deserialize(value)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use serde_json::json;

    use super::*;

    fn key(raw: &str) -> WorkflowKey {
        WorkflowKey::parse(raw).expect("valid key")
    }

    fn run_entry(raw_key: &str, state: WorkflowScriptTraceState) -> WorkflowScriptTraceEntry {
        WorkflowScriptTraceEntry {
            operation: WorkflowScriptOperation::Run,
            key: raw_key.to_string(),
            state,
            agent: Some("traced-agent".to_string()),
            run_id: None,
            duration_ms: None,
            phase: None,
            label: None,
            error: None,
            generated_lane_key: None,
            lane: None,
            warning: None,
        }
    }

    fn base_input<'a>() -> WorkflowChildSummaryInput<'a> {
        WorkflowChildSummaryInput {
            parent_tool_call_id: "tool-call-1",
            workflow_run_id: "wf-run-1",
            workflow_state: WorkflowState::Running,
            inventory_complete: false,
            trace: &[],
            steps: &[],
            children: &[],
            progress: &[],
        }
    }

    /// Pass 1: `started` stays `running`; a terminal follow-up advances; a later non-terminal
    /// entry keeps the advanced state (`previous?.state ?? "running"`), while `runId` comes off
    /// each entry alone — a later entry without one drops it.
    #[test]
    fn trace_pass_carries_state_forward_and_run_id_per_entry() {
        let mut with_run_id = run_entry("lane", WorkflowScriptTraceState::Started);
        with_run_id.run_id = Some("run-1".to_string());
        let trace = vec![
            with_run_id,
            run_entry("lane", WorkflowScriptTraceState::Completed),
            run_entry(".bad-key", WorkflowScriptTraceState::Started),
        ];
        let summary = workflow_child_summary(WorkflowChildSummaryInput {
            trace: &trace,
            ..base_input()
        })
        .expect("builds");
        assert_eq!(summary.children.len(), 1, "the bad key is dropped, not an error");
        let row = summary.children.first().expect("one row");
        assert_eq!(row.state, WorkflowChildState::Completed);
        assert_eq!(row.run_id, None, "the completed entry carried no runId; the old one drops");
        assert_eq!(row.agent, None, "the trace pass ignores the entry's own agent");
    }

    /// Pass 2: the `launchResolved` guard gates `agent` ONLY; `StepState::Complete` (wire
    /// `"complete"`) maps to the summary's `completed` (§0.19); a keyless step contributes
    /// nothing.
    #[test]
    fn step_pass_gates_agent_on_launch_resolution_only() {
        let mut unresolved = StepStatus::pending("pending-agent");
        unresolved.workflow_key = Some(key("lane.a"));
        unresolved.session_name = Some("Lane A".to_string());
        let mut resolved = StepStatus::pending("real-agent");
        resolved.workflow_key = Some(key("lane.b"));
        resolved.status = StepState::Complete;
        resolved.run_id = Some(crate::background::RunId::from_token("runbtoken001"));
        let keyless = StepStatus::pending("no-key");
        let steps = vec![unresolved, resolved, keyless];
        let summary = workflow_child_summary(WorkflowChildSummaryInput {
            steps: &steps,
            ..base_input()
        })
        .expect("builds");
        assert_eq!(summary.children.len(), 2);
        let first = summary.children.first().expect("row 0");
        assert_eq!(first.state, WorkflowChildState::Pending);
        assert_eq!(first.agent, None, "a pending step must not report its intended agent");
        assert_eq!(
            first.session_name.as_ref().map(Bounded::as_str),
            Some("Lane A"),
            "sessionName is copied unconditionally"
        );
        let second = summary.children.get(1).expect("row 1");
        assert_eq!(second.state, WorkflowChildState::Completed);
        assert_eq!(second.agent.as_ref().map(Bounded::as_str), Some("real-agent"));
        assert_eq!(second.run_id.as_ref().map(Bounded::as_str), Some("runbtoken001"));
    }

    /// Pass 3: the five-way precedence, the `Rejected` arm (the only live producer in cyrup),
    /// and §0.18's split — `runId`/`agent` from the child, `sessionName`/`model`/`thinking` from
    /// the first OBJECT element of `results` (a non-object element like `null` is skipped, but an
    /// ARRAY counts as pi's `typeof === "object"` and shadows a later real object).
    #[test]
    fn child_pass_reads_identity_from_two_objects_and_produces_rejected() {
        let rejected_child = WorkflowScriptChildResult {
            key: "lane.r".to_string(),
            ok: false,
            agent: Some("child-agent".to_string()),
            run_id: Some("child-run".to_string()),
            results: vec![
                json!(null),
                json!({
                    "sessionName": "Session R",
                    "model": "model-r",
                    "thinking": "high",
                    "acceptance": { "status": "rejected" },
                }),
            ],
            ..WorkflowScriptChildResult::default()
        };
        let stopped_child = WorkflowScriptChildResult {
            key: "lane.s".to_string(),
            ok: true,
            stopped: true,
            ..WorkflowScriptChildResult::default()
        };
        let array_shadowed = WorkflowScriptChildResult {
            key: "lane.f".to_string(),
            ok: false,
            results: vec![
                json!(["not", "an object"]),
                json!({ "sessionName": "Hidden", "acceptance": { "status": "rejected" } }),
            ],
            ..WorkflowScriptChildResult::default()
        };
        let children = vec![rejected_child, stopped_child, array_shadowed];
        let summary = workflow_child_summary(WorkflowChildSummaryInput {
            children: &children,
            ..base_input()
        })
        .expect("builds");
        let first = summary.children.first().expect("row 0");
        assert_eq!(first.state, WorkflowChildState::Rejected);
        assert_eq!(first.agent.as_ref().map(Bounded::as_str), Some("child-agent"));
        assert_eq!(first.run_id.as_ref().map(Bounded::as_str), Some("child-run"));
        assert_eq!(
            first.session_name.as_ref().map(Bounded::as_str),
            Some("Session R"),
            "null is skipped; the object supplies the result-side identity"
        );
        assert_eq!(first.thinking.as_ref().map(Bounded::as_str), Some("high"));
        let second = summary.children.get(1).expect("row 1");
        assert_eq!(second.state, WorkflowChildState::Stopped, "stopped outranks ok");
        // pi's find takes the FIRST `typeof === "object"` element — the ARRAY — and reads nothing
        // off it: the rejected acceptance behind it is invisible, so the child settles Failed with
        // no result-side identity. The port mirrors that exactly.
        let third = summary.children.get(2).expect("row 2");
        assert_eq!(third.state, WorkflowChildState::Failed);
        assert_eq!(third.session_name, None);
    }

    /// Passes 4 + 5: the inventory sweep forces non-terminal rows to `stopped`/`failed` (and
    /// `paused` counts as terminal HERE), then live progress applies only to rows still
    /// `running` — and is the only source of `activity`.
    #[test]
    fn sweep_and_progress_interact_exactly_as_upstream() {
        let trace = vec![
            run_entry("lane.live", WorkflowScriptTraceState::Started),
            run_entry("lane.stuck", WorkflowScriptTraceState::Started),
            run_entry("lane.done", WorkflowScriptTraceState::Completed),
        ];
        let progress = vec![(
            key("lane.live"),
            WorkflowChildLiveProgress {
                agent: "live-agent".to_string(),
                activity: WorkflowChildActivity {
                    tool_count: Some(serde_json::Number::from(3u32)),
                    ..WorkflowChildActivity::default()
                },
                ..WorkflowChildLiveProgress::default()
            },
        )];
        // Not inventory-complete: the live row keeps running and gains activity.
        let live = workflow_child_summary(WorkflowChildSummaryInput {
            trace: &trace,
            progress: &progress,
            ..base_input()
        })
        .expect("builds");
        let row = live.children.first().expect("live row");
        assert_eq!(row.state, WorkflowChildState::Running);
        assert_eq!(row.agent.as_ref().map(Bounded::as_str), Some("live-agent"));
        assert!(row.activity.is_some(), "the progress pass is the only activity source");

        // Inventory-complete on a stopped workflow: stragglers become `stopped`, and the sweep
        // runs BEFORE progress, so the settled row no longer accepts the overlay.
        let swept = workflow_child_summary(WorkflowChildSummaryInput {
            trace: &trace,
            progress: &progress,
            inventory_complete: true,
            workflow_state: WorkflowState::Stopped,
            ..base_input()
        })
        .expect("builds");
        assert!(
            swept
                .children
                .iter()
                .take(2)
                .all(|row| row.state == WorkflowChildState::Stopped),
            "non-terminal rows are forced to stopped"
        );
        assert_eq!(
            swept.children.get(2).expect("done row").state,
            WorkflowChildState::Completed,
            "a terminal row is untouched"
        );
        assert!(
            swept.children.iter().all(|row| row.activity.is_none()),
            "no row is running, so progress applies to none"
        );
    }

    /// The builder's only rejections: the two required ids, parent first, upstream's message
    /// verbatim.
    #[test]
    fn builder_rejects_bad_required_ids_in_order() {
        let blank_parent = workflow_child_summary(WorkflowChildSummaryInput {
            parent_tool_call_id: "  ",
            ..base_input()
        })
        .expect_err("blank parent id");
        assert_eq!(
            blank_parent.message(),
            "parentToolCallId must be a non-empty identifier of at most 4096 UTF-8 bytes."
        );
        let long_run = "x".repeat(4097);
        let bad_run = workflow_child_summary(WorkflowChildSummaryInput {
            workflow_run_id: &long_run,
            ..base_input()
        })
        .expect_err("over-long run id");
        assert_eq!(
            bad_run.message(),
            "workflowRunId must be a non-empty identifier of at most 4096 UTF-8 bytes."
        );
    }

    fn valid_summary_json() -> Value {
        json!({
            "version": 1,
            "parentToolCallId": "tool-1",
            "workflowRunId": "wf-1",
            "inventoryComplete": false,
            "workflowState": "running",
            "children": [
                { "childId": "lane.a", "state": "running",
                  "activity": { "currentTool": "edit", "toolCount": 2 } },
                { "childId": "lane.b", "state": "completed", "runId": "r-2" },
            ],
        })
    }

    /// `Ok(None)` for absent; `Err` for `null` — the two must stay distinguishable (SCOPE_3h).
    #[test]
    fn reader_distinguishes_absent_from_invalid() {
        assert_eq!(parse_workflow_child_summary(None), Ok(None));
        assert!(parse_workflow_child_summary(Some(&Value::Null)).is_err());
        let parsed = parse_workflow_child_summary(Some(&valid_summary_json()))
            .expect("valid")
            .expect("present");
        assert_eq!(parsed.children.len(), 2);
        assert_eq!(
            parsed.children.first().expect("row").activity,
            Some(WorkflowChildActivity {
                current_tool: Bounded::parse("edit"),
                tool_count: Some(serde_json::Number::from(2u32)),
                ..WorkflowChildActivity::default()
            })
        );
    }

    /// Every rejection message, exercised in source order against a minimally-broken payload —
    /// including the observable orderings: unknown fields before a bad version, a bad child row
    /// before a bad `parentToolCallId`, and duplicates before the id checks.
    #[test]
    fn reader_rejects_each_rule_with_upstreams_message() {
        let cases: Vec<(Value, &str)> = vec![
            (json!([]), "workflowChildren must be an object."),
            // Rule 2 fires even though `version` is ALSO bad — the allow-list is checked first.
            (
                json!({ "version": 2, "bogus": true }),
                "workflowChildren has unsupported fields.",
            ),
            (json!({ "version": 2 }), "workflowChildren is invalid."),
            (
                json!({ "version": 1, "inventoryComplete": false, "children": [],
                        "workflowState": "sideways" }),
                "workflowChildren.workflowState is invalid.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["children"] = json!(["row"]);
                    value
                },
                "workflowChildren child row is invalid.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["children"] = json!([{ "childId": "a", "state": "running", "extra": 1 }]);
                    value
                },
                "workflowChildren child row has unsupported fields.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["children"] = json!([{ "childId": ".bad", "state": "running" }]);
                    value
                },
                "workflowChildren childId is invalid.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["children"] = json!([{ "childId": "a", "state": "done" }]);
                    value
                },
                "workflowChildren child state is invalid.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["children"] = json!([{ "childId": "a", "state": "running",
                                                 "thinking": "x".repeat(33) }]);
                    value
                },
                "workflowChildren child thinking is invalid.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["children"] = json!([{ "childId": "a", "state": "running",
                                                 "activity": { "toolCount": -1 } }]);
                    value
                },
                "workflowChildren child activity is invalid.",
            ),
            // Rule 12: an EMPTY activity object is still presence.
            (
                {
                    let mut value = valid_summary_json();
                    value["children"] = json!([{ "childId": "a", "state": "completed",
                                                 "activity": {} }]);
                    value
                },
                "workflowChildren child activity requires running state.",
            ),
            // Rule 13 fires before the id rules even when the parent id is ALSO bad.
            (
                {
                    let mut value = valid_summary_json();
                    value["parentToolCallId"] = json!(7);
                    value["children"] = json!([
                        { "childId": "dup", "state": "running" },
                        { "childId": "dup", "state": "failed" },
                    ]);
                    value
                },
                "workflowChildren has duplicate childId values.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["parentToolCallId"] = json!(7);
                    value
                },
                "workflowChildren.parentToolCallId is invalid.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["workflowRunId"] = json!(null);
                    value
                },
                "workflowChildren.workflowRunId is invalid.",
            ),
            (
                {
                    let mut value = valid_summary_json();
                    value["parentToolCallId"] = json!("   ");
                    value
                },
                "workflowChildren.parentToolCallId must be a non-empty identifier of at most 4096 UTF-8 bytes.",
            ),
        ];
        for (payload, expected) in cases {
            let error = parse_workflow_child_summary(Some(&payload))
                .expect_err("payload must be rejected");
            assert_eq!(error.message(), expected);
        }
    }

    /// `workflowChildProgress`: `agent` is bounded-or-empty, never omitted; the activity is
    /// sanitized (negative and non-finite counters dropped, the bounded tool kept).
    #[test]
    fn progress_and_activity_sanitize_like_upstream() {
        let raw = WorkflowChildActivity {
            current_tool: Bounded::parse("edit"),
            tool_count: Some(serde_json::Number::from(-3i32)),
            turn_count: Some(serde_json::Number::from(2u32)),
            ..WorkflowChildActivity::default()
        };
        let progress = workflow_child_progress(WorkflowChildProgressInput {
            agent: &"a".repeat(300),
            session_name: Some("Sess"),
            model: None,
            thinking: Some("low"),
            activity: &raw,
        });
        assert_eq!(progress.agent, "", "over-bound agent collapses to the empty string");
        assert_eq!(progress.session_name.as_ref().map(Bounded::as_str), Some("Sess"));
        assert_eq!(progress.activity.tool_count, None, "negative counters are dropped");
        assert_eq!(
            progress.activity.turn_count,
            Some(serde_json::Number::from(2u32))
        );
        assert_eq!(
            progress.activity.current_tool.as_ref().map(Bounded::as_str),
            Some("edit")
        );
    }
}
