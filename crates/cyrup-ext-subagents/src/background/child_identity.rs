//! SUBA-087 — stable child identity for child-scoped control requests, the port of pi
//! `runs/shared/child-identity.ts` (`:1-51` @v0.64.0; the file is new in `v0.47.1..v0.57.0`,
//! `31a230cb (#1373)`, and unchanged since except for the `includeNested` option noted below).
//!
//! Upstream addresses one child of an async run by a string the caller can copy out of a status
//! surface rather than by a bare index, so a stop aimed at "the second child" survives the caller
//! and the runner disagreeing about which list the number indexes. The identity is derived, never
//! stored: `asyncStatusChildIdentity(step, index)` is the FIRST non-empty entry of
//! `asyncStatusChildIdentityCandidates` (`:16-18`), and that candidate list is FOUR rungs, not
//! three (`child-identity.ts:20-22` @v0.68.0):
//!
//! ```text
//! [step.childId, step.workflowKey, step.runId, `step:${index}`]
//! ```
//!
//! `resolveAsyncStatusChild` (`:24-47`) accepts ANY of them, so a caller may name a workflow child
//! by its explicit child id, its lane key, its child run id, or its position.
//!
//! # What the port can and cannot represent
//!
//! All FOUR rungs are ported. [`StepStatus::child_id`] (SUBA-087) is the first: an identity a
//! producer STAMPED on the step, which outranks the derived ones so that an id a caller copied
//! out of one surface keeps naming the same child on every later one. [`StepStatus::workflow_key`]
//! and [`StepStatus::run_id`] (SCOPE_3d) are the second and third, and the positional
//! `step:<index>` is the fallback.
//!
//! The first rung changes which child no EXISTING caller resolves, and that is by construction,
//! not by accident: nothing in this crate mints a child id today, so every step cyrup declares
//! carries `child_id: None` and the ladder starts, as it did, at the lane key. Upstream is in the
//! same position for a step no producer stamped — its `childId` is `undefined` and the
//! `value.length > 0` filter (`:21`) drops it. What the rung buys is that the ladder CAN now
//! widen: a producer that stamps one (upstream's own do, at `async-status.ts:334` and
//! `async-job-tracker.ts:270`) is resolvable by it, and `status.json` round-trips it.
//!
//! What remains genuinely unrepresentable is the `DynamicGroup` splice residual described below —
//! a dynamic group is still one entry whose members share an identity.
//!
//! `index` here is the index into [`RunStatus::steps`] — the SAME index space cyrup's other
//! per-child surfaces use (`steer`'s `target_index`, the transcript view's `index`, the runner's
//! `output-<index>.log`). SUBA-093 made that a FLAT index: a `ParallelGroup` contributes one entry
//! per MEMBER (`crate::background::flat_index`), so `step:1` of a three-task fan-out names the
//! second task and stops it alone. A `DynamicGroup` is still one entry whose members share an
//! identity — cyrup does not splice materialized items into `RunStatus::steps` as upstream does
//! (`subagent-runner.ts:4155` @v0.64.0); that half is a recorded SUBA-093 residual.
//!
//! # `includeNested` IS ported — as a seam, not as a flag
//!
//! `resolveAsyncStatusChild`'s `includeNested` option (`:27,34-42`, added between v0.57.0 and
//! v0.64.0) walks each step's `children: NestedRunSummary[]` for a nested run id. cyrup reaches
//! it through [`resolve_by_candidates`], which takes the candidate function rather than a boolean:
//! `/subagents-steer` — upstream's other `includeNested: true` caller
//! (`slash/slash-commands.ts:1097-1103`) — passes `candidates_including_nested`
//! (`extension/host/slash_steer.rs:129`), which appends each step's nested run ids to this
//! module's default rungs. That is why the signature here has no `include_nested` flag: the option
//! and the seam select the same behaviour, and the seam cannot be passed by a caller that has not
//! decided it wants nested matches.
//!
//! Two narrowings remain, both deliberate. [`async_status_child_identity_candidates`], the DEFAULT
//! candidate function, does not walk nested runs — so the tool path (`async-stop-action.ts:50`,
//! which never passes the flag upstream either) is unchanged. And cyrup's per-step nested tracking
//! is a list of bare [`crate::background::RunId`]s rather than `NestedRunSummary`s, so the
//! recursive `findNested` descent (`:38`) flattens to one level.

