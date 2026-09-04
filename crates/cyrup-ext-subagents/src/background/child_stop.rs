//! SUBA-087 — the runner-side state and pure status transitions behind a child-scoped stop, the
//! port of pi `subagent-runner.ts:2596,2955-3031,3048-3055` @v0.64.0 (`childStopRequests`,
//! `activeChildStops`, `markChildStopRequested`, `markChildStopped`, `stopChildStep`,
//! `registerStepStop`, `appendChildStatusEvent`).
//!
//! Upstream keeps two per-run maps inside the runner closure: `childStopRequests` (index → the
//! identity the request named and when it landed) and `activeChildStops` (index → the live child's
//! stop callback). A request for a child that is not yet running is remembered and applied the
//! moment that child registers (`registerStepStop` calls `stop()` immediately when
//! `childStopRequests.has(flatIndex)`, `:3054`), and a request for a child that already finished
//! is refused with a `subagent.step.stop_failed` event (`:3022-3025`).
//!
//! # Functional core / imperative shell
//!
//! The status transitions (`markChildStopRequested`, `markChildStopped`) are pure functions over
//! `&mut RunStatus` here — [`mark_child_stop_requested`], [`mark_child_stopped`] — and return a
//! domain enum/summary the shell renders into `events.jsonl` lines and the `status.json` write.
//! The shell (the control-inbox watcher task and the step loop in `runner_main.rs`) owns every
//! `.await`, the shared-status lock discipline, and the event writer. That split is what lets the
//! transitions be pinned by plain unit tests without a filesystem or a live child.
//!
//! # Why a registry rather than a per-step field on the executor
//!
//! The watcher task that receives the request and the executor that dispatches the child are two
//! different tasks holding two different views; the one thing they share is this registry. The
//! per-step stop handle is a child token of the run-wide interrupt token
//! (`interrupt_cancel.child_token()`), so a run-wide stop/interrupt/timeout still tears every child
//! down through the parent, and a child-scoped stop cancels exactly one child's token — the same
//! shape as upstream's `stopAbortController` (run-wide) beside `registerStop` (per child).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use cyrup_core::CancelToken;

use crate::background::child_identity::{async_status_child_identity, is_stoppable_step_state};
use crate::background::control::STOP_MESSAGE;
use crate::background::{RunStatus, StepState};

/// One recorded child-scoped stop request (pi `childStopRequests`'s value shape,
/// `subagent-runner.ts:2596`: `{ childId: string; requestedAt: number }`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildStopRecord {
    /// The identity the request named (or the runner derived from the index,
    /// `stopChildStep` `:3020`), echoed on every event about this child.
    pub child_id: String,
    /// Epoch milliseconds the request was applied.
    pub requested_at: i64,
}

/// The shared per-run registry: which steps have a child-scoped stop recorded against them, and
/// which steps currently have a live child that can be torn down.
#[derive(Clone, Default)]
pub struct ChildStopRegistry {
    inner: Arc<Mutex<ChildStopMaps>>,
}

#[derive(Default)]
struct ChildStopMaps {
    /// pi `childStopRequests` (`:2596`).
    requests: BTreeMap<usize, ChildStopRecord>,
    /// pi `activeChildStops` (`:2595`) — the live child's stop handle per step index.
    active: BTreeMap<usize, CancelToken>,
}

impl ChildStopRegistry {
    /// A fresh, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ChildStopMaps> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record a request against `index` (pi `childStopRequests.set`, `:2982`).
    pub fn record(&self, index: usize, record: ChildStopRecord) {
        self.lock().requests.insert(index, record);
    }

    /// The request recorded against `index`, if any (pi `childStopRequests.get`).
    #[must_use]
    pub fn recorded(&self, index: usize) -> Option<ChildStopRecord> {
        self.lock().requests.get(&index).cloned()
    }

    /// Whether a request is recorded against `index` (pi `childStopRequests.has`).
    #[must_use]
    pub fn is_requested(&self, index: usize) -> bool {
        self.lock().requests.contains_key(&index)
    }

    /// Every recorded request, in index order — the whole-run stop path emits a terminal
    /// `subagent.child-status` for each (pi `appendTerminalChildStatusEvent`, `:2975-2978`).
    #[must_use]
    pub fn recorded_indexes(&self) -> Vec<(usize, ChildStopRecord)> {
        self.lock()
            .requests
            .iter()
            .map(|(index, record)| (*index, record.clone()))
            .collect()
    }

