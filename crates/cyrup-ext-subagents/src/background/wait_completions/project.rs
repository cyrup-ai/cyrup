//! The projector: raw terminal-payload JSON in, the slim wait-facing shapes out.
//!
//! Ports pi `toWaitCompletion` (`wait-completions.ts:60-116`), `projectStructuredOutput`
//! (`:47-52`) and `completionUsage` (`subagent-wait.ts:318-335`, composed with `toAgentToolUsage`,
//! `shared/utils.ts:356-365`).
//!
//! # Why this projects `serde_json::Value`, not a typed `ResultFile`
//!
//! Upstream's own signature takes a raw object (`data: Record<string, unknown>`), not a validated
//! type, and that is deliberate: [`crate::background::ResultFile`] is a VALIDATING type (its
//! `SessionId`/`ResultFileName`/`RunState` fields hard-error on a shape a foreign build might have
//! written), so deserializing straight into it would turn a best-effort projection into a hard read
//! failure. Every reader here therefore takes `&serde_json::Value` and degrades a malformed
//! individual field to absence, except [`to_wait_completion`]'s `workflowChildren`, whose
//! corruption is reported, not hidden — see [`CompletionProjectionError`].

use crate::background::RunId;

/// The inline ceiling for a child's structured output, in SERIALIZED bytes.
///
/// pi `STRUCTURED_OUTPUT_INLINE_LIMIT_BYTES` (`wait-completions.ts:45`).
const STRUCTURED_OUTPUT_INLINE_LIMIT_BYTES: usize = 4 * 1024;

/// pi `WaitCompletion` (`shared/types.ts:1358-1369`) — the slim per-run shape that lands on
/// `details.completions`.
///
/// Output TEXT is deliberately absent: it already travels in the tool-result content, and
/// duplicating it here would double the payload of every wait (upstream's own note,
/// `wait-completions.ts:54-59`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitCompletion {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_receipt_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// A versioned bounded output archive (`shared/types.ts:1366`). Declared HERE, set by nobody
    /// in this module: the field is part of the wire shape a pi consumer reads, and a shape that
    /// gains a key later is a wire change, while a key that is always omitted is not. Its
    /// producer — SCOPE_4's durable completion-replay writer — has not landed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_path: Option<String>,
    /// Omitted when empty, never `[]` — upstream's `results && results.length > 0` (`:113`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<WaitCompletionChild>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_children: Option<crate::workflows::WorkflowChildSummary>,
}

/// pi `WaitCompletionChild` (`shared/types.ts:1335-1352`) — the slim per-child shape that lands
/// inside [`WaitCompletion::results`]. Every optional field mirrors upstream's own
/// `#[serde(skip_serializing_if = "Option::is_none")]` discipline, so a pi-shaped consumer reading
/// `details.completions[].results` sees a byte-identical object.
///
/// Every field here has a real producer on [`crate::exec::SingleResult`] (directly, or derived
/// from one) — none is a residual placeholder waiting on a future task. The one exception,
/// `archivePath`, is declared and left unset for the same reason
/// [`WaitCompletion::archive_path`] is: its producer has not landed yet.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitCompletionChild {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CompletionUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
    /// pi copies `child.success` (`:83`); cyrup's `SingleResult` has no such field, and
    /// `exit_code == 0` is ALREADY this crate's per-child success predicate
    /// (`runner_main/finish.rs:338` folds exactly this over `results`). `Option`, not `bool`: a
    /// payload with no `exitCode` omits the key rather than claiming failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// pi `child.runId` (`:72`), read from cyrup's `childRunId` first — see
    /// [`to_wait_completion`]'s own projection for why the two spellings both have to be tried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_state: Option<crate::exec::output_state::SubagentOutputState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output_path: Option<std::path::PathBuf>,
    /// pi writes the key only when `true` (`:92`); `false` is the common case and must be
    /// omitted, matching [`crate::exec::SingleResult::context_overflow`]'s own discipline.
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub context_overflow: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_paths: Option<crate::artifacts::ArtifactPaths>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_recovery: Option<crate::exec::mutation_evidence::TimeoutRecoveryProjection>,
}

/// pi's subagent-local `Usage` (`shared/types.ts:256-263`) — the shape
/// [`WaitCompletionChild::usage`] puts on the wire.
///
/// Deliberately NOT [`cyrup_core::Usage`]: `details.completions` is a pi-shaped consumer surface,
/// and upstream's own pipeline is two-step — project into this flat six-field shape, then convert
/// to the host tool-result shape via [`completion_usage`] (pi `toAgentToolUsage`,
/// `shared/utils.ts:356-365`). Collapsing the two would change the wire object AND lose `turns`,
/// which the host type cannot hold (`cyrup_core::Usage` has no turn concept, for the reason
/// [`crate::exec::SingleResult::turns`] states at its own declaration).
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub turns: u64,
}

