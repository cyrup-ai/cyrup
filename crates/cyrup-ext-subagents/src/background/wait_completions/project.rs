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
    /// in this module: the projector does not know where the archive will land. Its producer is
    /// [`crate::background::completion_replay::write_completion_replay`], which writes the archive
    /// first and then assigns its path over this field (pi `completion-replay.ts:196`) — so a
    /// completion read back from a replay record carries it and one taken straight off a payload
    /// does not.
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
/// from one) — none is a residual placeholder. Per-child archive references live in the archive's
/// own entries ([`crate::background::completion_replay::CompletionArchiveEntry`]), keyed by
/// `resultIndex`, rather than on this shape.
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
///
/// `pub(crate)` because [`crate::background::completion_replay`]'s archive projector reads the SAME
/// raw payload through the SAME predicate (pi's own `nonEmptyString`,
/// `completion-replay.ts:53-55`, is a second copy of this function upstream) — one implementation,
/// not a third copy.
pub(crate) fn non_empty(value: Option<&serde_json::Value>) -> Option<String> {
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
        // Left unset HERE on purpose: this projector has no `results_dir` and therefore cannot
        // know the archive's address. `write_completion_replay` fills it in afterwards, on the copy
        // it persists and hands back (pi `completion-replay.ts:196`) — see the field's own doc.
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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use serde_json::json;

    fn run(token: &str) -> RunId {
        RunId::from_token(token)
    }

    /// A summary whose `workflowRunId` is `wf-1` — the one shape `to_wait_completion` will accept
    /// only on the run of the same name.
    fn summary_for(workflow_run_id: &str) -> serde_json::Value {
        json!({
            "version": 1,
            "parentToolCallId": "tool-1",
            "workflowRunId": workflow_run_id,
            "inventoryComplete": true,
            "workflowState": "completed",
            "children": [],
        })
    }

    /// pi `:107`. The projected identity is the run the WAIT asked about, never `data.runId` —
    /// otherwise a misfiled payload answers for a run it does not belong to, which is precisely
    /// the confusion the three-rung resolution in `collect.rs` exists to make impossible.
    #[test]
    fn run_id_is_the_runs_own_never_the_payloads() {
        let data = json!({ "runId": "impostor", "agent": "coder" });
        let projected = to_wait_completion(&data, &run("asked-for")).expect("projects");
        assert_eq!(projected.run_id, "asked-for");
        assert_eq!(projected.agent.as_deref(), Some("coder"));
    }

    /// pi `:105`. The ONE place tolerance stops: a summary found on the wrong run is a CORRUPT
    /// inventory, not an absent one, and must propagate rather than be silently dropped.
    #[test]
    fn a_workflow_children_run_id_mismatch_is_reported_not_dropped() {
        let data = json!({ "workflowChildren": summary_for("wf-1") });

        let matched = to_wait_completion(&data, &run("wf-1")).expect("matching id projects");
        assert!(matched.workflow_children.is_some());

        let error = to_wait_completion(&data, &run("wf-2")).expect_err("mismatch is an error");
        assert!(matches!(
            error,
            CompletionProjectionError::WorkflowRunIdMismatch
        ));

        // The other half of the strict seam: a structurally corrupt inventory is rejected by the
        // parser itself, and that rejection is carried, not swallowed.
        let corrupt = json!({ "workflowChildren": { "version": 7 } });
        assert!(matches!(
            to_wait_completion(&corrupt, &run("wf-1")).expect_err("corrupt inventory"),
            CompletionProjectionError::WorkflowChildren(_)
        ));
    }

    /// The tolerance policy, stated as a test: a payload written by a foreign build — wrong types
    /// in every field, `results` not even an array — projects to absence, never to a read failure.
    #[test]
    fn every_field_but_the_inventory_degrades_to_absence() {
        let data = json!({
            "agent": 7,
            "mode": "",
            "state": null,
            "success": "yes",
            "workflowReceipt": "not-an-object",
            "results": "not-an-array",
        });
        let projected = to_wait_completion(&data, &run("r1")).expect("tolerant");
        assert_eq!(
            projected,
            WaitCompletion {
                run_id: "r1".to_string(),
                ..WaitCompletion::default()
            }
        );
    }

    /// `success` is DERIVED from `exitCode`, and an absent `exitCode` omits the key rather than
    /// claiming failure — the distinction `Option<bool>` exists to carry.
    #[test]
    fn child_success_is_derived_from_exit_code_and_absent_without_one() {
        let data = json!({ "results": [
            { "exitCode": 0 },
            { "exitCode": 2 },
            { "agent": "no-exit-code" },
        ] });
        let projected = to_wait_completion(&data, &run("r1")).expect("projects");
        let successes: Vec<Option<bool>> = projected
            .results
            .iter()
            .map(|child| child.success)
            .collect();
        assert_eq!(successes, vec![Some(true), Some(false), None]);
    }

    /// Both spellings of the child's run id are on the wire; cyrup's `childRunId` wins, and pi's
    /// `runId` is the fallback for a payload this build did not write.
    #[test]
    fn child_run_id_prefers_cyrups_spelling_over_pis() {
        let data = json!({ "results": [
            { "childRunId": "cyrup", "runId": "pi" },
            { "runId": "pi-only" },
            { "childRunId": "" , "runId": "pi-because-blank" },
        ] });
        let projected = to_wait_completion(&data, &run("r1")).expect("projects");
        let ids: Vec<Option<&str>> = projected
            .results
            .iter()
            .map(|child| child.run_id.as_deref())
            .collect();
        assert_eq!(
            ids,
            vec![Some("cyrup"), Some("pi-only"), Some("pi-because-blank")]
        );
    }

    /// The wire shape a pi consumer reads: the common case writes NO key for a `false`
    /// `contextOverflow`, an empty `results`, or the `archivePath` this projector cannot know — an
    /// always-omitted key is not a wire change, which is what lets `archive_path` be declared here
    /// and populated only by
    /// [`crate::background::completion_replay::write_completion_replay`].
    #[test]
    fn the_common_case_omits_every_absent_key() {
        let data = json!({ "results": [{ "exitCode": 0 }] });
        let projected = to_wait_completion(&data, &run("r1")).expect("projects");
        let wire = serde_json::to_value(&projected).expect("serializes");

        let keys: Vec<&str> = wire
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["runId", "results"]);
        assert!(!wire.to_string().contains("archivePath"));

        let child = &wire["results"][0];
        let child_keys: Vec<&str> = child
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(child_keys, vec!["success"]);

        // And an empty `results` is omitted entirely rather than written as `[]` (pi `:113`).
        let empty = to_wait_completion(&json!({}), &run("r1")).expect("projects");
        assert_eq!(
            serde_json::to_value(&empty).expect("serializes"),
            json!({ "runId": "r1" })
        );
    }

    /// pi `projectStructuredOutput` (`:47-52`): over the limit the value is OMITTED WHOLE, never
    /// truncated — a truncated JSON value is not a JSON value, and the payload is still on disk.
    /// The measurement is over serialized bytes, so the boundary is exact.
    #[test]
    fn structured_output_over_the_inline_limit_is_omitted_whole() {
        assert_eq!(project_structured_output(None), None);
        assert_eq!(
            project_structured_output(Some(&serde_json::Value::Null)),
            None
        );

        // `{"k":"<pad>"}` is 8 bytes of framing plus the padding.
        let framing = json!({ "k": "" }).to_string().len();
        let fits = json!({ "k": "x".repeat(STRUCTURED_OUTPUT_INLINE_LIMIT_BYTES - framing) });
        assert_eq!(
            serde_json::to_vec(&fits).expect("serializes").len(),
            STRUCTURED_OUTPUT_INLINE_LIMIT_BYTES
        );
        assert_eq!(project_structured_output(Some(&fits)), Some(fits.clone()));

        let over = json!({ "k": "x".repeat(STRUCTURED_OUTPUT_INLINE_LIMIT_BYTES) });
        assert_eq!(project_structured_output(Some(&over)), None);
    }

    /// `cost` is the one field the type system cannot discharge: `f64` can be NaN or negative, and
    /// only a well-formed usage object may reach the wire. `turns` missing is `0`, not a
    /// rejection — a legacy payload legitimately omits it.
    #[test]
    fn child_usage_rejects_an_unusable_cost_and_defaults_turns_to_zero() {
        let usage = |cost: f64, turns: Option<u64>| {
            let mut child = json!({ "usage": {
                "input": 1, "output": 2, "cacheRead": 4, "cacheWrite": 8, "totalTokens": 15,
                "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0,
                          "total": cost },
            } });
            if let Some(turns) = turns {
                child["turns"] = json!(turns);
            }
            project_child_usage(&child)
        };

        let projected = usage(0.5, Some(3)).expect("well-formed");
        assert_eq!(projected.input, 1);
        assert_eq!(projected.output, 2);
        assert_eq!(projected.cache_read, 4);
        assert_eq!(projected.cache_write, 8);
        assert_eq!(projected.cost, 0.5);
        assert_eq!(projected.turns, 3);

        assert_eq!(usage(0.5, None).expect("no turns key").turns, 0);
        assert_eq!(usage(-1.0, Some(1)), None, "a negative cost is unusable");
        assert_eq!(usage(f64::NAN, Some(1)), None, "a NaN cost is unusable");

        // The five other fields need no hand-written predicate: a negative, fractional or absent
        // token count cannot deserialize into `cyrup_core::Usage` at all, so a malformed object
        // yields `None` by the same route upstream's explicit guard reaches. This is the claim
        // `project_child_usage`'s doc makes about the type system discharging five of six checks.
        for malformed in [
            json!({ "usage": { "input": -1 } }),
            json!({ "usage": { "input": 1.5, "output": 2, "cacheRead": 4, "cacheWrite": 8,
                               "totalTokens": 15, "cost": { "input": 0.0, "output": 0.0,
                               "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.5 } } }),
            json!({ "usage": { "input": 1, "output": 2 } }),
            json!({ "usage": "not-an-object" }),
            json!({}),
        ] {
            assert_eq!(project_child_usage(&malformed), None, "{malformed}");
        }
    }

    /// pi `subagent-wait.ts:324`. The all-zeros skip covers all SIX fields, so a wait over
    /// children that reported nothing returns `None` — which omits the key — and never
    /// `Some(zero)`, which would put a misleading zero on the wire.
    #[test]
    fn completion_usage_is_none_when_every_child_reported_zero() {
        let zero = WaitCompletionChild {
            usage: Some(CompletionUsage::default()),
            ..child()
        };
        let completion = WaitCompletion {
            run_id: "r1".to_string(),
            results: vec![
                zero,
                WaitCompletionChild {
                    usage: None,
                    ..child()
                },
            ],
            ..WaitCompletion::default()
        };
        assert_eq!(completion_usage(&[completion]), None);
        assert_eq!(completion_usage(&[]), None);

        // The skip covers all six fields, so a child whose ONLY non-zero field is `turns` is not
        // skipped — it sets `projected` and produces a real, all-zero-token usage. Dropping
        // `turns` from the condition would silently swallow exactly this child, which is why the
        // sixth field is spelled out rather than folded into a "no tokens" shorthand.
        let turns_only = WaitCompletion {
            run_id: "r1".to_string(),
            results: vec![WaitCompletionChild {
                usage: Some(CompletionUsage {
                    turns: 1,
                    ..CompletionUsage::default()
                }),
                ..child()
            }],
            ..WaitCompletion::default()
        };
        let usage = completion_usage(&[turns_only]).expect("a turns-only child still projects");
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.cost.total, 0.0);
    }

    /// `total_tokens` is the FOUR-field sum (`utils.ts:362`), not `input + output`; the summed
    /// dollar figure lands in `cost.total` ONLY, with the components left at zero because a flat
    /// per-child cost has no breakdown to distribute; and `turns` is dropped, surviving per-child.
    #[test]
    fn completion_usage_sums_four_fields_and_keeps_cost_whole() {
        let with = |input, output, cache_read, cache_write, cost, turns| WaitCompletionChild {
            usage: Some(CompletionUsage {
                input,
                output,
                cache_read,
                cache_write,
                cost,
                turns,
            }),
            ..child()
        };
        // Two completions, so the fold is exercised across runs as well as across children.
        let first = WaitCompletion {
            run_id: "r1".to_string(),
            results: vec![with(1, 2, 4, 8, 0.25, 3), with(0, 0, 0, 0, 0.0, 0)],
            ..WaitCompletion::default()
        };
        let second = WaitCompletion {
            run_id: "r2".to_string(),
            results: vec![with(10, 20, 40, 80, 0.75, 5)],
            ..WaitCompletion::default()
        };

        let usage = completion_usage(&[first, second]).expect("some child reported usage");
        assert_eq!(usage.input, 11);
        assert_eq!(usage.output, 22);
        assert_eq!(usage.cache_read, 44);
        assert_eq!(usage.cache_write, 88);
        assert_eq!(usage.total_tokens, 11 + 22 + 44 + 88);
        assert_eq!(usage.cost.total, 1.0);
        assert_eq!(usage.cost.input, 0.0);
        assert_eq!(usage.cost.output, 0.0);
        assert_eq!(usage.cost.cache_read, 0.0);
        assert_eq!(usage.cost.cache_write, 0.0);
        assert_eq!(usage.cache_write_1h, None);
        assert_eq!(usage.reasoning, None);
    }

    /// A child with nothing set — the base every test above varies one field of.
    fn child() -> WaitCompletionChild {
        WaitCompletionChild {
            agent: None,
            usage: None,
            model: None,
            error: None,
            structured_output: None,
            success: None,
            run_id: None,
            session_file: None,
            output_state: None,
            structured_output_path: None,
            context_overflow: false,
            artifact_paths: None,
            timeout_recovery: None,
        }
    }
}