    /// pi `registerStepStop(flatIndex, stop)` (`:3048-3055`): register the live child's stop
    /// handle for `index`, and fire it at once if a request is already recorded — that is how a
    /// stop that landed while the child was still `pending` (`subagent.step.stop_queued`) is
    /// applied the moment the child starts.
    pub fn register_active(&self, index: usize, token: CancelToken) {
        let already_requested = {
            let mut maps = self.lock();
            maps.active.insert(index, token.clone());
            maps.requests.contains_key(&index)
        };
        if already_requested {
            token.cancel();
        }
    }

    /// pi `registerStepStop(flatIndex, undefined)` (`:3049-3052`): the child for `index` is gone.
    pub fn clear_active(&self, index: usize) {
        self.lock().active.remove(&index);
    }

    /// The live child's stop handle for `index`, if one is registered (pi
    /// `activeChildStops.get`, `:3026`).
    #[must_use]
    pub fn active_token(&self, index: usize) -> Option<CancelToken> {
        self.lock().active.get(&index).cloned()
    }

    /// pi `stopChildStep`'s `const stop = activeChildStops.get(index); if (stop) stop();`
    /// (`:3026-3027`): tear the live child for `index` down, reporting whether there was one.
    pub fn cancel_active(&self, index: usize) -> bool {
        match self.active_token(index) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }
}

/// What [`mark_child_stop_requested`] decided — pi `markChildStopRequested`'s boolean return
/// (`:2979-2991`) plus the one fact `stopChildStep` re-reads afterwards (`:3028`: whether the step
/// was still `pending`, which decides `subagent.step.stop_queued`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChildStopMarking {
    /// The step was `pending` or `running` and now carries `stopRequested`/`stopRequestedAt`.
    Requested {
        /// The identity recorded for the step.
        child_id: String,
        /// The step's agent, carried onto the `subagent.step.stop_requested` /
        /// `subagent.child-status` events.
        agent: String,
        /// `true` when the step had not started yet — the request is remembered and applied at
        /// dispatch (pi's `stop_queued` case).
        was_pending: bool,
    },
    /// pi `if (!step || (step.status !== "pending" && step.status !== "running")) return false;`
    /// — the step does not exist or is already past stoppable (`subagent.step.stop_failed`,
    /// message `Child is not pending or running.`).
    NotStoppable,
}

/// pi `markChildStopRequested(index, childId, now)` (`subagent-runner.ts:2979-2991`), pure: gate on
/// pending/running, stamp `stopRequested`/`stopRequestedAt`, drop the live `activityState`, touch
/// `lastUpdate`. The registry record and the events are the shell's job.
pub fn mark_child_stop_requested(
    status: &mut RunStatus,
    index: usize,
    child_id: &str,
    now: i64,
) -> ChildStopMarking {
    let Some(step) = status.steps.get_mut(index) else {
        return ChildStopMarking::NotStoppable;
    };
    if !is_stoppable_step_state(step.status) {
        return ChildStopMarking::NotStoppable;
    }
    let was_pending = step.status == StepState::Pending;
    step.stop_requested = true;
    step.stop_requested_at = Some(now);
    step.telemetry.activity_state = None;
    let agent = step.agent.clone();
    status.last_update = now;
    ChildStopMarking::Requested {
        child_id: child_id.to_string(),
        agent,
        was_pending,
    }
}

/// The facts [`mark_child_stopped`] hands back for the `subagent.step.stopped` event
/// (`subagent-runner.ts:3008`) and the stopped result record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildStoppedSummary {
    /// The identity to echo — the recorded request's, else the step's own (`:3007`).
    pub child_id: String,
    /// The step's agent.
    pub agent: String,
    /// pi `step.durationMs = step.startedAt ? now - step.startedAt : 0` (`:3003`).
    pub duration_ms: i64,
}