/// Why a projection can fail. Two variants, because the two are acted on differently upstream and
/// a caller that logs them must be able to tell them apart.
#[derive(Debug, thiserror::Error)]
pub enum CompletionProjectionError {
    /// `parse_workflow_child_summary` rejected the inventory — pi's sixteen `throw`s
    /// (`workflow-child-summary.ts:143-170`), messages verbatim. The error type is already
    /// re-exported from `crate::workflows`.
    #[error("{0}")]
    WorkflowChildren(#[from] crate::workflows::WorkflowChildSummaryError),
    /// pi `wait-completions.ts:105`, message verbatim.
    #[error("workflowChildren.workflowRunId does not match its completion run id.")]
    WorkflowRunIdMismatch,
}

/// pi `asNonEmptyString` (`wait-completions.ts:9-11`) — `Some` only for a JSON string that is not
/// empty. One helper; there are eleven call sites across the run- and child-level fields below.
fn non_empty(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Project a terminal result payload into the slim shape that is safe to surface in tool-result
/// details: run identity, per-child outcome, and the artifact trail.
///
/// pi `toWaitCompletion` (`wait-completions.ts:60-116`).
///
/// # Errors
///
/// [`CompletionProjectionError`] — a corrupt `workflowChildren`, or one whose `workflowRunId`
/// disagrees with the run it was found on. Every OTHER malformed field degrades to absence: a
/// foreign payload must not be able to fail a status read.
pub fn to_wait_completion(
    data: &serde_json::Value,
    run_id: &RunId,
) -> Result<WaitCompletion, CompletionProjectionError> {
    let workflow_children =
        crate::workflows::parse_workflow_child_summary(data.get("workflowChildren"))?;
    if let Some(summary) = &workflow_children {
        // pi `:105` — a summary whose `workflowRunId` disagrees with the run it was found on is a
        // CORRUPT inventory, not an absent one; propagate rather than silently drop it.
        if summary.workflow_run_id.as_str() != run_id.as_str() {
            return Err(CompletionProjectionError::WorkflowRunIdMismatch);
        }
    }

    let results = data
        .get("results")
        .and_then(serde_json::Value::as_array)
        .map(|children| children.iter().map(project_child).collect())
        .unwrap_or_default();

    Ok(WaitCompletion {
        // pi `:107` — the run this WAIT asked about, never `data.runId`: trusting the payload
        // would let a misfiled result answer for a different run.
        run_id: run_id.as_str().to_string(),
        agent: non_empty(data.get("agent")),
        mode: non_empty(data.get("mode")),
        workflow_receipt_path: non_empty(
            data.get("workflowReceipt")
                .and_then(|receipt| receipt.get("path")),
        ),
        state: non_empty(data.get("state")),
        success: data.get("success").and_then(serde_json::Value::as_bool),
        // Left unset until SCOPE_4's producer (a durable completion-replay writer) lands — see
        // the field's own doc.
        archive_path: None,
        results,
        workflow_children,
    })
}

/// Project one raw child object into [`WaitCompletionChild`], read off the raw **camelCase** wire
/// key each field serializes as on [`crate::exec::SingleResult`]
/// (`#[serde(rename_all = "camelCase")]`).
fn project_child(child: &serde_json::Value) -> WaitCompletionChild {
    WaitCompletionChild {
        agent: non_empty(child.get("agent")),
        usage: project_child_usage(child),
        model: non_empty(child.get("model")),
        error: non_empty(child.get("error")),
        structured_output: project_structured_output(child.get("structuredOutput")),
        success: child
            .get("exitCode")
            .and_then(serde_json::Value::as_i64)
            .map(|code| code == 0),
        // pi reads `child.runId` (`:72`); cyrup's field serializes as `childRunId` (named that way
        // so it cannot collide with the run-LEVEL id) — read that first and fall back to `runId`,
        // exactly as `result_index::locate::payload_run_id` accepts both spellings of the run id
        // for the same "the wire carries it twice" reason.
        run_id: non_empty(child.get("childRunId")).or_else(|| non_empty(child.get("runId"))),
        session_file: non_empty(child.get("sessionFile")).map(std::path::PathBuf::from),
        output_state: child
            .get("outputState")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        structured_output_path: non_empty(child.get("structuredOutputPath"))
            .map(std::path::PathBuf::from),
        context_overflow: child
            .get("contextOverflow")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        artifact_paths: child
            .get("artifactPaths")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        timeout_recovery: crate::exec::mutation_evidence::project_timeout_recovery(
            child.get("timeoutRecovery"),
        ),
    }
}

/// Project one child's `usage` + `turns` pair into the wire [`CompletionUsage`] shape — pi's
/// `projectedUsage` (`wait-completions.ts:17-28`).
///
/// Upstream's "reject unless all six are finite, non-negative, and `turns` is a safe integer" is
/// discharged by the type system for five of the six fields: deserializing into `u64`/
/// [`cyrup_core::Usage`] cannot yield a negative, fractional, NaN, or unsafely large value, so a
/// malformed `usage` object simply fails to deserialize and this returns `None` — upstream's own
/// outcome for the same input. `cost` is the one field that still needs a runtime check: `f64`
/// CAN be NaN or negative, and only a well-formed `usage` object should ever reach
/// [`CompletionUsage`]. `turns` missing is `0`, not a rejection — a zero-turn or legacy payload
/// legitimately omits the key.
fn project_child_usage(child: &serde_json::Value) -> Option<CompletionUsage> {
    let usage: cyrup_core::Usage = serde_json::from_value(child.get("usage")?.clone()).ok()?;
    if !usage.cost.total.is_finite() || usage.cost.total < 0.0 {
        return None;
    }
    let turns = child
        .get("turns")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Some(CompletionUsage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        cost: usage.cost.total,
        turns,
    })
}

/// pi `projectStructuredOutput` (`wait-completions.ts:47-52`).
///
/// Structured output above [`STRUCTURED_OUTPUT_INLINE_LIMIT_BYTES`] is **omitted entirely**, not
/// truncated: a truncated JSON value is not a JSON value, and the payload it came from is still on
/// disk — named by `structuredOutputPath` (`exec/run_result.rs`) — for a caller that needs it. The
/// measurement is over the SERIALIZED bytes (`Buffer.byteLength(JSON.stringify(value), "utf8")`),
/// not any in-memory size, so [`serde_json::to_vec`] is the faithful probe and `String::len` on a
/// pretty-print is not.
///
/// Upstream `throw`s when the value is not JSON-serializable (`:50`); a [`serde_json::Value`]
/// always is, so that branch is unreachable here and `.ok()?` covers the impossible case.
#[must_use]
pub fn project_structured_output(value: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let value = value?;
    if value.is_null() {
        return None; // pi's `value === undefined` early return (`:48`), for a JSON null
    }
    let bytes = serde_json::to_vec(value).ok()?;
    (bytes.len() <= STRUCTURED_OUTPUT_INLINE_LIMIT_BYTES).then(|| value.clone())
}

/// pi `completionUsage` (`subagent-wait.ts:318-335`) composed with `toAgentToolUsage`
/// (`shared/utils.ts:356-365`) — the summed child usage that lands on the wait's tool-result
/// `usage` field.
///
/// # The two rules that are easy to get wrong
///
/// * **The all-zeros skip** (`subagent-wait.ts:324`) covers all SIX fields including `turns` and
///   `cost`. A child with a zero usage contributes nothing AND does not set `projected`, which is
///   what makes the return `None` — not `Some(zero)` — for a wait over children that reported no
///   usage. `None` omits the key; `Some(zero)` would put a misleading zero on the wire.
/// * **`total_tokens` is `input + output + cache_read + cache_write`** — upstream's own line
///   (`utils.ts:362`), NOT `input + output`. And the nested [`cyrup_core::Cost`] carries the
///   summed dollar figure in `total` ONLY, with the four component costs left at `0.0`
///   (`utils.ts:363`): the flat per-child `cost` upstream sums has no component breakdown to
///   distribute, and inventing one would fabricate accounting. `reasoning` and `cache_write_1h`
///   stay `None` for the same reason.
///
/// `turns` has nowhere to go in [`cyrup_core::Usage`] and is dropped here exactly as upstream
/// drops it — it survives per-child in [`CompletionUsage::turns`], which is where a consumer that
/// wants it looks.
#[must_use]
pub fn completion_usage(completions: &[WaitCompletion]) -> Option<cyrup_core::Usage> {
    let mut input = 0u64;
    let mut output = 0u64;
    let mut cache_read = 0u64;
    let mut cache_write = 0u64;
    let mut cost = 0.0f64;
    let mut projected = false;

    for child in completions
        .iter()
        .flat_map(|completion| &completion.results)
    {
        let Some(usage) = &child.usage else {
            continue;
        };
        // pi `subagent-wait.ts:324` — the all-zeros skip covers all SIX fields, `turns` included;
        // a child that reported nothing must not flip `projected`.
        if usage.input == 0
            && usage.output == 0
            && usage.cache_read == 0
            && usage.cache_write == 0
            && usage.cost == 0.0
            && usage.turns == 0
        {
            continue;
        }
        // The same five-line fold as `CostUsage::add_usage` (`registration/cost.rs:118-124`).
        input += usage.input;
        output += usage.output;
        cache_read += usage.cache_read;
        cache_write += usage.cache_write;
        cost += usage.cost;
        projected = true;
    }

    if !projected {
        return None;
    }

    Some(cyrup_core::Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output + cache_read + cache_write,
        cost: cyrup_core::Cost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: cost,
        },
    })
}
