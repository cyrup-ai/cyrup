//! The mission binding glue that ties a dispatched run to a durable mission record.

use std::path::{Path, PathBuf};

use cyrup_core::{ToolError, ToolResult};

/// The [`crate::background::watch::CompletionObserver`] that runs pi's
/// `syncMissionFromAsyncCompletion` (`extension/index.ts:655`) for every completed background run.
///
/// The observer receives a [`crate::background::watch::CompletionNotification`] — a parsed
/// [`crate::background::ResultFile`] — and has to rebuild the JSON event shape
/// `syncMissionFromAsyncCompletion` reads. That shape is upstream's `SUBAGENT_ASYNC_COMPLETE_EVENT`
/// payload; the two fields it cannot take from the result file are `asyncDir` (derived from the
/// async root plus the run id — the same [`crate::background::RunDir`] arithmetic the runner used
/// to create it) and `summary` (which cyrup's `ResultFile` does not carry as a distinct field, so
/// it is omitted rather than guessed at).
pub(crate) struct MissionSyncCompletionObserver {
    /// This session's async root, so a completed run's `asyncDir` — and therefore its
    /// `mission.json` binding — can be located from the run id alone.
    pub(crate) async_root: PathBuf,
}

#[async_trait::async_trait]
impl crate::background::watch::CompletionObserver for MissionSyncCompletionObserver {
    async fn observe(&self, notification: &crate::background::watch::CompletionNotification) {
        let result = &notification.result;
        let async_dir = crate::background::RunDir::new(&self.async_root, &result.run_id);
        let event = serde_json::json!({
            "runId": result.run_id.as_str(),
            "asyncDir": async_dir.as_path().to_string_lossy(),
            // `RunState`/`RunMode` are `#[serde(rename_all = "camelCase")]`, so these serialize
            // to the same `"complete"`/`"single"` strings pi's own event payload carries and
            // `missionStatusForRun`/`syncMissionFromAsyncCompletion` branch on.
            "state": result.state,
            "success": result.success,
            "mode": result.mode,
            "results": result.results,
        });
        // pi wraps this call in `try { … } catch { console.error(...) }` (`:654-658`) — mission
        // bookkeeping is never allowed to disturb the completion pipeline.
        if let Err(e) = crate::missions::sync_mission_from_async_completion(&event) {
            tracing::warn!("Failed to update mission from async completion: {e}");
        }
    }
}

/// pi's pre-launch mission resolution (`subagent-executor.ts:5101-5111` @v0.43.0) — the LAUNCH half
/// of the binding pair whose settle half is [`attach_mission_to_tool_outcome`].
///
/// Returns `(binding, warning)`. Upstream's error discipline, verbatim:
///
/// * an EXPLICIT `mission`/`missionId` makes a failure fatal to the whole call (`:5109` returns
///   `toExecutionErrorResult`, cyrup's equivalent being `Err(ToolError)`) — the caller asked for
///   mission tracking and did not get it;
/// * an AUTOMATIC binding degrades to a non-fatal `Mission tracking unavailable: <e>` warning
///   (`:5110`) that [`attach_mission_to_tool_outcome`] later stamps onto `details.missionWarning`,
///   so bookkeeping can never take down a run that would otherwise have succeeded.
///
/// Split out of `execute_dispatch` so the degradation is reachable from a test without driving a
/// real subagent subprocess; `execute_dispatch` is its only production caller.
///
/// # Errors
///
/// [`ToolError`] carrying the mission error's own message, and only when `explicit_mission`.
pub(crate) fn prepare_mission_binding_for_dispatch(
    params: &crate::missions::MissionLaunchParams,
    cwd: &Path,
    config: Option<&crate::missions::MissionStoreConfig>,
    owner_session_id: Option<&str>,
    explicit_mission: bool,
) -> Result<
    (
        Option<crate::missions::MissionLaunchBinding>,
        Option<String>,
    ),
    ToolError,
> {
    match crate::missions::prepare_mission_launch(params, cwd, config, owner_session_id) {
        Ok(binding) => Ok((binding, None)),
        Err(e) if explicit_mission => Err(ToolError::new(e.to_string())),
        Err(e) => {
            // Never silent — an auto-created mission that could not be written is invisible to the
            // model beyond one `details` key, so the operator gets the reason in the log.
            tracing::warn!(error = %e, "mission tracking unavailable; continuing without a binding");
            Ok((None, Some(format!("Mission tracking unavailable: {e}"))))
        }
    }
}