/// pi `markChildStopped(index, now)` (`subagent-runner.ts:2992-3010`), pure: the step becomes
/// `stopped` with pi's literal stop message as its error, `stopped`/`stopRequested` set,
/// `stopRequestedAt` taken from the recorded request (else what the step already had, else `now`),
/// `activityState` dropped, end/duration/last-activity stamped. Idempotent: a step already
/// `stopped` is left alone and reports `None` (`if (step.status === "stopped") return;`, `:2994`).
///
/// `exitCode: 1` (`:2997`) has no home on cyrup's [`crate::background::StepStatus`]; it is carried
/// on the step's `SingleResult` instead, which is where every other cyrup reader looks for it.
pub fn mark_child_stopped(
    status: &mut RunStatus,
    index: usize,
    recorded: Option<&ChildStopRecord>,
    now: i64,
) -> Option<ChildStoppedSummary> {
    let step = status.steps.get_mut(index)?;
    if step.status == StepState::Stopped {
        return None;
    }
    step.status = StepState::Stopped;
    step.error = Some(STOP_MESSAGE.to_string());
    step.stopped = true;
    step.stop_requested = true;
    step.stop_requested_at = recorded
        .map(|record| record.requested_at)
        .or(step.stop_requested_at)
        .or(Some(now));
    step.telemetry.activity_state = None;
    step.ended_at = Some(now);
    let duration_ms = step
        .started_at
        .map(|started| (now - started).max(0))
        .unwrap_or(0);
    step.telemetry.last_activity_at = Some(now);
    let agent = step.agent.clone();
    let child_id = recorded
        .map(|record| record.child_id.clone())
        .unwrap_or_else(|| async_status_child_identity(step, index));
    status.last_update = now;
    Some(ChildStoppedSummary {
        child_id,
        agent,
        duration_ms,
    })
}

/// The two lifecycle words a `subagent.child-status` event carries (pi
/// `SubagentChildStatusEvent.status`, `shared/types.ts:2304` @v0.64.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildStatusWord {
    /// The stop was recorded; the child is being torn down or will be skipped.
    Stopping,
    /// The child is terminally stopped.
    Stopped,
}

impl ChildStatusWord {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }
}