use crate::background::{RunStatus, StepState, StepStatus};

/// One resolved child of an async run (pi `ResolvedAsyncStatusChild`, `child-identity.ts:5-10`):
/// its position, the identity string the runner will echo back on every event it emits for it,
/// and the two facts the stop gate and the receipt need from the step itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAsyncStatusChild {
    /// Index into [`RunStatus::steps`] (pi `flatIndex`).
    pub index: usize,
    /// The child's canonical identity — [`async_status_child_identity`] of the matched step, which
    /// may differ from the candidate the caller spelled (pi `id: asyncStatusChildIdentity(step,
    /// index)`, `:32`).
    pub id: String,
    /// The matched step's lifecycle state, read by [`is_stoppable_async_status_step`]'s caller to
    /// word the refusal.
    pub state: StepState,
    /// The matched step's agent, carried onto the `subagent.child-status` event.
    pub agent: String,
}

/// The outcome of [`resolve_async_status_child`] — pi's `AsyncStatusChildResolution` union
/// (`child-identity.ts:12-14`), whose two failure codes each carry their own sentence.
///
/// A domain enum rather than `Result<_, String>`: both failures are expected business outcomes a
/// caller renders verbatim to the model, not technical errors, and the `not_found`/`ambiguous`
/// distinction is observable (upstream's RPC surface returns the code alongside the message).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AsyncStatusChildResolution {
    /// Exactly one step matched.
    Resolved(ResolvedAsyncStatusChild),
    /// No step matched — pi `{ ok: false, code: "not_found", message }` (`:46`), the message being
    /// `Child '<id>' was not found under async run '<run>'.`
    NotFound(String),
    /// More than one step matched — pi `{ ok: false, code: "ambiguous", message }` (`:45`), the
    /// message being `Child '<id>' is ambiguous under async run '<run>'.`
    Ambiguous(String),
}

impl AsyncStatusChildResolution {
    /// The failure sentence, when this is a failure.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        match self {
            Self::Resolved(_) => None,
            Self::NotFound(message) | Self::Ambiguous(message) => Some(message),
        }
    }
}

/// The positional identity rung — pi's `` `step:${index}` `` (`child-identity.ts:17`).
#[must_use]
pub fn positional_child_identity(index: usize) -> String {
    format!("step:{index}")
}

/// pi `asyncStatusChildIdentity`'s FOUR-rung fallback (`child-identity.ts:16-22`), over its raw
/// inputs: `childId ?? workflowKey ?? runId ?? step:<index>`. Empty strings count as absent,
/// matching the candidate filter at `:21` (`value.length > 0`).
///
/// Upstream derives the identity from the candidate list itself (`candidates[0]!`, `:17`), so the
/// two must agree rung for rung: [`candidates_from_parts`] below is the same four values in the
/// same order.
#[must_use]
pub fn identity_from_parts(
    child_id: Option<&str>,
    workflow_key: Option<&str>,
    run_id: Option<&str>,
    index: usize,
) -> String {
    child_id
        .filter(|id| !id.is_empty())
        .or(workflow_key.filter(|key| !key.is_empty()))
        .or(run_id.filter(|id| !id.is_empty()))
        .map(str::to_string)
        .unwrap_or_else(|| positional_child_identity(index))
}

/// The number of rungs upstream's candidate array carries (`child-identity.ts:21`:
/// `[step.childId, step.workflowKey, step.runId, `step:${index}`]`) — the `Vec` pre-allocation
/// and the one place the count is written down.
const RUNG_COUNT: usize = 4;

