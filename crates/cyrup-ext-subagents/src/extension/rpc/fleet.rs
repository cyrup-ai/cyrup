//! PB-8 — the public FLEET projection: pi `src/extension/rpc.ts:92-118` (the wire types and the
//! five bounds), `:120-156` (`displayText`/`publicTokens`/`activeState`) and `:177-302`
//! (`buildFleetStatus`) @v0.68.0.
//!
//! # The opaque-key state machine, and the three properties that must survive
//!
//! A fleet entry's `key` is `fleet-<n>` and NOTHING else. Upstream's own doc on the field
//! (`rpc.ts:93`) reads *"Opaque key for client-side reconciliation; never a run or async
//! identifier"*, and that is a boundary, not a naming preference: the internal key
//! (`foreground:<runId>:<index>` / `async:<asyncId>[:<index>]`) is exactly the addressing a bus
//! client would need to drive `stop`/`steer` against a child it was never handed, so it stays on
//! this side of the wire and only the opaque alias crosses.
//!
//! 1. **The key map resets whenever the session changes** (`:183-187`). `fleet-3` is only
//!    comparable to `fleet-3` within one session.
//! 2. **The session gate is FAIL-CLOSED** (`:188-191`): no state, no session, or a session
//!    mismatch answers with the EMPTY fleet and CLEARS the map. Multiple cyrup instances share one
//!    per-cwd async root (`background/artifact_roots.rs:281-284`), so an unfiltered answer would
//!    hand one instance's live children to another. This is the same class of gate, for the same
//!    reason, as `background/async_status_snapshot/state.rs:47-53`.
//! 3. **Keys for candidates that are no longer active are EVICTED** (`:297-299`), so the map does
//!    not grow without bound across a long session.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::background::TokenTotals;
use crate::background::active_async_capacity::ActiveAsyncCapacitySnapshot;
use crate::background::{RunMode, StepState};
use crate::tui::fleet_state::FleetState;
use crate::workflows::{sanitize_display_text, truncate_display};

/// pi `MAX_FLEET_ENTRIES` (`rpc.ts:114`).
const MAX_FLEET_ENTRIES: usize = 16;
/// pi `MAX_FLEET_CANDIDATES` (`rpc.ts:115`).
const MAX_FLEET_CANDIDATES: usize = 256;
/// pi `MAX_AGENT_LENGTH` (`rpc.ts:116`).
const MAX_AGENT_LENGTH: usize = 96;
/// pi `MAX_GOAL_LENGTH` (`rpc.ts:117`).
const MAX_GOAL_LENGTH: usize = 512;
/// pi `MAX_METADATA_LENGTH` (`rpc.ts:118`).
const MAX_METADATA_LENGTH: usize = 128;

/// pi's `value.slice(0, 4_096)` pre-slice (`rpc.ts:122`). Load-bearing rather than an
/// optimisation, for the reason `background/async_status_snapshot/types.rs:325-327` already
/// records: the sanitizer walks every character, so an unbounded value makes a bounded output
/// expensive to produce.
const DISPLAY_TEXT_PRESLICE_UTF16_UNITS: usize = 4_096;

/// pi's `FleetKeyState` (`rpc.ts:158-162`), owned by the bridge for its whole lifetime
/// (`rpc.ts:821`).
#[derive(Debug, Default)]
pub(crate) struct FleetKeyState {
    /// The session the current key map belongs to. `None` is upstream's `null`.
    session_id: Option<String>,
    /// The monotonically increasing suffix (`rpc.ts:279`'s `++keyState.next`).
    next: u64,
    /// internal key -> `fleet-<n>`.
    keys: BTreeMap<String, String>,
}

/// pi `displayText` (`rpc.ts:120-124`) over cyrup's own exact ports of `sanitizeDisplayText` and
/// `truncateDisplayText` (`workflows/display_text.rs:82` and `:153`, re-exported at `workflows/mod.rs`).
fn display_text(value: Option<&str>, max_length: usize) -> Option<String> {
    let value = value?;
    let normalized =
        sanitize_display_text(&truncate_display(value, DISPLAY_TEXT_PRESLICE_UTF16_UNITS));
    if normalized.is_empty() {
        None
    } else {
        Some(truncate_display(&normalized, max_length))
    }
}