/// pi's `attachMission` closure (`subagent-executor.ts:5112-5133` @v0.43.0) — the settle half of
/// the mission launch binding, adapted to cyrup's `Result<ToolResult, ToolError>` error channel.
///
/// [CYRUP-DELTA] upstream's `AgentToolResult` carries `isError` alongside `content`/`details`, so
/// one value covers both outcomes and `attachMissionToLaunchResult` runs on it either way. cyrup
/// surfaces a failed tool call as `Err(ToolError)`, which carries TEXT ONLY — no `details`. So the
/// `Err` arm is converted into a [`crate::missions::LaunchOutcome`] with `is_error: true`, its
/// message as the single text part and no `details`; that reaches
/// `attachMissionToLaunchResult`'s no-run-id branch, which is exactly where upstream's own
/// `isError` results without a run id land (`missions/lifecycle.ts:171-190`) — the mission is
/// marked `failed` (unless another run is still live) and takes the error text as its summary. An
/// `Err` therefore stays an `Err`: the mission is updated, the failure is not swallowed.
pub(crate) fn attach_mission_to_tool_outcome(
    outcome: Result<ToolResult, ToolError>,
    binding: Option<&crate::missions::MissionLaunchBinding>,
    mission_warning: Option<String>,
    explicit_mission: bool,
) -> Result<ToolResult, ToolError> {
    /// `{ ...result.details, missionWarning }` — pi's non-fatal degradation
    /// (`subagent-executor.ts:5114`).
    fn with_warning(mut result: ToolResult, warning: &str) -> ToolResult {
        // `{ ...result.details, missionWarning }`: spreading an absent (or non-object) `details`
        // yields `{}` in JS, so a fresh object is the faithful base — the previous value is not
        // smuggled under a nested key.
        let mut details = match result.details.take() {
            Some(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        details.insert(
            "missionWarning".to_string(),
            serde_json::Value::String(warning.to_string()),
        );
        result.details = Some(serde_json::Value::Object(details));
        result
    }

    let Some(binding) = binding else {
        return match (outcome, mission_warning) {
            (Ok(result), Some(warning)) => Ok(with_warning(result, &warning)),
            (outcome, _) => outcome,
        };
    };

    let (is_error, content, details) = match &outcome {
        Ok(result) => (false, result.content.clone(), result.details.clone()),
        Err(err) => (true, vec![cyrup_core::Content::text(err.to_string())], None),
    };
    let attached = crate::missions::attach_mission_to_launch_result(
        binding,
        crate::missions::LaunchOutcome {
            content,
            details,
            is_error,
        },
    );
    match attached {
        Ok(attached) => match outcome {
            // The ERROR arm keeps its error identity: only the mission bookkeeping ran, and the
            // announcement/`details` stamp has nowhere to live on a `ToolError`.
            Err(err) => Err(err),
            Ok(result) => Ok(ToolResult {
                content: attached.content,
                details: attached.details,
                ..result
            }),
        },
        Err(e) => {
            let warning = format!("Mission tracking unavailable after launch: {e}");
            // Never silent: the run itself succeeded and only its bookkeeping did not, which is a
            // condition an operator has to be able to see in the log even when the model is told
            // about it in prose.
            tracing::warn!(error = %e, "mission bookkeeping failed after launch");
            match outcome {
                Err(err) => Err(err),
                // pi `:5119-5127`: BOTH arms are `{ ...result, … }` — the settled run's content and
                // `details` survive a post-launch bookkeeping failure in either case. The EXPLICIT
                // arm additionally makes the warning MODEL-VISIBLE by appending it as a new text
                // part (`content: [...result.content, { type: "text", text: warning }]`) and sets
                // `isError: true`; the automatic arm records it on `details` only.
                //
                // [CYRUP-DELTA] `cyrup_core::ToolResult` has no `isError` field — this crate's
                // error-result channel is `Err(ToolError)`, and `ToolError` is a bare `String`.
                // Returning one here is what this arm used to do, and it DISCARDED THE ENTIRE RUN:
                // `details` (the `runId`, every settled `results[]` entry, the artifact paths, the
                // mission stamp) and every non-text content part of a subagent run that had ALREADY
                // FINISHED were thrown away to signal a bookkeeping failure upstream treats as
                // non-destructive — `attachMissionToLaunchResult` throwing means the mission store
                // could not be written, not that the work was lost. The result is kept and the
                // warning is carried in both of the places upstream carries it; the one thing that
                // cannot cross is the `isError` bit, whose only cyrup encoding costs the payload.
                Ok(mut result) if explicit_mission => {
                    result
                        .content
                        .push(cyrup_core::Content::text(warning.clone()));
                    Ok(with_warning(result, &warning))
                }
                Ok(result) => Ok(with_warning(result, &warning)),
            }
        }
    }
}

/// pi `duplicateSubagentCallResult` (`subagent-executor.ts:3882-3891`)'s content text, verbatim.
/// (pi also attaches `details: { mode: inferExecutionMode(params), results: [] }`; this crate's
/// `ToolError` carries no `details` channel, matching every other `isError: true` -> `Err`
/// translation in this file — R-02-024.)
pub(crate) fn duplicate_subagent_call_text() -> &'static str {
    "Rejected: a subagent call is already in progress. Issue exactly ONE subagent call per turn."
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
    use crate::extension::testsupport::scoped_mission_config;
    use crate::extension::testsupport::scoped_missions;

    /// A background run's terminal result file reconciles its bound mission through the completion
    /// watcher's observer (pi `extension/index.ts:655`), in a process that never launched it.
    #[tokio::test]
    async fn a_completed_background_run_reconciles_its_mission_from_the_result_file() {
        use crate::background::watch::CompletionObserver as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = dir.path().join("async");
        let location = crate::missions::resolve_mission_store_location(
            dir.path(),
            Some(&scoped_missions(dir.path())),
            None,
        );
        let binding = crate::missions::prepare_mission_launch(
            &crate::missions::MissionLaunchParams {
                task: Some("run it in the background".to_string()),
                ..Default::default()
            },
            dir.path(),
            Some(&scoped_missions(dir.path())),
            Some("sess"),
        )
        .expect("prepare")
        .expect("a task-bearing launch binds a mission");
        let run_id = crate::background::RunId::from_token("bgrun000001");
        let run_dir = crate::background::RunDir::new(&async_root, &run_id);
        std::fs::create_dir_all(run_dir.as_path()).expect("mkdir");
        // The binding file the launch would have written into the async dir.
        crate::missions::attach_mission_to_launch_result(
            &binding,
            crate::missions::LaunchOutcome {
                content: vec![cyrup_core::Content::text("started")],
                details: Some(serde_json::json!({
                    "mode": "single",
                    "asyncId": run_id.as_str(),
                    "asyncDir": run_dir.as_path().to_string_lossy(),
                    "results": [],
                })),
                is_error: false,
            },
        )
        .expect("attach");
        assert_eq!(
            crate::missions::read_mission(&location, &binding.mission_id)
                .expect("read")
                .status,
            crate::missions::MissionStatus::Active
        );

        let observer = MissionSyncCompletionObserver {
            async_root: async_root.clone(),
        };
        observer
            .observe(&crate::background::watch::CompletionNotification {
                result: crate::background::ResultFile {
                    id: run_id.clone(),
                    run_id: run_id.clone(),
                    agent: "scout".to_string(),
                    mode: crate::background::RunMode::Single,
                    state: crate::background::RunState::Complete,
                    success: true,
                    cwd: dir.path().to_path_buf(),
                    session_file: None,
                    results: Vec::new(),
                },
                result_path: dir.path().join("results").join("bgrun000001.json"),
                exhausted: false,
            })
            .await;

        let reconciled =
            crate::missions::read_mission(&location, &binding.mission_id).expect("read");
        assert_eq!(reconciled.status, crate::missions::MissionStatus::Completed);
        assert_eq!(reconciled.runs.len(), 1);
        assert_eq!(reconciled.runs[0].status.as_deref(), Some("complete"));
        assert!(reconciled.runs[0].completed_at.is_some());
    }

    /// A mission store whose `missions` directory path is occupied by a FILE, so every write into
    /// it fails — the cheapest real (not mocked) way to make the store refuse.
    fn wedged_mission_root(root: &Path) -> crate::missions::MissionStoreConfig {
        let mission_dir = root.join("wedged").join("missions");
        std::fs::create_dir_all(mission_dir.parent().expect("parent")).expect("mkdir");
        std::fs::write(&mission_dir, b"not a directory").expect("write");
        crate::missions::MissionStoreConfig {
            directory: Some(mission_dir.to_string_lossy().into_owned()),
            ..scoped_mission_config(root)
        }
    }

    /// pi `:5110`. An AUTOMATIC binding (no `mission`/`missionId` in the call) whose store cannot
    /// be written degrades to a warning and a `None` binding — the run still goes ahead.
    #[test]
    fn a_pre_launch_mission_failure_degrades_to_a_warning_when_the_mission_is_automatic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = wedged_mission_root(dir.path());
        let params = crate::missions::MissionLaunchParams {
            task: Some("do the thing".to_string()),
            ..Default::default()
        };
        let (binding, warning) = prepare_mission_binding_for_dispatch(
            &params,
            dir.path(),
            Some(&config),
            Some("sess"),
            false,
        )
        .expect("an automatic mission never fails the call");
        assert!(
            binding.is_none(),
            "no binding survives a store that cannot be written"
        );
        let warning = warning.expect("the degradation must record why");
        assert!(
            warning.starts_with("Mission tracking unavailable: "),
            "pi's own prefix, verbatim: {warning}"
        );

        // …and the warning really reaches the model-facing result, as `details.missionWarning`.
        let result = attach_mission_to_tool_outcome(
            Ok(ToolResult {
                content: vec![cyrup_core::Content::text("child said hi")],
                details: Some(serde_json::json!({"mode": "single", "runId": "r1"})),
                ..Default::default()
            }),
            binding.as_ref(),
            Some(warning.clone()),
            false,
        )
        .expect("an automatic mission never turns a good run into an error");
        let details = result.details.expect("details survive");
        assert_eq!(
            details.get("missionWarning").and_then(|v| v.as_str()),
            Some(warning.as_str())
        );
        assert_eq!(details.get("runId").and_then(|v| v.as_str()), Some("r1"));
    }

    /// pi `:5109`. An EXPLICIT `mission`/`missionId` makes the same failure fatal to the call.
    #[test]
    fn a_pre_launch_mission_failure_is_fatal_when_the_mission_is_explicit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = wedged_mission_root(dir.path());
        let params = crate::missions::MissionLaunchParams {
            task: Some("do the thing".to_string()),
            mission: Some(serde_json::json!({"title": "Explicit"})),
            ..Default::default()
        };
        let err = prepare_mission_binding_for_dispatch(
            &params,
            dir.path(),
            Some(&config),
            Some("sess"),
            true,
        )
        .expect_err("an explicit mission that cannot be tracked fails the call");
        assert!(
            !err.to_string().is_empty(),
            "the store's own message is what surfaces, not a generic one"
        );
        // Upstream's `toExecutionErrorResult(…, error, …)` carries the RAW error, without the
        // `Mission tracking unavailable:` prefix that belongs to the degradation arm alone.
        assert!(
            !err.to_string().starts_with("Mission tracking unavailable"),
            "{err}"
        );
    }

    /// A binding whose record is destroyed between launch and settle, so
    /// `attach_mission_to_launch_result` really fails — the post-launch degradation's only trigger.
    fn binding_with_a_corrupt_record(
        root: &Path,
    ) -> (
        crate::missions::MissionLaunchBinding,
        crate::missions::MissionStoreConfig,
    ) {
        let config = scoped_mission_config(root);
        let binding = crate::missions::prepare_mission_launch(
            &crate::missions::MissionLaunchParams {
                task: Some("settle me".to_string()),
                ..Default::default()
            },
            root,
            Some(&config),
            Some("sess"),
        )
        .expect("prepare")
        .expect("a task-bearing launch binds a mission");
        let record = crate::missions::mission_record_path(&binding.location, &binding.mission_id)
            .expect("record path");
        std::fs::write(&record, b"{ this is not json").expect("corrupt the record");
        (binding, config)
    }

    /// pi `:5127`. An AUTOMATIC mission whose POST-launch bookkeeping fails keeps the whole result
    /// and records the warning on `details`.
    #[test]
    fn a_post_launch_mission_failure_keeps_the_result_when_the_mission_is_automatic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (binding, _config) = binding_with_a_corrupt_record(dir.path());
        let result = attach_mission_to_tool_outcome(
            Ok(ToolResult {
                content: vec![cyrup_core::Content::text("the child's answer")],
                details: Some(serde_json::json!({"mode": "single", "runId": "r9", "results": []})),
                terminate: cyrup_core::TerminateHint::Terminate,
                ..Default::default()
            }),
            Some(&binding),
            None,
            false,
        )
        .expect("bookkeeping failure must not fail the run");
        assert_eq!(
            result.content.len(),
            1,
            "no warning part on the automatic arm"
        );
        let details = result.details.expect("details survive");
        assert!(
            details
                .get("missionWarning")
                .and_then(|v| v.as_str())
                .is_some_and(|w| w.starts_with("Mission tracking unavailable after launch: ")),
            "{details}"
        );
        assert_eq!(details.get("runId").and_then(|v| v.as_str()), Some("r9"));
        assert!(
            result.terminate.requested(),
            "every other ToolResult field survives too"
        );
    }

    /// pi `:5119-5126`. An EXPLICIT mission's post-launch failure appends the warning as a NEW text
    /// part — and, the actual defect this pins, KEEPS the settled run.
    ///
    /// Pre-fix this arm returned `Err(ToolError::new(<joined text parts>))`, which discarded
    /// `details` (run id, `results[]`, artifact paths), every non-text content part, and every
    /// other `ToolResult` field of a subagent run that had already finished — for a mission-store
    /// write failure upstream treats as non-destructive (`{ ...result, isError: true }`).
    #[test]
    fn a_post_launch_mission_failure_keeps_the_result_when_the_mission_is_explicit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (binding, _config) = binding_with_a_corrupt_record(dir.path());
        let image = cyrup_core::Content::Image {
            data: "AAAA".to_string(),
            mime_type: "image/png".to_string(),
        };
        let result = attach_mission_to_tool_outcome(
            Ok(ToolResult {
                content: vec![
                    cyrup_core::Content::text("the child's answer"),
                    image.clone(),
                ],
                details: Some(serde_json::json!({
                    "mode": "single", "runId": "r9",
                    "results": [{"agent": "scout", "exitCode": 0}],
                })),
                terminate: cyrup_core::TerminateHint::Terminate,
                ..Default::default()
            }),
            Some(&binding),
            None,
            true,
        )
        .expect("a finished run is never discarded to signal a bookkeeping failure");

        // The warning is model-visible, appended AFTER the run's own content (upstream's
        // `content: [...result.content, { type: "text", text: warning }]`).
        assert_eq!(result.content.len(), 3);
        assert_eq!(
            result.content[1], image,
            "a non-text part is carried through untouched"
        );
        let warning = match &result.content[2] {
            cyrup_core::Content::Text { text, .. } => text.to_string(),
            other => panic!("{other:?}"),
        };
        assert!(
            warning.starts_with("Mission tracking unavailable after launch: "),
            "{warning}"
        );

        // …and none of the run is lost.
        let details = result.details.expect("details survive");
        assert_eq!(details.get("runId").and_then(|v| v.as_str()), Some("r9"));
        assert_eq!(
            details
                .get("results")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(1),
            "the settled per-child results survive: {details}"
        );
        assert_eq!(
            details.get("missionWarning").and_then(|v| v.as_str()),
            Some(warning.as_str())
        );
        assert!(result.terminate.requested());
    }

    /// The same post-launch failure on an outcome that was ALREADY an error stays an error, with no
    /// warning smuggled onto a channel that cannot carry it.
    #[test]
    fn a_post_launch_mission_failure_leaves_an_error_outcome_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (binding, _config) = binding_with_a_corrupt_record(dir.path());
        let err = attach_mission_to_tool_outcome(
            Err(ToolError::new("the child failed")),
            Some(&binding),
            None,
            true,
        )
        .expect_err("an error outcome stays an error");
        assert_eq!(err.to_string(), "the child failed");
    }
}