/// pi `asyncStatusChildIdentityCandidates` (`child-identity.ts:20-22`): every spelling that names
/// this child, de-duplicated in rung order, empties dropped.
#[must_use]
pub fn candidates_from_parts(
    child_id: Option<&str>,
    workflow_key: Option<&str>,
    run_id: Option<&str>,
    index: usize,
) -> Vec<String> {
    let positional = positional_child_identity(index);
    let mut out: Vec<String> = Vec::with_capacity(RUNG_COUNT);
    for candidate in [child_id, workflow_key, run_id, Some(positional.as_str())]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
    {
        if !out.iter().any(|seen| seen == candidate) {
            out.push(candidate.to_string());
        }
    }
    out
}

/// pi `asyncStatusChildIdentity(step, index)` over a real [`StepStatus`]: `childId ?? workflowKey
/// ?? runId ?? step:<index>`, off the step's own fields (SUBA-087 + SCOPE_3d).
#[must_use]
pub fn async_status_child_identity(step: &StepStatus, index: usize) -> String {
    identity_from_parts(
        step.child_id.as_deref(),
        step.workflow_key.as_ref().map(|key| key.as_str()),
        step.run_id.as_ref().map(|run_id| run_id.as_str()),
        index,
    )
}

/// pi `asyncStatusChildIdentityCandidates(step, index)` over a real [`StepStatus`].
#[must_use]
pub fn async_status_child_identity_candidates(step: &StepStatus, index: usize) -> Vec<String> {
    candidates_from_parts(
        step.child_id.as_deref(),
        step.workflow_key.as_ref().map(|key| key.as_str()),
        step.run_id.as_ref().map(|run_id| run_id.as_str()),
        index,
    )
}

/// pi `resolveAsyncStatusChild(status, childId)` (`child-identity.ts:24-47`, tool-path form with
/// no `includeNested`): collect every step whose candidate set contains `child_id`; exactly one
/// match resolves, more than one is [`AsyncStatusChildResolution::Ambiguous`], none is
/// [`AsyncStatusChildResolution::NotFound`], each with upstream's exact sentence naming the
/// caller's spelling and the run id.
#[must_use]
pub fn resolve_async_status_child(
    status: &RunStatus,
    child_id: &str,
) -> AsyncStatusChildResolution {
    resolve_by_candidates(status, child_id, async_status_child_identity_candidates)
}

/// The resolver over an explicit candidate provider — the match-count logic of
/// `resolveAsyncStatusChild` (`:29-46`) with the identity rungs factored out, so the ambiguity
/// arm is testable even though positional identities are unique by construction.
pub(crate) fn resolve_by_candidates(
    status: &RunStatus,
    child_id: &str,
    candidates_for: impl Fn(&StepStatus, usize) -> Vec<String>,
) -> AsyncStatusChildResolution {
    let mut matches: Vec<ResolvedAsyncStatusChild> = Vec::new();
    for (index, step) in status.steps.iter().enumerate() {
        if candidates_for(step, index)
            .iter()
            .any(|candidate| candidate == child_id)
        {
            matches.push(ResolvedAsyncStatusChild {
                index,
                id: async_status_child_identity(step, index),
                state: step.status,
                agent: step.agent.clone(),
            });
        }
    }
    let run_id = status.run_id.as_str();
    match matches.len() {
        1 => match matches.pop() {
            Some(child) => AsyncStatusChildResolution::Resolved(child),
            None => AsyncStatusChildResolution::NotFound(not_found_message(child_id, run_id)),
        },
        0 => AsyncStatusChildResolution::NotFound(not_found_message(child_id, run_id)),
        _ => AsyncStatusChildResolution::Ambiguous(format!(
            "Child '{child_id}' is ambiguous under async run '{run_id}'."
        )),
    }
}

fn not_found_message(child_id: &str, run_id: &str) -> String {
    format!("Child '{child_id}' was not found under async run '{run_id}'.")
}

/// pi `isStoppableAsyncStatusStep` (`child-identity.ts:49-51`): only a `pending` or `running`
/// child may be stopped. The same predicate over the bare state, for callers that hold one.
#[must_use]
pub fn is_stoppable_step_state(state: StepState) -> bool {
    matches!(state, StepState::Pending | StepState::Running)
}

