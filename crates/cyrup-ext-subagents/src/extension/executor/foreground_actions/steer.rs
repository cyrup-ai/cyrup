//! `action: "steer"` (G90) — queue NON-TERMINAL guidance for a still-live background child. Ports
//! `runs/foreground/async-steering-action.ts` (246 LOC, the largest of the three WORKFLOW_8
//! verbs). Moved out of `extension::executor::control` verbatim, plus `:162`'s steering notice
//! (WORKFLOW_8 SUBTASK2) — see [`super`] for the scope note shared by every verb in this
//! directory.
//!
//! # ⚠ NAMING TRAP — this is not a foreground steer
//!
//! `foreground_actions/` names pi's **source directory** (`src/runs/foreground/`), not the kind of
//! run being acted on. [`SubagentExecutor::control_steer`] is `asyncDir`-gated (`:45`/`:52`),
//! reconciles at `:71` and delivers through `requestAsyncSteer` at `:149` — it acts on an **async**
//! run. A run with no directory cannot enter it.
//!
//! Steering a live **foreground** child is WORKFLOW_14, created because this file's own name made
//! it look owned when it was not — do not close that gap here.
//!
//! # What is NOT here: the steering-recovery subsystem
//!
//! `async-steering-action.ts`'s bulk (`:88-250`) is a steering-RECOVERY subsystem —
//! `status.steering`, `createSteeringStatus`, `recordSteeringRequest`, `updateSteeringTarget`,
//! `claimSteeringRecovery`, `remainingSteeringRecoveryLimits`, `queueRevivalBrief`,
//! `actionResultFromSteeringStatus`, `waitForSteeringAction` — none of which exist in cyrup. cyrup
//! took a different, already-landed return path (SUBA-049): per-child ack files under
//! `<run_dir>/control/steer-acks/<index>/`, drained by
//! [`crate::background::control::take_steer_acks`] and narrowed by [`SubagentExecutor::await_steer_ack`].
//!
//! Do not port the recovery subsystem here — it is a different, much larger port that would
//! duplicate SUBA-049's design, and its interrupt-then-revive-a-replacement behaviour is separate,
//! not-yet-owned work. This is also why [`SteeringNoticeState::Recovered`] has no producer in this
//! build: upstream emits it only from that recovery block.

use std::path::Path;

use crate::background::control;
use crate::background::run_status;
use crate::background::{RunPaths, RunState, RunStatus, StepState};
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::extension::tool::text::{
    STEER_ACK_POLL_INTERVAL, STEER_ACK_TIMEOUT, STEER_FOREGROUND_RUN_REFUSAL,
};
use crate::identity::SessionId;