/// pi `publicTokens` (`rpc.ts:126-152`) — the clamp that turns a garbage counter into a `0`
/// rather than a `NaN` on the wire.
///
/// # `[CYRUP-DELTA, unrepresentable]` — `window` / `windowPeak` have no source
///
/// Upstream's `TokenUsage` is `{input, output, total, window?, windowPeak?}`. cyrup's
/// [`TokenTotals`] (`background/telemetry.rs:41-48`) is the three integers only, and nothing in
/// this crate reads a context-window size. Upstream omits both keys when they are absent
/// (`:149-150`), so an omission here is a shape upstream also produces; it is recorded because the
/// omission is permanent rather than data-dependent.
///
/// The `u64` domain already enforces upstream's `Number.isFinite && raw >= 0 && Math.floor`
/// clauses, so what survives the port is the `total = max(input + output, total)` reconciliation
/// (`:148`) and the saturating add that stands in for `Math.min(Number.MAX_SAFE_INTEGER, …)`.
fn public_tokens(tokens: Option<TokenTotals>) -> Value {
    let tokens = tokens.unwrap_or_default();
    let sum = tokens.input.saturating_add(tokens.output);
    serde_json::json!({
        "input": tokens.input,
        "output": tokens.output,
        "total": sum.max(tokens.total),
    })
}

/// One row before bounding — pi's `FleetCandidate` (`rpc.ts:164-173`).
struct FleetCandidate {
    internal_key: String,
    agent: Option<String>,
    role: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    started_at: Option<i64>,
    tokens: Option<TokenTotals>,
    goal: Option<String>,
}

/// pi `activeState` (`rpc.ts:154-156`) for a STEP.
///
/// Upstream tests the string union `running|queued|pending`; cyrup's [`StepState`] has no `queued`
/// member at all (a step is `Pending` until it is dispatched), so the two live members are the
/// whole set.
fn step_is_active(state: StepState) -> bool {
    matches!(state, StepState::Pending | StepState::Running)
}