/// pi `isStoppableAsyncStatusStep(step)` over a real [`StepStatus`].
#[must_use]
pub fn is_stoppable_async_status_step(step: &StepStatus) -> bool {
    is_stoppable_step_state(step.status)
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
    use crate::background::{RunId, RunMode, RunStatus, StepState, StepStatus};

    fn status_with(states: &[StepState]) -> RunStatus {
        let mut status = RunStatus::queued(
            RunId::from_token("childid00001"),
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

    /// `child-identity.ts:16-22` — `childId ?? workflowKey ?? runId ?? step:<index>`, in that
    /// order, with an empty string treated as absent (`:21`).
    ///
    /// MUTATION: drop the `childId` rung (or demote it below `workflowKey`) and the first
    /// assertion resolves to `wf-key` instead of `explicit-child`.
    #[test]
    fn identity_falls_back_child_id_then_workflow_key_then_run_id_then_position() {
        assert_eq!(
            identity_from_parts(Some("explicit-child"), Some("wf-key"), Some("run-9"), 3),
            "explicit-child"
        );
        assert_eq!(
            identity_from_parts(None, Some("wf-key"), Some("run-9"), 3),
            "wf-key"
        );
        assert_eq!(
            identity_from_parts(Some(""), Some("wf-key"), Some("run-9"), 3),
            "wf-key",
            "an empty childId is upstream's `value.length > 0` filter, not a match"
        );
        assert_eq!(identity_from_parts(None, None, Some("run-9"), 3), "run-9");
        assert_eq!(
            identity_from_parts(None, Some(""), Some("run-9"), 3),
            "run-9"
        );
        assert_eq!(identity_from_parts(None, None, None, 3), "step:3");
        assert_eq!(
            identity_from_parts(Some(""), Some(""), Some(""), 0),
            "step:0"
        );
    }

    /// `child-identity.ts:20-22` — every non-empty rung is a candidate, de-duplicated, position
    /// always last, `childId` always FIRST.
    ///
    /// MUTATION: drop the `childId` rung and the four-rung case loses its head entry.
    #[test]
    fn candidates_keep_rung_order_and_dedupe() {
        assert_eq!(
            candidates_from_parts(Some("cid"), Some("wf"), Some("run-1"), 2),
            vec![
                "cid".to_string(),
                "wf".to_string(),
                "run-1".to_string(),
                "step:2".to_string()
            ],
            "all four rungs, in upstream's order"
        );
        assert_eq!(
            candidates_from_parts(Some("same"), Some("same"), None, 1),
            vec!["same".to_string(), "step:1".to_string()],
            "the `new Set` de-dupe (`:21`) keeps the first spelling only"
        );
        assert_eq!(
            candidates_from_parts(None, Some("wf"), Some("wf"), 2),
            vec!["wf".to_string(), "step:2".to_string()]
        );
        assert_eq!(
            candidates_from_parts(None, None, Some("run-1"), 0),
            vec!["run-1".to_string(), "step:0".to_string()]
        );
        assert_eq!(
            candidates_from_parts(None, None, None, 7),
            vec!["step:7".to_string()]
        );
    }

    /// SUBA-087 — the ladder over a REAL [`StepStatus`]: a step carrying a `child_id` resolves by
    /// it and reports it as its canonical spelling, ahead of the `workflow_key` and `run_id` the
    /// same step also carries; a step with no `child_id` falls through IDENTICALLY to the
    /// three-rung behaviour that preceded the field.
    ///
    /// MUTATION: drop the rung from `async_status_child_identity_candidates` and `"explicit-child"`
    /// stops resolving while step 0's canonical id regresses to `lane.a`.
    #[test]
    fn a_stamped_child_id_outranks_the_derived_rungs_and_absence_falls_through() {
        let mut status = status_with(&[StepState::Running, StepState::Running]);
        if let Some(step) = status.steps.get_mut(0) {
            step.child_id = Some("explicit-child".to_string());
            step.workflow_key = crate::workflows::WorkflowKey::parse("lane.a").ok();
            step.run_id = Some(RunId::from_token("childrun00001"));
        }
        if let Some(step) = status.steps.get_mut(1) {
            step.workflow_key = crate::workflows::WorkflowKey::parse("lane.b").ok();
            step.run_id = Some(RunId::from_token("childrun00002"));
        }

        assert_eq!(
            async_status_child_identity_candidates(&status.steps[0], 0),
            vec![
                "explicit-child".to_string(),
                "lane.a".to_string(),
                "childrun00001".to_string(),
                "step:0".to_string()
            ]
        );
        match resolve_async_status_child(&status, "explicit-child") {
            AsyncStatusChildResolution::Resolved(child) => {
                assert_eq!((child.index, child.id.as_str()), (0, "explicit-child"));
            }
            other => panic!("expected the stamped rung to resolve, got {other:?}"),
        }
        // The lower rungs still ADDRESS the same child; only the canonical spelling changed.
        for alias in ["lane.a", "childrun00001", "step:0"] {
            match resolve_async_status_child(&status, alias) {
                AsyncStatusChildResolution::Resolved(child) => assert_eq!(
                    (child.index, child.id.as_str()),
                    (0, "explicit-child"),
                    "{alias} must still name step 0, canonically spelled by the first rung"
                ),
                other => panic!("expected {alias} to resolve, got {other:?}"),
            }
        }
        // …and the step with no `child_id` behaves exactly as it did before the field existed.
        assert_eq!(
            async_status_child_identity_candidates(&status.steps[1], 1),
            vec![
                "lane.b".to_string(),
                "childrun00002".to_string(),
                "step:1".to_string()
            ]
        );
        assert_eq!(async_status_child_identity(&status.steps[1], 1), "lane.b");
    }

    /// `StepStatus::child_id`'s serde discipline: absent from the wire while `None` (so a
    /// `status.json` written before the field existed still round-trips), and round-tripped when
    /// set.
    ///
    /// MUTATION: drop `skip_serializing_if` and every status this crate writes gains a
    /// `"childId": null`; drop `default` and every status written before the field fails to read.
    #[test]
    fn child_id_is_omitted_while_absent_and_round_trips_when_set() {
        let step = StepStatus::pending("scout");
        let json = serde_json::to_string(&step).expect("serialize");
        assert!(
            !json.contains("childId"),
            "an absent child id must not reach the wire: {json}"
        );
        let back: StepStatus = serde_json::from_str(&json).expect("a status with no childId reads");
        assert_eq!(back.child_id, None);

        let mut stamped = StepStatus::pending("scout");
        stamped.child_id = Some("child-7".to_string());
        let json = serde_json::to_string(&stamped).expect("serialize");
        assert!(json.contains(r#""childId":"child-7""#), "{json}");
        let back: StepStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.child_id.as_deref(), Some("child-7"));
    }

    /// On a real cyrup status every child is positional, and the resolved `id` is the canonical
    /// identity (`:32`), carrying the step's state and agent for the gate and the events.
    #[test]
    fn resolves_a_positional_child_with_its_state_and_agent() {
        let status = status_with(&[StepState::Complete, StepState::Running, StepState::Pending]);
        match resolve_async_status_child(&status, "step:1") {
            AsyncStatusChildResolution::Resolved(child) => {
                assert_eq!(child.index, 1);
                assert_eq!(child.id, "step:1");
                assert_eq!(child.state, StepState::Running);
                assert_eq!(child.agent, "agent-1");
            }
            other => panic!("expected a resolution, got {other:?}"),
        }
    }

    /// `child-identity.ts:46` — the not-found sentence names the caller's spelling and the run.
    #[test]
    fn an_unknown_child_reports_upstreams_not_found_sentence() {
        let status = status_with(&[StepState::Running]);
        let resolution = resolve_async_status_child(&status, "step:4");
        assert_eq!(
            resolution,
            AsyncStatusChildResolution::NotFound(
                "Child 'step:4' was not found under async run 'childid00001'.".to_string()
            )
        );
        assert_eq!(
            resolution.failure_message(),
            Some("Child 'step:4' was not found under async run 'childid00001'.")
        );
        // A run with no steps at all resolves nothing (`(status.steps ?? [])`).
        let empty = status_with(&[]);
        assert!(matches!(
            resolve_async_status_child(&empty, "step:0"),
            AsyncStatusChildResolution::NotFound(_)
        ));
    }

    /// `child-identity.ts:45` — more than one match is ambiguous, with its own sentence.
    /// Positional identities are unique by construction, so the count logic is driven through a
    /// candidate provider under which two steps both answer to `shared`.
    #[test]
    fn ambiguity_is_reported_with_upstreams_sentence() {
        let status = status_with(&[StepState::Running, StepState::Running, StepState::Pending]);
        let resolution = resolve_by_candidates(&status, "shared", |_, index| {
            if index < 2 {
                vec!["shared".to_string(), positional_child_identity(index)]
            } else {
                vec![positional_child_identity(index)]
            }
        });
        assert_eq!(
            resolution,
            AsyncStatusChildResolution::Ambiguous(
                "Child 'shared' is ambiguous under async run 'childid00001'.".to_string()
            )
        );
        // …while a candidate only ONE step answers to still resolves through the same provider,
        // and its canonical id is the positional identity, not the alias the caller spelled.
        match resolve_by_candidates(&status, "step:2", |_, index| {
            vec![positional_child_identity(index)]
        }) {
            AsyncStatusChildResolution::Resolved(child) => {
                assert_eq!((child.index, child.id.as_str()), (2, "step:2"));
            }
            other => panic!("expected a resolution, got {other:?}"),
        }
    }

    /// SCOPE_3d — the first two rungs resolve on a real workflow status: a step carrying a
    /// `workflow_key` answers to it (and its canonical id IS the key), one carrying only a
    /// `run_id` answers to that, and both still answer positionally.
    #[test]
    fn workflow_steps_resolve_by_key_then_run_id() {
        let mut status = status_with(&[StepState::Running, StepState::Running]);
        if let Some(step) = status.steps.get_mut(0) {
            step.workflow_key = crate::workflows::WorkflowKey::parse("lane.a").ok();
            step.run_id = Some(RunId::from_token("childrun00001"));
        }
        if let Some(step) = status.steps.get_mut(1) {
            step.run_id = Some(RunId::from_token("childrun00002"));
        }
        match resolve_async_status_child(&status, "lane.a") {
            AsyncStatusChildResolution::Resolved(child) => {
                assert_eq!((child.index, child.id.as_str()), (0, "lane.a"));
            }
            other => panic!("expected the keyed rung to resolve, got {other:?}"),
        }
        match resolve_async_status_child(&status, "childrun00002") {
            AsyncStatusChildResolution::Resolved(child) => {
                assert_eq!(
                    (child.index, child.id.as_str()),
                    (1, "childrun00002"),
                    "a keyless step's canonical id is its run id rung"
                );
            }
            other => panic!("expected the run-id rung to resolve, got {other:?}"),
        }
        // The keyed step still answers to its run id and its position, with the KEY as the
        // canonical spelling (`child-identity.ts:32`).
        match resolve_async_status_child(&status, "step:0") {
            AsyncStatusChildResolution::Resolved(child) => {
                assert_eq!(child.id, "lane.a");
            }
            other => panic!("expected the positional alias to resolve, got {other:?}"),
        }
    }

    /// `child-identity.ts:49-51` — stoppable iff pending or running.
    #[test]
    fn only_pending_and_running_children_are_stoppable() {
        for (state, expected) in [
            (StepState::Pending, true),
            (StepState::Running, true),
            (StepState::Paused, false),
            (StepState::Complete, false),
            (StepState::Failed, false),
            (StepState::Stopped, false),
        ] {
            assert_eq!(is_stoppable_step_state(state), expected, "{state:?}");
            let mut step = StepStatus::pending("x");
            step.status = state;
            assert_eq!(is_stoppable_async_status_step(&step), expected, "{state:?}");
        }
    }
}