impl SubagentExecutor {
    /// G90 — `action: "steer"` (pi `subagent-executor.ts:570-626,3194-3220` @v0.34.0): queue
    /// NON-TERMINAL guidance for a still-live background child.
    ///
    /// This is deliberately NOT [`Self::control_resume`]. `resume` interrupts the child first and
    /// then delivers a follow-up (or revives a finished one from its transcript); `steer` never
    /// interrupts and never respawns — it drops a request into the run's control inbox
    /// ([`crate::background::control::request_async_steer`]) and the runner hands it to the running
    /// child. That is why the two verbs coexist upstream, and why the confirmation text below says
    /// "queued": the parent's job ends at the inbox.
    ///
    /// pi's guards, in pi's order:
    ///
    /// * `message` is required, falling back to `task`, and must be non-blank (`:3195-3196`);
    /// * `id` or `dir` is required (`:3208`);
    /// * the run must reconcile to `Running` or `Queued` (`:585-590`);
    /// * an explicit `index` must be in range (`:592-598`) and must name a child that is `running`
    ///   or `pending` (`:600-607`);
    /// * with no `index`, a multi-child run with nothing running yet is refused with pi's
    ///   "Provide index to steer a queued child." (`:609-617`).
    ///
    /// # Errors
    ///
    /// Returns each of the above refusals, or a resolution/reconciliation/write failure, as `Err`.
    // SUBA-049 raised this from 7 to 8 arguments by adding `mode`. The alternative — an options
    // struct — would be a cyrup-original shape for a function whose whole contract is pi's
    // `steerAsyncRun` input, and this is the crate's established treatment for exactly that
    // (`build_attempt_spawn_plan_with_read_requirement` carries the same allowance for the same
    // reason).
    #[allow(clippy::too_many_arguments)]
    pub async fn control_steer(
        &self,
        cwd: &Path,
        target: Option<&str>,
        dir: Option<&str>,
        message: Option<&str>,
        task: Option<&str>,
        index: Option<usize>,
        mode: Option<&str>,
    ) -> Result<String, String> {
        // SUBA-049 — pi `mode` (`extension/schemas.ts:283` @v0.43.0), validated HERE rather than by
        // serde so an unrecognised value is a sentence the model can act on. Upstream's schema
        // `enum` does the rejecting there; cyrup's schema carries the same enum, but a tool call
        // that bypasses schema validation must still be refused rather than silently defaulted to
        // the INTERRUPTING mode — quietly upgrading `follow_up` to `steer` is the worst available
        // failure for this parameter.
        let mode = match mode.map(str::trim).filter(|m| !m.is_empty()) {
            None => None,
            Some(raw) => Some(control::SteerDeliveryMode::parse(raw).ok_or_else(|| {
                format!("Unknown steer mode '{raw}'. Valid: steer, follow_up, auto.")
            })?),
        };
        let message = message
            .or(task)
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| "action='steer' requires message.".to_string())?;
        if target.is_none() && dir.is_none() {
            return Err("action='steer' requires id or dir.".to_string());
        }
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        let (status, paths) = match (target, dir) {
            (Some(id), None) => {
                // WORKFLOW_7 — pi `steerWorkflowRun`'s already-live-workflow-controller branch
                // (`workflow-foreground-steering.ts:87-90`): an id this process is DRIVING as a
                // workflow routes to the workflow resolver and never touches the async store.
                // Ahead of the foreground refusal below, which was written when a workflow-owned
                // foreground child had no route at all. Exact-match only (`live_workflow_run_id_for`'s
                // own doc) — a prefix match here would risk routing at the wrong workflow.
                if let Some(workflow_run_id) = self.live_workflow_run_id_for(id) {
                    return self
                        .steer_workflow_foreground(
                            &workflow_run_id,
                            message,
                            mode,
                            index,
                            &async_root,
                        )
                        .await;
                }
                // pi classifies an id-addressed selector with `resolveSubagentRunId` BEFORE it
                // touches the async store (`subagent-executor.ts:3211` @v0.34.0) and branches on
                // all three outcomes, with a DISTINCT refusal for each. cyrup collapsed two of
                // them into the `steerAsyncRun` "no live run directory" text, which told a caller
                // who had named a live FOREGROUND run — or a typo — that an async run existed but
                // had lost its directory. Both are restored here, in pi's own order.
                if self.is_live_foreground_run(id) {
                    // pi `:3217`, rebranded (`Pi child sessions` → `Cyrup child sessions`,
                    // matching this crate's standing rebrand of pi's user-facing product noun —
                    // see `control_steer`'s own success text).
                    return Err(STEER_FOREGROUND_RUN_REFUSAL.to_string());
                }
                // pi `:3218`: the selector resolved to NOTHING — neither a foreground run nor an
                // async one. Distinct from "resolved, but its run dir is gone", which the shared
                // `ok_or_else` below still reports with `steerAsyncRun`'s own text (pi `:3580`).
                if run_status::resolve_run_id(&async_root, &results_dir, id)
                    .await
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    return Err(format!("No async run found for '{id}'."));
                }
                run_status::reconcile_by_id(&async_root, &results_dir, id).await
            }
            (_, Some(dir)) => run_status::reconcile_by_dir(Path::new(dir), &results_dir).await,
            (None, None) => Ok(None),
        }
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!(
                "Async run '{}' has no live run directory to steer.",
                target.or(dir).unwrap_or("")
            )
        })?;

        let run_id = status.run_id.as_str().to_string();

        // S4 — pi `async-steering-action.ts:48`, PERMISSIVE, and ordered BEFORE the state guard
        // exactly as upstream orders it. Steering injects text into a running child's prompt, so
        // an ungated `id`/`dir` lookup over the shared per-cwd root would let one instance speak
        // into another instance's agent.
        if !crate::background::delivery::SessionGate::Permissive.admits(
            SessionId::parse_opt(self.current_session_id().as_deref()).as_ref(),
            status.session_id.as_ref(),
        ) {
            return Err(format!(
                "Async run '{run_id}' was not found in the active session."
            ));
        }

        if !matches!(status.state, RunState::Running | RunState::Queued) {
            return Err(format!(
                "Async run '{run_id}' is not running or queued and cannot be steered."
            ));
        }
        let steps = &status.steps;
        if let Some(index) = index {
            let Some(step) = steps.get(index) else {
                return Err(format!(
                    "Async run '{run_id}' has {} children. Index {index} is out of range.",
                    steps.len()
                ));
            };
            if !matches!(step.status, StepState::Running | StepState::Pending) {
                return Err(format!(
                    "Async run '{run_id}' child {index} is {} and cannot be steered.",
                    run_status::step_state_label(step.status)
                ));
            }
        } else if steps.len() > 1 && !steps.iter().any(|s| s.status == StepState::Running) {
            return Err(format!(
                "Async run '{run_id}' has no running child yet. Provide index to steer a queued \
                 child."
            ));
        }

        let (_, request_id) = control::request_async_steer_with_mode(
            &paths.run_dir,
            message,
            mode,
            index,
            Some("steer-action"),
        )
        .await
        .map_err(|e| e.to_string())?;

        // SUBA-049 — WAIT FOR THE CHILD'S ANSWER. This is the whole item: before it, the function
        // returned here with "Steering queued …", which was true of the file drop and said nothing
        // about delivery. A steer that reached a child mid-tool and never got a turn boundary, one
        // refused by a child whose host cannot inject messages, and one acted on immediately were
        // three identical successes.
        //
        // pi's own budget: `waitForSteeringAction({ …, timeoutMs: input.ackTimeoutMs ?? 3_000 })`
        // (`runs/foreground/async-steering-action.ts`). Upstream waits on `status.steering`, which
        // its RUNNER folds acks into; cyrup's parent reads the ack files directly and narrows them
        // to this request — see [`control::take_steer_acks`]'s `[CYRUP-DELTA]`.
        let outcome = Self::await_steer_ack(&paths.run_dir, &request_id, index).await;
        let state = match outcome.as_ref() {
            // pi's `stateText` (`async-steering-action.ts`'s final `return`). No acknowledgment
            // inside the budget is upstream's `pending`, NOT a failure: the request is on disk and a
            // child that reaches a safe point later still takes it.
            None => "pending",
            Some(ack) => ack.state.as_str(),
        };
        let text = format!("Steering {state} for async run {run_id} (request {request_id}).");
        match outcome {
            // pi sets `isError` for `failed`/`partial` only; `pending` and `queued` are ordinary
            // successes because the request is still live.
            Some(ack) if ack.state == control::SteerAckState::Failed => {
                let failure_text = format!("{text} {}", ack.message);
                // pi `:225`: `appendSteeringNotice("failed", ...)` happens BEFORE the error result
                // is returned, so the stream records the failure even when the caller drops the
                // answer. Every early-return above this point (the `asyncDir`/session/state/index
                // guards) returns before this line is reachable, so a refusal never writes it —
                // the target's `events.jsonl` stays byte-identical on every refusal path.
                append_steering_notice(
                    &paths,
                    &status,
                    &request_id,
                    SteeringNoticeState::Failed,
                    &failure_text,
                );
                Err(failure_text)
            }
            _ => Ok(text),
        }
    }

    /// SUBA-049 — poll this run's steer-acknowledgment directory for `request_id` until one arrives
    /// or [`STEER_ACK_TIMEOUT`] elapses.
    ///
    /// Returns the LAST acknowledgment seen for the request, not the first, and that matters: the
    /// lifecycle can produce two (`queued` then `delivered`) and the later one is the outcome.
    /// [`control::take_steer_acks`] already returns them in lifecycle order — see
    /// `steer_ack_write_path` for why the file name encodes it.
    ///
    /// `index` narrows a fan-out's answers to the addressed child; with no index every running
    /// child is a legitimate answerer and the first one back is taken, which is upstream's own
    /// `targetIndexes` semantics collapsed to the single answer this surface reports.
    pub(crate) async fn await_steer_ack(
        run_dir: &Path,
        request_id: &str,
        index: Option<usize>,
    ) -> Option<control::SteerAck> {
        Self::await_steer_ack_within(run_dir, request_id, index, STEER_ACK_TIMEOUT).await
    }

    /// As [`Self::await_steer_ack`], with a caller-supplied budget.
    ///
    /// WORKFLOW_14: `runs.steer`'s `ackTimeoutMs` (`prelude.js`'s option list, already validated
    /// there as a positive integer) is a real parameter, and a surface that accepted it and then
    /// waited [`STEER_ACK_TIMEOUT`] anyway would be silently retargeting the caller's own request —
    /// the failure class `control_steer` documents for `mode`. The default-budget entry point above
    /// is unchanged for every async caller.
    pub(crate) async fn await_steer_ack_within(
        run_dir: &Path,
        request_id: &str,
        index: Option<usize>,
        budget: std::time::Duration,
    ) -> Option<control::SteerAck> {
        let deadline = std::time::Instant::now() + budget;
        let mut latest: Option<control::SteerAck> = None;
        loop {
            for ack in control::take_steer_acks(run_dir, Some(request_id)).await {
                if index.is_some_and(|want| want != ack.index) {
                    continue;
                }
                latest = Some(ack);
            }
            if latest.is_some() || std::time::Instant::now() >= deadline {
                return latest;
            }
            tokio::time::sleep(STEER_ACK_POLL_INTERVAL).await;
        }
    }
}