/// pi `buildFleetStatus` (`rpc.ts:177-302`).
///
/// `capacity` stands in for upstream's `state.activeAsyncCapacity` (`:301`), which is a FIELD on
/// its `SubagentState`. cyrup's [`FleetState`] carries no such field, so the value is measured by
/// the caller and handed in rather than being read from inside this projection. That is the
/// identical split `extension/executor/reports.rs:92-120` already makes for the doctor report, and
/// its own comment states the rule: *"measured here, not in the formatter"*.
///
/// The caller measures it with `active_async_capacity::capacity::snapshot_for`, which COUNTS and
/// does not reconcile (`background/active_async_capacity/sweep.rs:25-48`) — deliberately, because
/// an external bus client can drive `status` in a loop and a read must not sweep the slot pool.
/// `get_active_async_capacity_snapshot` is the reconciling variant and is NOT used on this path.
/// `[CYRUP-DELTA, mechanism]`.
pub(crate) fn build_fleet_status(
    state: Option<&FleetState>,
    key_state: &mut FleetKeyState,
    session_id: Option<&str>,
    capacity: ActiveAsyncCapacitySnapshot,
) -> Value {
    // `:183-187` — the session rotation.
    if key_state.session_id.as_deref() != session_id {
        key_state.session_id = session_id.map(str::to_string);
        key_state.next = 0;
        key_state.keys.clear();
    }
    // `:188-191` — the fail-closed gate, all three arms. Spelled as [`fleet_gate_open`] so the
    // caller can ask the SAME question before paying for `capacity`, which this arm throws away.
    if !fleet_gate_open(state, session_id) {
        key_state.keys.clear();
        return empty_fleet();
    }
    let (Some(state), Some(session_id)) = (state, session_id) else {
        // Unreachable: `fleet_gate_open` above is exactly this pattern's refutation.
        key_state.keys.clear();
        return empty_fleet();
    };

    let mut total_active: usize = 0;
    let mut candidates: Vec<FleetCandidate> = Vec::new();
    // `:195-198` — `totalActive` counts EVERY candidate, including ones past the candidate cap, so
    // `omitted` at `:300` stays truthful.
    let mut add_candidate = |candidate: FleetCandidate| {
        total_active += 1;
        if candidates.len() < MAX_FLEET_CANDIDATES {
            candidates.push(candidate);
        }
    };

    // `:199-220` — the live FOREGROUND half.
    for control in &state.foreground_controls {
        if control.session_id.as_deref() != Some(session_id) {
            continue;
        }
        if control.active_children.is_empty() {
            // `:211-218`.
            add_candidate(FleetCandidate {
                internal_key: format!(
                    "foreground:{}:{}",
                    control.run_id,
                    control.current_index.unwrap_or(0)
                ),
                agent: control
                    .current_agent
                    .clone()
                    .or_else(|| Some(run_mode_word(control.mode).to_string())),
                role: None,
                model: control.model.clone(),
                effort: control.thinking.clone(),
                started_at: Some(control.started_at),
                tokens: control.tokens.map(scalar_tokens),
                goal: control.description.clone(),
            });
        } else {
            // `:201-209`.
            for child in &control.active_children {
                add_candidate(FleetCandidate {
                    internal_key: format!("foreground:{}:{}", control.run_id, child.index),
                    agent: Some(child.agent.clone()),
                    role: None,
                    model: child.model.clone(),
                    effort: child.thinking.clone(),
                    started_at: Some(child.started_at),
                    tokens: child.tokens.map(scalar_tokens),
                    goal: child.description.clone(),
                });
            }
        }
    }

    // `:221-263` — the live ASYNC half.
    for job in &state.tracked_jobs {
        if job.session_id.as_deref() != Some(session_id) || !job.is_active() {
            continue;
        }
        let status = &job.status;
        let async_id = status.run_id.as_str();
        // `:223` — `job.startedAt ?? job.updatedAt`.
        let started_at = status.started_at;
        let run_tokens = status.telemetry.total_tokens;
        if status.mode == RunMode::Workflow {
            // `:224-232`.
            add_candidate(FleetCandidate {
                internal_key: format!("async:{async_id}"),
                agent: Some("workflow".to_string()),
                role: None,
                model: None,
                effort: None,
                started_at: Some(started_at),
                tokens: run_tokens,
                goal: job.description.clone(),
            });
            continue;
        }
        // `:233-248`. [CYRUP-DELTA, unrepresentable] upstream's `job.steps?.length ? job.steps :
        // job.agents?.map(…)` fallback has no second source here: cyrup's `RunStatus::steps` is
        // written by the runner for every mode and there is no separate `agents` list on
        // `AsyncRunView` — the same absence `background/async_status_snapshot/mod.rs` already
        // records for `job.agents`. The `!steps?.length` arm below is upstream's own and is kept,
        // because a provisional status synthesized at spawn time can genuinely carry no steps yet.
        if status.steps.is_empty() {
            add_candidate(FleetCandidate {
                internal_key: format!("async:{async_id}"),
                agent: Some(run_mode_word(status.mode).to_string()),
                role: None,
                model: None,
                effort: None,
                started_at: Some(started_at),
                tokens: run_tokens,
                goal: job.description.clone(),
            });
            continue;
        }
        let step_count = status.steps.len();
        for (offset, step) in status.steps.iter().enumerate() {
            if !step_is_active(step.status) {
                continue;
            }
            // `:252` — a chain's not-yet-current pending steps are declared work, not live work,
            // so they are not fleet members.
            //
            // [CYRUP-DELTA, unrepresentable] upstream's third conjunct `!job.activeParallelGroup`
            // has no live analogue: `RunStatus::parallel_groups` is a SETTLED-detail record (its
            // own doc, `background/records.rs:190-198`), and since SUBA-093 a parallel group's
            // members carry their OWN live `StepState` in `steps`. A genuinely dispatched member
            // therefore reads `Running`, not `Pending`, and is admitted by this guard already —
            // which is precisely the case upstream's conjunct exists to rescue.
            if step.status == StepState::Pending
                && status.mode == RunMode::Chain
                && offset != status.current_step.unwrap_or(0)
            {
                continue;
            }
            add_candidate(FleetCandidate {
                internal_key: format!("async:{async_id}:{offset}"),
                agent: Some(step.agent.clone()),
                // [CYRUP-DELTA, unrepresentable] upstream's `role: step.label` has no source —
                // `StepStatus` carries no display label, the same absence
                // `background/async_status_snapshot/mod.rs` records for `step.label`.
                role: None,
                model: step.model.as_ref().map(|m| m.as_str().to_string()),
                effort: step.telemetry.thinking.clone(),
                // `:259` — the step's own start, falling back to the run's.
                started_at: Some(step.started_at.unwrap_or(started_at)),
                // `:260` — a single-step run's totals stand in for the step's own.
                tokens: step
                    .telemetry
                    .tokens
                    .or(if step_count == 1 { run_tokens } else { None }),
                goal: job.description.clone(),
            });
        }
    }

    // `:265-269` — oldest first, ties broken on the internal key so the order is total.
    candidates.sort_by(|left, right| {
        let left_started = left.started_at.unwrap_or(i64::MAX);
        let right_started = right.started_at.unwrap_or(i64::MAX);
        left_started
            .cmp(&right_started)
            .then_with(|| left.internal_key.cmp(&right.internal_key))
    });
    let active_keys: std::collections::BTreeSet<&str> =
        candidates.iter().map(|c| c.internal_key.as_str()).collect();

    // `:271-296` — the bounded, display-safe entry window.
    let mut entries: Vec<Value> = Vec::new();
    let mut assigned: Vec<(String, String)> = Vec::new();
    for candidate in &candidates {
        if entries.len() >= MAX_FLEET_ENTRIES {
            break; // `:273`.
        }
        let agent = display_text(candidate.agent.as_deref(), MAX_AGENT_LENGTH);
        // `:276` — a candidate with no displayable agent, or an unusable `startedAt`, is dropped
        // from `entries` but was already counted in `totalActive` at `:196`.
        let (Some(agent), Some(started_at)) = (agent, candidate.started_at.filter(|v| *v >= 0))
        else {
            continue;
        };
        // `:277-281`.
        let key = match key_state.keys.get(&candidate.internal_key) {
            Some(existing) => existing.clone(),
            None => {
                key_state.next += 1;
                let key = format!("fleet-{}", key_state.next);
                assigned.push((candidate.internal_key.clone(), key.clone()));
                key
            }
        };
        let mut entry = Map::new();
        entry.insert("key".to_string(), Value::from(key));
        entry.insert("agent".to_string(), Value::from(agent));
        for (field, value, max) in [
            ("role", candidate.role.as_deref(), MAX_AGENT_LENGTH),
            ("model", candidate.model.as_deref(), MAX_METADATA_LENGTH),
            ("effort", candidate.effort.as_deref(), MAX_METADATA_LENGTH),
        ] {
            if let Some(text) = display_text(value, max) {
                entry.insert(field.to_string(), Value::from(text));
            }
        }
        entry.insert("startedAt".to_string(), Value::from(started_at));
        entry.insert("tokens".to_string(), public_tokens(candidate.tokens));
        // `:285,294`. [CYRUP-DELTA, superset] upstream declares `goal` on the entry and bounds it
        // at `MAX_GOAL_LENGTH`, but NO `buildFleetStatus` call site ever sets `FleetCandidate.goal`
        // @v0.68.0 — the field is permanently absent on the wire. cyrup fills it from the run's or
        // child's own `description`, which is upstream's `AsyncJobState.description` /
        // `ForegroundChildControl.description` — the caller-facing task — so the advertised key
        // carries the thing it names. A delegating host can then reconcile a fleet row against the
        // goal it asked for without addressing the run.
        if let Some(goal) = display_text(candidate.goal.as_deref(), MAX_GOAL_LENGTH) {
            entry.insert("goal".to_string(), Value::from(goal));
        }
        entries.push(Value::Object(entry));
    }
    for (internal_key, key) in assigned {
        key_state.keys.insert(internal_key, key);
    }
    // `:297-299` — evict keys whose candidate is no longer active.
    key_state
        .keys
        .retain(|internal_key, _| active_keys.contains(internal_key.as_str()));

    // `:300-301`.
    let omitted = total_active.saturating_sub(entries.len());
    serde_json::json!({
        "version": 1,
        "entries": entries,
        "totalActive": total_active,
        "topLevelAsyncCapacity": { "used": capacity.used, "limit": capacity.limit },
        "omitted": omitted,
    })
}