/// The `subagent.child-status` event payload (pi `appendChildStatusEvent`,
/// `subagent-runner.ts:2956-2974`, shape `SubagentChildStatusEvent` `shared/types.ts:2299-2315`):
/// `version: 1`, `reason: "user"`, `source: "async"`, the step index and agent. cyrup's step
/// record has none of the four optional workflow fields (`childRunId`/`workflowKey`/`phase`/
/// `label`), so they are omitted exactly as upstream's conditional spreads omit them when absent.
#[must_use]
pub fn child_status_event(
    run_id: &str,
    index: usize,
    child_id: &str,
    agent: &str,
    word: ChildStatusWord,
    now: i64,
) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "ts": now,
        "runId": run_id,
        "childId": child_id,
        "status": word.as_str(),
        "reason": "user",
        "source": "async",
        "stepIndex": index,
        "agent": agent,
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
    use crate::background::{RunId, RunMode, StepStatus};

    fn status_with(states: &[StepState]) -> RunStatus {
        let mut status = RunStatus::queued(
            RunId::from_token("childstop001"),
            RunMode::Chain,
            Some(std::process::id()),
        );
        status.steps = states
            .iter()
            .enumerate()
            .map(|(i, state)| {
                let mut step = StepStatus::pending(format!("agent-{i}"));
                step.status = *state;
                step
            })
            .collect();
        status
    }

    /// `subagent-runner.ts:2979-2991` — pending/running steps take the request and carry the two
    /// stamps; anything else (or an index past the list) is refused.
    #[test]
    fn mark_child_stop_requested_gates_on_pending_or_running() {
        let mut status = status_with(&[
            StepState::Complete,
            StepState::Running,
            StepState::Pending,
            StepState::Paused,
        ]);
        status.steps[1].telemetry.activity_state =
            Some(crate::background::ActivityState::NeedsAttention);

        assert_eq!(
            mark_child_stop_requested(&mut status, 0, "step:0", 10),
            ChildStopMarking::NotStoppable
        );
        assert_eq!(
            mark_child_stop_requested(&mut status, 3, "step:3", 10),
            ChildStopMarking::NotStoppable
        );
        assert_eq!(
            mark_child_stop_requested(&mut status, 9, "step:9", 10),
            ChildStopMarking::NotStoppable
        );
        assert!(!status.steps[0].stop_requested);

        assert_eq!(
            mark_child_stop_requested(&mut status, 1, "step:1", 10),
            ChildStopMarking::Requested {
                child_id: "step:1".to_string(),
                agent: "agent-1".to_string(),
                was_pending: false,
            }
        );
        assert!(status.steps[1].stop_requested);
        assert_eq!(status.steps[1].stop_requested_at, Some(10));
        assert_eq!(
            status.steps[1].status,
            StepState::Running,
            "not yet stopped"
        );
        assert!(status.steps[1].telemetry.activity_state.is_none());
        assert_eq!(status.last_update, 10);

        assert_eq!(
            mark_child_stop_requested(&mut status, 2, "step:2", 11),
            ChildStopMarking::Requested {
                child_id: "step:2".to_string(),
                agent: "agent-2".to_string(),
                was_pending: true,
            }
        );
    }

    /// `subagent-runner.ts:2992-3010` — the stopped marking, its stamps, its idempotency, and the
    /// identity/timestamp precedence (recorded request first).
    #[test]
    fn mark_child_stopped_stamps_the_step_and_is_idempotent() {
        let mut status = status_with(&[StepState::Running, StepState::Pending]);
        status.steps[0].started_at = Some(1_000);
        let record = ChildStopRecord {
            child_id: "step:0".to_string(),
            requested_at: 1_500,
        };
        let summary = mark_child_stopped(&mut status, 0, Some(&record), 2_250).expect("marked");
        assert_eq!(
            summary,
            ChildStoppedSummary {
                child_id: "step:0".to_string(),
                agent: "agent-0".to_string(),
                duration_ms: 1_250,
            }
        );
        let step = &status.steps[0];
        assert_eq!(step.status, StepState::Stopped);
        assert_eq!(step.error.as_deref(), Some(STOP_MESSAGE));
        assert!(step.stopped && step.stop_requested);
        assert_eq!(step.stop_requested_at, Some(1_500));
        assert_eq!(step.ended_at, Some(2_250));
        assert_eq!(step.telemetry.last_activity_at, Some(2_250));
        assert_eq!(status.last_update, 2_250);

        // Already stopped: untouched, `None`.
        assert!(mark_child_stopped(&mut status, 0, Some(&record), 9_999).is_none());
        assert_eq!(status.steps[0].ended_at, Some(2_250));

        // No record and never started: identity falls back to the positional one, duration 0,
        // `stopRequestedAt` falls back to `now`.
        let summary = mark_child_stopped(&mut status, 1, None, 3_000).expect("marked");
        assert_eq!(summary.child_id, "step:1");
        assert_eq!(summary.duration_ms, 0);
        assert_eq!(status.steps[1].stop_requested_at, Some(3_000));

        // Out of range: nothing to mark.
        assert!(mark_child_stopped(&mut status, 5, None, 3_000).is_none());
    }

    /// `subagent-runner.ts:3048-3055` — a handle registered AFTER the request was recorded is
    /// fired immediately (the `stop_queued` → applied-at-dispatch path); one registered before is
    /// fired by `cancel_active`; clearing forgets it.
    #[test]
    fn registry_applies_a_queued_request_at_registration_and_cancels_live_ones() {
        let registry = ChildStopRegistry::new();
        assert!(!registry.cancel_active(0), "nothing registered yet");

        let live = CancelToken::new();
        registry.register_active(0, live.clone());
        assert!(!live.is_cancelled());
        registry.record(
            0,
            ChildStopRecord {
                child_id: "step:0".to_string(),
                requested_at: 1,
            },
        );
        assert!(registry.is_requested(0));
        assert!(registry.cancel_active(0));
        assert!(live.is_cancelled());

        registry.record(
            2,
            ChildStopRecord {
                child_id: "step:2".to_string(),
                requested_at: 2,
            },
        );
        let late = CancelToken::new();
        registry.register_active(2, late.clone());
        assert!(
            late.is_cancelled(),
            "a queued request fires at registration"
        );

        registry.clear_active(2);
        assert!(registry.active_token(2).is_none());
        assert_eq!(
            registry
                .recorded_indexes()
                .into_iter()
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert_eq!(
            registry.recorded(2).map(|r| r.child_id),
            Some("step:2".to_string())
        );
    }

    /// `shared/types.ts:2299-2315` — the event carries pi's fixed fields and words.
    #[test]
    fn child_status_event_has_pis_shape() {
        let event =
            child_status_event("run-1", 3, "step:3", "scout", ChildStatusWord::Stopping, 77);
        assert_eq!(
            event,
            serde_json::json!({
                "version": 1, "ts": 77, "runId": "run-1", "childId": "step:3",
                "status": "stopping", "reason": "user", "source": "async",
                "stepIndex": 3, "agent": "scout",
            })
        );
        assert_eq!(
            child_status_event("r", 0, "step:0", "a", ChildStatusWord::Stopped, 1)["status"],
            serde_json::json!("stopped")
        );
    }
}