/// pi `appendSteeringNotice` (`async-steering-action.ts:158-166`) — one diagnostic line into the
/// TARGET RUN's own `events.jsonl`.
///
/// # The session on this line is the RUN's, not the caller's
///
/// `currentSessionId` is upstream's field NAME; its VALUE is `status.sessionId`
/// (`:162`'s `...(status.sessionId ? { currentSessionId: status.sessionId } : {})`). The caller's
/// session is threaded somewhere else entirely, forty lines down at `:203`
/// ([`crate::background::control::resume`]'s `current_session`). Conflating them would attribute
/// another instance's steer to this one in the very stream a multi-instance post-mortem reads.
///
/// The key is OMITTED — not null, not empty — when the run carries no session, because a reader
/// distinguishing "unattributed run" from "attributed to nothing" is the whole point of the
/// conditional spread upstream wrote it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SteeringNoticeState {
    /// The child's answer, once it arrived, was a failure — `control::SteerAckState::Failed`.
    Failed,
    /// No producer in this build: upstream emits it from the steering-recovery block, which cyrup
    /// has not ported (this module's doc, "what is NOT here"). Kept so the wire shape is whole.
    #[allow(dead_code)]
    Recovered,
}

/// pi `appendSteeringNotice` (`async-steering-action.ts:158-166`), best-effort by construction:
/// upstream wraps the write in `try { … } catch { /* the action result and status remain
/// authoritative */ }`, so a failure here must never turn an already-decided steer outcome into a
/// different one.
fn append_steering_notice(
    paths: &RunPaths,
    status: &RunStatus,
    request_id: &str,
    state: SteeringNoticeState,
    message: &str,
) {
    let mut line = serde_json::Map::new();
    line.insert("type".into(), "subagent.steering.notice".into());
    line.insert("ts".into(), crate::time::now_epoch_millis().into());
    line.insert("runId".into(), status.run_id.as_str().into());
    line.insert("requestId".into(), request_id.into());
    line.insert(
        "state".into(),
        serde_json::to_value(state).unwrap_or(serde_json::Value::Null),
    );
    line.insert("message".into(), message.into());
    // `:162`'s conditional spread: the RUN's own session, and only when it has one.
    if let Some(session) = status.session_id.as_ref() {
        line.insert("currentSessionId".into(), session.as_str().into());
    }
    // Upstream's `catch {}`: the action result and status remain authoritative if diagnostic
    // notification persistence fails.
    let _ =
        crate::artifacts::append_jsonl(&paths.events, &serde_json::Value::Object(line).to_string());
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::sync::Arc;

    use crate::background::{RunId, RunMode, RunPaths};
    use crate::extension::executor::SubagentExecutor;
    use crate::extension::testsupport::{FixedSessionHost, seed_running_run};

    use super::{SteeringNoticeState, append_steering_notice};

    /// S4 — pi `async-steering-action.ts:48`, PERMISSIVE, and the zero-trace invariant every
    /// refusal in this directory shares: a refused steer must not write a
    /// `subagent.steering.notice` line (or anything else) into the target's `events.jsonl`.
    #[tokio::test]
    async fn control_steer_refuses_a_foreign_session_and_writes_no_notice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = seed_running_run(dir.path(), "run0steerforeign", &["researcher"]);
        let mut status: crate::background::RunStatus =
            serde_json::from_slice(&std::fs::read(&paths.status).expect("read status"))
                .expect("parse status");
        status.session_id = crate::identity::SessionId::parse("session-OWNER");
        std::fs::write(
            &paths.status,
            serde_json::to_string(&status).expect("serialize status"),
        )
        .expect("rewrite status with an owning session");

        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionHost("session-CALLER")));

        let err = executor
            .control_steer(
                dir.path(),
                Some("run0steerforeign"),
                None,
                Some("keep going"),
                None,
                None,
                None,
            )
            .await
            .expect_err("a foreign session must be refused");
        assert_eq!(
            err,
            "Async run 'run0steerforeign' was not found in the active session."
        );
        assert!(
            !paths.events.exists(),
            "a refused steer must leave zero filesystem trace, including no steering notice"
        );
    }

    /// pi `:162`'s two load-bearing properties: the key's VALUE is the RUN's own session (never
    /// the caller's, which is a wholly different concept threaded through `resume`'s
    /// `current_session` instead), and the key is OMITTED — not `null`, not `""` — when the run
    /// carries no session at all.
    #[test]
    fn append_steering_notice_carries_the_runs_own_session_and_omits_the_key_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");

        let owned_id = RunId::from_token("run0noticeown");
        let owned_paths = RunPaths::for_run(&async_root, &results_dir, &owned_id);
        std::fs::create_dir_all(&owned_paths.run_dir).expect("mkdir run dir");
        let mut owned = crate::background::RunStatus::queued(owned_id, RunMode::Single, Some(1));
        owned.session_id = crate::identity::SessionId::parse("session-OWNER");

        append_steering_notice(
            &owned_paths,
            &owned,
            "req-1",
            SteeringNoticeState::Failed,
            "boom",
        );
        let line: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(&owned_paths.events)
                .expect("events.jsonl written")
                .lines()
                .next()
                .expect("one line"),
        )
        .expect("valid json");
        assert_eq!(line["type"], serde_json::json!("subagent.steering.notice"));
        assert_eq!(line["runId"], serde_json::json!("run0noticeown"));
        assert_eq!(line["requestId"], serde_json::json!("req-1"));
        assert_eq!(line["state"], serde_json::json!("failed"));
        assert_eq!(line["message"], serde_json::json!("boom"));
        assert_eq!(
            line["currentSessionId"],
            serde_json::json!("session-OWNER"),
            "the key's VALUE is the RUN's own session, never the caller's"
        );

        let sessionless_id = RunId::from_token("run0noticenone");
        let sessionless_paths = RunPaths::for_run(&async_root, &results_dir, &sessionless_id);
        std::fs::create_dir_all(&sessionless_paths.run_dir).expect("mkdir run dir");
        let sessionless =
            crate::background::RunStatus::queued(sessionless_id, RunMode::Single, Some(1));

        append_steering_notice(
            &sessionless_paths,
            &sessionless,
            "req-2",
            SteeringNoticeState::Failed,
            "boom",
        );
        let line2: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(&sessionless_paths.events)
                .expect("events.jsonl written")
                .lines()
                .next()
                .expect("one line"),
        )
        .expect("valid json");
        assert!(
            !line2
                .as_object()
                .expect("a JSON object")
                .contains_key("currentSessionId"),
            "no session at all must OMIT the key entirely, not null/empty: {line2}"
        );
    }
}