/// pi's `!state || !sessionId || state.currentSessionId !== sessionId` (`rpc.ts:188`) — the
/// fail-closed session gate, hoisted out of [`build_fleet_status`] so its caller can ask the same
/// question first.
///
/// That hoist is not cosmetic. `capacity` is measured by the caller (see [`build_fleet_status`]'s
/// own note), and a CLOSED gate returns [`empty_fleet`]'s hardcoded `{used: 0, limit: 0}` — so
/// measuring it before asking would list a session pool on disk for a number that is then
/// discarded, on a path a bus client can drive in a loop.
pub(crate) fn fleet_gate_open(state: Option<&FleetState>, session_id: Option<&str>) -> bool {
    match (state, session_id) {
        (Some(state), Some(session_id)) => state.current_session_id.as_deref() == Some(session_id),
        _ => false,
    }
}

/// pi's empty fleet (`rpc.ts:190`) — including its hard-coded `{used: 0, limit: 0}` capacity: the
/// gate refused to answer, so it reports nothing measured rather than a real reading taken for a
/// session it just declined to speak for.
fn empty_fleet() -> Value {
    serde_json::json!({
        "version": 1,
        "entries": [],
        "totalActive": 0,
        "topLevelAsyncCapacity": { "used": 0, "limit": 0 },
        "omitted": 0,
    })
}

/// The two fleet VIEWS carry a scalar `tokens: Option<u64>` (`tui/fleet_state.rs:195` and `:264`)
/// where upstream's foreground controls carry `{inputTokens, outputTokens, tokens}`. Widening the
/// views is a different task's; `publicTokens`' own `total = max(input + output, total)` makes
/// `{0, 0, n}` render as `n`, which is the honest answer for a source that only knows the total.
/// `[CYRUP-DELTA, unrepresentable]`.
fn scalar_tokens(total: u64) -> TokenTotals {
    TokenTotals {
        input: 0,
        output: 0,
        total,
    }
}

/// pi's `control.mode` / `job.mode ?? "subagent"` agent fallbacks (`rpc.ts:213,243`).
fn run_mode_word(mode: RunMode) -> &'static str {
    crate::formatters::run_mode_label(mode)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::background::async_status_snapshot::testfixtures::run_view;
    use crate::tui::fleet_state::{ForegroundChildView, ForegroundControlView};

    const SESSION: &str = "sessionaaaaaaaa1";
    const OTHER_SESSION: &str = "sessionbbbbbbbb2";

    fn capacity(used: u32, limit: u32) -> ActiveAsyncCapacitySnapshot {
        ActiveAsyncCapacitySnapshot { used, limit }
    }

    /// One live foreground run with no parallel children — pi's `:211-218` candidate.
    fn control(run_id: &str, agent: &str, started_at: i64) -> ForegroundControlView {
        ForegroundControlView {
            run_id: run_id.to_string(),
            session_id: Some(SESSION.to_string()),
            started_at,
            current_agent: Some(agent.to_string()),
            ..ForegroundControlView::default()
        }
    }

    fn state(controls: Vec<ForegroundControlView>) -> FleetState {
        FleetState {
            current_session_id: Some(SESSION.to_string()),
            foreground_controls: controls,
            ..FleetState::default()
        }
    }

    fn entries(fleet: &Value) -> &Vec<Value> {
        fleet["entries"].as_array().expect("entries is an array")
    }

    fn keys(fleet: &Value) -> Vec<String> {
        entries(fleet)
            .iter()
            .map(|e| {
                e["key"]
                    .as_str()
                    .expect("every entry carries a key")
                    .to_string()
            })
            .collect()
    }

    fn agents(fleet: &Value) -> Vec<String> {
        entries(fleet)
            .iter()
            .map(|e| {
                e["agent"]
                    .as_str()
                    .expect("every entry names an agent")
                    .to_string()
            })
            .collect()
    }

    /// pi's fail-closed gate (`rpc.ts:188-191`), all three arms: no state, no session, and a
    /// session mismatch each answer with the EMPTY fleet and CLEAR the key map.
    ///
    /// The capacity handed in is deliberately non-zero, because the discriminating half of this
    /// test is that a refused answer reports `{used: 0, limit: 0}` — upstream's hardcoded
    /// `:190` literal — rather than a real reading taken for a session it just declined to speak
    /// for. Multiple cyrup instances share one per-cwd async root
    /// (`background/artifact_roots.rs`), so leaking through this gate hands one instance's live
    /// children to another.
    #[test]
    fn the_session_gate_is_fail_closed_on_all_three_arms() {
        let mut key_state = FleetKeyState::default();
        let live = state(vec![control("run-a", "alpha", 10)]);

        // Warm the map so the CLEAR below has something to clear.
        let open = build_fleet_status(Some(&live), &mut key_state, Some(SESSION), capacity(2, 4));
        assert_eq!(entries(&open).len(), 1, "{open}");
        assert_eq!(key_state.keys.len(), 1);

        // Arm 1 — no session id at all.
        let refused = build_fleet_status(Some(&live), &mut key_state, None, capacity(2, 4));
        assert_eq!(refused, empty_fleet(), "no session must answer empty");
        assert!(key_state.keys.is_empty(), "the key map is cleared with it");

        // Arm 2 — no state.
        let refused = build_fleet_status(None, &mut key_state, Some(SESSION), capacity(2, 4));
        assert_eq!(refused, empty_fleet(), "no state must answer empty");

        // Arm 3 — the state belongs to a DIFFERENT session.
        let foreign = build_fleet_status(
            Some(&live),
            &mut key_state,
            Some(OTHER_SESSION),
            capacity(2, 4),
        );
        assert_eq!(
            foreign,
            empty_fleet(),
            "another instance's state must never be answered for"
        );
        assert_eq!(
            foreign["topLevelAsyncCapacity"],
            serde_json::json!({ "used": 0, "limit": 0 }),
            "a refused answer reports nothing measured, not the real reading"
        );
        assert!(key_state.keys.is_empty());
    }

    /// The gate predicate the DISPATCHER asks before paying for `capacity` is the same one
    /// `build_fleet_status` applies — if the two could disagree, the caller would either measure
    /// for nothing or hand in a zero the projection then published.
    #[test]
    fn the_hoisted_gate_predicate_agrees_with_the_projection() {
        let live = state(Vec::new());
        let foreign = FleetState {
            current_session_id: Some(OTHER_SESSION.to_string()),
            ..FleetState::default()
        };
        for (st, session, expected) in [
            (Some(&live), Some(SESSION), true),
            (Some(&live), Some(OTHER_SESSION), false),
            (Some(&live), None, false),
            (None, Some(SESSION), false),
            (Some(&foreign), Some(SESSION), false),
        ] {
            let mut key_state = FleetKeyState::default();
            let fleet = build_fleet_status(st, &mut key_state, session, capacity(3, 9));
            assert_eq!(
                fleet_gate_open(st, session),
                expected,
                "gate predicate disagreed for {session:?}"
            );
            assert!(
                !expected || fleet != empty_fleet(),
                "an open gate must not produce the refusal document"
            );
            assert_eq!(
                fleet["topLevelAsyncCapacity"]["limit"],
                serde_json::json!(if expected { 9 } else { 0 }),
                "only an open gate publishes the measured capacity"
            );
        }
    }

    /// pi `rpc.ts:93`'s own contract for the entry key: *"Opaque key for client-side
    /// reconciliation; never a run or async identifier"*.
    ///
    /// The internal key is `foreground:<runId>:<index>` / `async:<asyncId>[:<offset>]` — exactly
    /// the addressing a bus client would need to drive `stop`/`steer` against a child it was never
    /// handed. Emitting it as the entry key is the leak this test exists to catch, so the
    /// assertions are on the SHAPE of every key and on the absence of the run id anywhere in the
    /// entry, not merely on `fleet-1` being present.
    #[test]
    fn entry_keys_are_opaque_and_never_carry_the_run_id() {
        let mut key_state = FleetKeyState::default();
        let live = state(vec![
            control("run-alpha-7", "alpha", 10),
            control("run-beta-9", "beta", 20),
        ]);
        let fleet = build_fleet_status(Some(&live), &mut key_state, Some(SESSION), capacity(1, 4));

        assert_eq!(keys(&fleet), vec!["fleet-1", "fleet-2"], "{fleet}");
        let rendered = fleet.to_string();
        for leaked in ["run-alpha-7", "run-beta-9", "foreground:"] {
            assert!(
                !rendered.contains(leaked),
                "`{leaked}` must never cross the wire: {rendered}"
            );
        }
        for key in keys(&fleet) {
            assert!(
                key.strip_prefix("fleet-")
                    .is_some_and(|n| n.parse::<u64>().is_ok()),
                "every key is `fleet-<n>` and nothing else, saw `{key}`"
            );
        }
    }

    /// `:277-281` — a candidate that is still active keeps the key it was already given, so a
    /// client can reconcile row-by-row across polls; `:297-299` — a candidate that is gone has its
    /// key EVICTED, so the map does not grow without bound across a long session.
    #[test]
    fn keys_are_stable_across_polls_and_evicted_when_the_candidate_goes_away() {
        let mut key_state = FleetKeyState::default();
        let both = state(vec![
            control("run-a", "alpha", 10),
            control("run-b", "beta", 20),
        ]);

        let first = build_fleet_status(Some(&both), &mut key_state, Some(SESSION), capacity(0, 0));
        assert_eq!(keys(&first), vec!["fleet-1", "fleet-2"]);

        let again = build_fleet_status(Some(&both), &mut key_state, Some(SESSION), capacity(0, 0));
        assert_eq!(
            keys(&again),
            vec!["fleet-1", "fleet-2"],
            "the same live candidates must keep the same opaque keys"
        );
        assert_eq!(
            key_state.next, 2,
            "no new key was minted for a known candidate"
        );

        // `beta` finishes. `alpha` keeps `fleet-1`; `beta`'s entry leaves the map entirely.
        let one = state(vec![control("run-a", "alpha", 10)]);
        let third = build_fleet_status(Some(&one), &mut key_state, Some(SESSION), capacity(0, 0));
        assert_eq!(keys(&third), vec!["fleet-1"]);
        assert_eq!(
            key_state.keys.len(),
            1,
            "the departed candidate's key is evicted, not retained: {:?}",
            key_state.keys
        );

        // A NEW candidate gets a fresh number rather than reusing the evicted one.
        let renewed = state(vec![
            control("run-a", "alpha", 10),
            control("run-c", "gamma", 30),
        ]);
        let fourth = build_fleet_status(
            Some(&renewed),
            &mut key_state,
            Some(SESSION),
            capacity(0, 0),
        );
        assert_eq!(keys(&fourth), vec!["fleet-1", "fleet-3"], "{fourth}");
    }

    /// `:183-187` — the map is keyed to ONE session. `fleet-3` is only comparable to `fleet-3`
    /// within a session, so a session change resets the counter as well as the map.
    #[test]
    fn a_session_change_rotates_the_whole_key_map() {
        let mut key_state = FleetKeyState::default();
        let a = state(vec![control("run-a", "alpha", 10)]);
        assert_eq!(
            keys(&build_fleet_status(
                Some(&a),
                &mut key_state,
                Some(SESSION),
                capacity(0, 0)
            )),
            vec!["fleet-1"]
        );

        let b = FleetState {
            current_session_id: Some(OTHER_SESSION.to_string()),
            foreground_controls: vec![ForegroundControlView {
                session_id: Some(OTHER_SESSION.to_string()),
                ..control("run-z", "zeta", 5)
            }],
            ..FleetState::default()
        };
        assert_eq!(
            keys(&build_fleet_status(
                Some(&b),
                &mut key_state,
                Some(OTHER_SESSION),
                capacity(0, 0)
            )),
            vec!["fleet-1"],
            "the counter restarts for the new session rather than continuing at fleet-2"
        );
        assert_eq!(key_state.session_id.as_deref(), Some(OTHER_SESSION));
    }

    /// `MAX_FLEET_ENTRIES` (`rpc.ts:114` = 16) bounds the rendered window; `totalActive`
    /// (`:195-198`) counts EVERY candidate so `omitted` (`:300`) stays truthful. A client sizing a
    /// panel off `entries.length` while a delegating host sizes a budget off `totalActive` needs
    /// both to be right.
    #[test]
    fn the_entry_window_is_bounded_at_sixteen_and_omitted_stays_truthful() {
        let mut key_state = FleetKeyState::default();
        let live = state(
            (0..20)
                .map(|i| control(&format!("run-{i}"), &format!("agent-{i}"), 100 + i))
                .collect(),
        );
        let fleet = build_fleet_status(Some(&live), &mut key_state, Some(SESSION), capacity(0, 0));

        assert_eq!(entries(&fleet).len(), MAX_FLEET_ENTRIES);
        assert_eq!(entries(&fleet).len(), 16, "the constant is upstream's 16");
        assert_eq!(fleet["totalActive"], serde_json::json!(20));
        assert_eq!(fleet["omitted"], serde_json::json!(4));
    }

    /// `MAX_FLEET_CANDIDATES` (`rpc.ts:115` = 256) bounds the COLLECTION, before the sort.
    ///
    /// Made observable by giving the candidates DESCENDING start times: the 257th and later
    /// controls are the oldest, so if the cap were removed they would sort to the front of the
    /// window. With the cap they are never collected at all, and the window opens on candidate
    /// 255. `totalActive` still counts all 300.
    #[test]
    fn candidate_collection_is_capped_at_two_hundred_and_fifty_six() {
        let mut key_state = FleetKeyState::default();
        let live = state(
            (0..300i64)
                .map(|i| control(&format!("run-{i}"), &format!("agent-{i}"), 10_000 - i))
                .collect(),
        );
        let fleet = build_fleet_status(Some(&live), &mut key_state, Some(SESSION), capacity(0, 0));

        assert_eq!(MAX_FLEET_CANDIDATES, 256);
        assert_eq!(fleet["totalActive"], serde_json::json!(300));
        assert_eq!(
            agents(&fleet)[0],
            "agent-255",
            "the oldest COLLECTED candidate opens the window; agent-299 would mean the \
             candidate cap did not apply: {fleet}"
        );
    }

    /// `:274-294`'s three display bounds, each on the field it guards: `MAX_AGENT_LENGTH` (96),
    /// `MAX_METADATA_LENGTH` (128) on `model`/`effort`, and `MAX_GOAL_LENGTH` (512).
    ///
    /// These are a wire contract, not cosmetics: the fleet document goes to a client that has to
    /// size a row for it, and an unbounded `goal` is a caller-supplied string.
    #[test]
    fn every_display_field_is_bounded_at_its_own_length() {
        let mut key_state = FleetKeyState::default();
        let live = state(vec![ForegroundControlView {
            current_agent: Some("a".repeat(200)),
            model: Some("m".repeat(300)),
            thinking: Some("e".repeat(300)),
            description: Some("g".repeat(1_000)),
            ..control("run-a", "unused", 10)
        }]);
        let fleet = build_fleet_status(Some(&live), &mut key_state, Some(SESSION), capacity(0, 0));
        let entry = &entries(&fleet)[0];

        assert_eq!(entry["agent"].as_str().unwrap().chars().count(), 96);
        assert_eq!(MAX_AGENT_LENGTH, 96);
        assert_eq!(entry["model"].as_str().unwrap().chars().count(), 128);
        assert_eq!(entry["effort"].as_str().unwrap().chars().count(), 128);
        assert_eq!(MAX_METADATA_LENGTH, 128);
        assert_eq!(entry["goal"].as_str().unwrap().chars().count(), 512);
        assert_eq!(MAX_GOAL_LENGTH, 512);
    }

    /// `displayText` (`rpc.ts:120-124`) runs the value through `sanitizeDisplayText` FIRST, so a
    /// control sequence in a caller-supplied description cannot reach a client's terminal, and a
    /// value that sanitizes to nothing omits its key rather than emitting an empty string.
    #[test]
    fn display_text_sanitizes_before_it_truncates_and_omits_an_empty_result() {
        assert_eq!(
            display_text(Some("hello\u{1b}[31mworld"), MAX_AGENT_LENGTH).as_deref(),
            Some("hello world"),
            "the CSI escape is consumed, leaving the separator pi's `appendSpace` records"
        );
        assert_eq!(display_text(Some("   "), MAX_AGENT_LENGTH), None);
        assert_eq!(display_text(None, MAX_AGENT_LENGTH), None);
    }

    /// `:276` — a candidate with no displayable agent is dropped from `entries` but was ALREADY
    /// counted in `totalActive` at `:196`, so it surfaces as an `omitted`. Silently dropping it
    /// from both would tell a delegating host it has spare capacity it does not have.
    #[test]
    fn an_undisplayable_candidate_is_omitted_rather_than_uncounted() {
        let mut key_state = FleetKeyState::default();
        let live = state(vec![
            ForegroundControlView {
                current_agent: Some("\u{1b}[2J".to_string()),
                ..control("run-bad", "unused", 10)
            },
            control("run-good", "alpha", 20),
        ]);
        let fleet = build_fleet_status(Some(&live), &mut key_state, Some(SESSION), capacity(0, 0));

        assert_eq!(fleet["totalActive"], serde_json::json!(2));
        assert_eq!(entries(&fleet).len(), 1);
        assert_eq!(agents(&fleet), vec!["alpha"]);
        assert_eq!(fleet["omitted"], serde_json::json!(1));
    }

    /// `publicTokens` (`rpc.ts:126-152`), specifically its `total = max(input + output, total)`
    /// reconciliation at `:148`: a counter that disagrees with its own components is repaired
    /// upward, never published as a total smaller than the parts it is made of.
    #[test]
    fn public_tokens_reconciles_a_total_that_disagrees_with_its_parts() {
        assert_eq!(
            public_tokens(Some(TokenTotals {
                input: 5,
                output: 7,
                total: 3,
            })),
            serde_json::json!({ "input": 5, "output": 7, "total": 12 }),
            "a too-small recorded total is raised to input + output"
        );
        assert_eq!(
            public_tokens(Some(TokenTotals {
                input: 1,
                output: 1,
                total: 50,
            })),
            serde_json::json!({ "input": 1, "output": 1, "total": 50 }),
            "a larger recorded total is kept"
        );
        assert_eq!(
            public_tokens(Some(TokenTotals {
                input: u64::MAX,
                output: 9,
                total: 0,
            }))["total"],
            serde_json::json!(u64::MAX),
            "the add saturates rather than wrapping"
        );
        assert_eq!(
            public_tokens(None),
            serde_json::json!({ "input": 0, "output": 0, "total": 0 })
        );
    }

    /// The foreground half's two arms (`:201-218`): a run with parallel children contributes ONE
    /// candidate per child, and a run without them contributes itself. A child-bearing run that
    /// contributed only itself would under-report a fanout's whole cost.
    #[test]
    fn a_run_with_parallel_children_contributes_one_candidate_per_child() {
        let mut key_state = FleetKeyState::default();
        let live = state(vec![ForegroundControlView {
            active_children: vec![
                ForegroundChildView {
                    index: 0,
                    agent: "first".to_string(),
                    started_at: 10,
                    tokens: Some(40),
                    ..ForegroundChildView::default()
                },
                ForegroundChildView {
                    index: 1,
                    agent: "second".to_string(),
                    started_at: 20,
                    ..ForegroundChildView::default()
                },
            ],
            ..control("run-parent", "unused", 5)
        }]);
        let fleet = build_fleet_status(Some(&live), &mut key_state, Some(SESSION), capacity(0, 0));

        assert_eq!(fleet["totalActive"], serde_json::json!(2));
        assert_eq!(agents(&fleet), vec!["first", "second"]);
        assert!(
            !fleet.to_string().contains("run-parent"),
            "the parent's run id must not ride out on a child's row: {fleet}"
        );
        // The scalar-token view widens to `{0, 0, n}`, which `publicTokens` renders as `n`.
        assert_eq!(entries(&fleet)[0]["tokens"]["total"], serde_json::json!(40));
    }

    /// The ASYNC half's session filter (`:221-222`): a tracked job belonging to another instance's
    /// session is not a member of THIS fleet, however live it is. The per-cwd async root is shared,
    /// so this filter is the only thing separating two cyrup processes' rosters.
    #[test]
    fn a_tracked_job_from_another_session_is_not_a_fleet_member() {
        let mut key_state = FleetKeyState::default();
        let live = FleetState {
            current_session_id: Some(SESSION.to_string()),
            tracked_jobs: vec![
                run_view("mineaaaaaaaaaaaa", Some(SESSION), 100),
                run_view("theirsbbbbbbbbbb", Some(OTHER_SESSION), 100),
            ],
            ..FleetState::default()
        };
        let fleet = build_fleet_status(Some(&live), &mut key_state, Some(SESSION), capacity(1, 4));

        assert_eq!(fleet["totalActive"], serde_json::json!(1));
        assert_eq!(entries(&fleet).len(), 1);
        assert!(
            !fleet.to_string().contains("theirs"),
            "another session's run must not appear at all: {fleet}"
        );
        assert_eq!(
            fleet["topLevelAsyncCapacity"],
            serde_json::json!({ "used": 1, "limit": 4 }),
            "an OPEN gate publishes the capacity the caller measured"
        );
    }
}
