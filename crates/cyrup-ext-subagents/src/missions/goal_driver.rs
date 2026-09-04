//! Turn-end goal continuation notices — a 1:1 port of
//! `pi-subagents/src/missions/goal-driver.ts` (162 lines @v0.43.0).
//!
//! A **goal mission** is a mission with `goal.status === "active"` and a token budget. At the end
//! of every orchestrator turn, [`collect_goal_continuation_notices`] asks, for each goal mission
//! owned by the current session: *is this mission idle, un-exhausted, and still open?* If so it
//! emits a `needs_attention` control notice naming the mission's **next ready action** — so a
//! long-running objective keeps being picked back up instead of stalling the moment its last
//! child exits.
//!
//! The scan is deliberately read-mostly but not read-only: it refreshes each goal mission's linked
//! run statuses from their `status.json` files first ([`refresh_goal_mission`], upstream
//! `refreshGoalMission`), because "is a run still live" is exactly the question the notice hinges
//! on and a stale record would answer it wrong.
//!
//! # Where the next ready action comes from
//!
//! In priority order (`nextReadyAction`, `goal-driver.ts:105-116`):
//!
//! 1. A `nextReadyAction`/`nextAction` string, or a `status: "ready"` node's
//!    `action`/`task`/`title`/`summary`, found by a bounded (depth ≤ 8) search of the mission's
//!    `state.json` ([`super::workflow_state`]'s file).
//! 2. The recommendation on the first OPEN decision, or `Resolve decision: <title>`.
//! 3. `Inspect linked run <id> and continue the mission` when the latest run failed or paused.
//! 4. `Continue objective: <objective>`.
//!
//! …and if a RETAINED CHILD matches the latest run, the whole thing is wrapped in
//! `Resume retained child <runId> (<agent>) for: <action>`.
//!
//! # [CYRUP-DELTA] `RetainedChild` has no producer in cyrup yet
//!
//! [`RetainedChild`] is ported here as an input type. Its upstream producer is
//! `listRetainedChildren` (`runs/background/retained-children.ts:34-55`), which selects async runs
//! having a `parentWorkflowRunId` and exactly one step — i.e. children of a **`workflowScript`**
//! run. cyrup has no `workflowScript` runtime (the identifier appears nowhere in this crate), so
//! no cyrup async run can carry that field and the list is necessarily empty here; the production
//! call site passes `&[]` and gains the retained-resume wrapping for free the day the
//! `workflowScript` port lands. The retained-child logic itself is ported and tested below against
//! synthesized input, so it is the CALL that needs changing then, not this module.

use std::path::Path;

use serde_json::Value;

use super::store::{list_missions, read_mission, update_mission};
use super::workflow_state::mission_state_path;
use super::{
    MissionDecisionStatus, MissionGoalStatus, MissionRecord, MissionResult, MissionRunLink,
    MissionStatus, MissionStoreLocation, MissionTokenUsage, MissionUpdateInput,
};

/// pi `ACTIVE_RUN_STATUSES` (`goal-driver.ts:10`).
const ACTIVE_RUN_STATUSES: [&str; 3] = ["queued", "running", "active"];

/// pi `MAX_ACTION_LENGTH` (`goal-driver.ts:11`).
const MAX_ACTION_LENGTH: usize = 180;

/// The `readyActionFromValue` recursion bound (`goal-driver.ts:63`).
const MAX_STATE_SEARCH_DEPTH: usize = 8;

/// pi `RetainedChild` (`runs/background/retained-children.ts:8-16`) — a completed child whose
/// session file survives, so a follow-up can RESUME it rather than starting fresh.
///
/// Only the three fields this module reads are modelled; see the module's `[CYRUP-DELTA]` note on
/// why nothing in cyrup produces one yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedChild {
    /// The retained child's own run id.
    pub run_id: String,
    /// The workflow run that spawned it, when it had one.
    pub parent_run_id: Option<String>,
    /// The agent persona it ran.
    pub agent: String,
}

/// pi `GoalContinuationNotice` (`goal-driver.ts:13-17`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalContinuationNotice {
    /// The mission the notice is about.
    pub mission_id: String,
    /// The rendered notice body — four lines, see [`collect_goal_continuation_notices`].
    pub message: String,
    /// The control event that carries it to the transcript.
    pub event: crate::exec::control::ControlEvent,
}

/// pi `bounded` (`goal-driver.ts:19-22`): collapse all whitespace runs to single spaces, trim,
/// then ellipsize with `…` (U+2026, ONE character) at [`MAX_ACTION_LENGTH`].
fn bounded(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > MAX_ACTION_LENGTH {
        let head: String = normalized.chars().take(MAX_ACTION_LENGTH - 1).collect();
        format!("{head}\u{2026}")
    } else {
        normalized
    }
}

/// pi `tokenUsage` (`goal-driver.ts:24-28`): `{ total: <non-negative safe integer> }`.
fn token_usage(value: Option<&Value>) -> Option<u64> {
    let total = value?.as_object()?.get("total")?.as_i64()?;
    // `Number.isSafeInteger(total) && total >= 0` — the upper bound is JS's 2^53-1.
    (0..=9_007_199_254_740_991)
        .contains(&total)
        .then(|| total.unsigned_abs())
}

/// pi `readLinkedRun` (`goal-driver.ts:30-49`): re-read a linked run's `status.json` and project
/// its `state` (plus a `completedAt` stamp and any token total) back onto the run link.
///
/// Upstream THROWS when the file exists but is missing `state`; that propagates out of
/// `collectGoalContinuationNotices` to `extension/index.ts:598`'s catch, which logs and abandons
/// the whole scan. This reproduces that (an `Err` from here aborts the scan), because the
/// alternative — silently treating a malformed status as "no change" — would let a live run look
/// idle and generate a spurious continuation notice.
fn read_linked_run(run: &MissionRunLink) -> MissionResult<MissionRunLink> {
    let Some(async_dir) = run.async_dir.as_deref() else {
        return Ok(run.clone());
    };
    let status_path = Path::new(async_dir).join("status.json");
    if !status_path.exists() {
        return Ok(run.clone());
    }
    let raw = std::fs::read_to_string(&status_path)
        .map_err(|err| super::MissionError::io(&status_path, err))?;
    let status: Value =
        serde_json::from_str(&raw).map_err(|err| super::MissionError::invalid(err.to_string()))?;
    let state = status
        .get("state")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            super::MissionError::invalid(format!(
                "Linked run status '{}' is missing state",
                status_path.display()
            ))
        })?;
    let tokens = token_usage(status.get("totalTokens")).or_else(|| {
        status.get("steps").and_then(Value::as_array).map(|steps| {
            steps
                .iter()
                .map(|step| {
                    step.as_object()
                        .and_then(|s| token_usage(s.get("tokens")))
                        .unwrap_or(0)
                })
                .sum()
        })
    });
    Ok(MissionRunLink {
        status: Some(state.to_string()),
        completed_at: if !ACTIVE_RUN_STATUSES.contains(&state) && run.completed_at.is_none() {
            Some(super::format_iso8601_millis(crate::time::now_epoch_millis()))
        } else {
            run.completed_at.clone()
        },
        usage: tokens.map_or(run.usage, |tokens| Some(MissionTokenUsage { tokens })),
        ..run.clone()
    })
}

/// pi `refreshGoalMission` (`goal-driver.ts:51-60`): re-read every linked run, and persist only if
/// SOMETHING changed. A goal mission whose runs all settled goes (back) to `active`, not to a
/// terminal status — that is what keeps it eligible for the next turn's notice.
fn refresh_goal_mission(
    location: &MissionStoreLocation,
    record: MissionRecord,
) -> MissionResult<MissionRecord> {
    let runs = record
        .runs
        .iter()
        .map(read_linked_run)
        .collect::<MissionResult<Vec<_>>>()?;
    if runs == record.runs {
        return Ok(record);
    }
    let active = runs.iter().any(|run| {
        run.status
            .as_deref()
            .is_some_and(|s| ACTIVE_RUN_STATUSES.contains(&s))
    });
    let status = if active || record.goal.is_some() {
        MissionStatus::Active
    } else {
        record.status
    };
    update_mission(
        location,
        &record.id,
        &MissionUpdateInput {
            status: Some(status),
            add_runs: runs,
            ..Default::default()
        },
        crate::time::now_epoch_millis(),
        None,
    )
}

/// One node of a mission `state.json`, decoded in **file order**.
///
/// # Why this exists instead of [`serde_json::Value`]
///
/// `readyActionFromValue`'s last resort is `for (const [key, child] of Object.entries(input))`
/// (`goal-driver.ts:81`), and `Object.entries` on a `JSON.parse` result yields keys in INSERTION
/// order — the order they appear in the file. A [`serde_json::Map`] is a [`std::collections::BTreeMap`]
/// unless the `preserve_order` feature is on (it is not, anywhere in this workspace: `serde_json`
/// has no `indexmap` dependency in `Cargo.lock`), so iterating one walks keys ALPHABETICALLY.
///
/// That silently reordered the search: a `state.json` of
/// `{"zeta": {"nextAction": "Z"}, "alpha": {"nextAction": "A"}}` answered `"A"` here and `"Z"`
/// upstream. The precedence this function's own doc claims to pin — explicit keys, then the
/// `status: "ready"` fallback, then descent — is only half the contract; WHICH sibling is
/// descended into first is the other half, and it was wrong.
///
/// This enum is decoded straight from the file by a `visit_map` that appends entries as they
/// arrive, so descent order is the file's. Duplicate keys follow `JSON.parse` + `Object.entries`
/// exactly: the LAST value wins, at the FIRST occurrence's position.
///
/// Only the shapes `readyActionFromValue` inspects are retained — strings, arrays and objects.
/// Every other scalar collapses into [`StateNode::Other`], because upstream's `!value ||
/// typeof value !== "object"` guard returns `undefined` for all of them without looking further.
#[derive(Debug, PartialEq)]
enum StateNode {
    /// `null`, a boolean or a number — never inspected, only skipped.
    Other,
    /// A JSON string (upstream reads these off `nextReadyAction`/`status`/`task`/…).
    Str(String),
    /// A JSON array, in order.
    Array(Vec<StateNode>),
    /// A JSON object's entries, in FILE order.
    Object(Vec<(String, StateNode)>),
}

impl StateNode {
    /// `input[key]` — the entry for `key`, or `None` when this node is not an object.
    fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(entries) => entries
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// `typeof input[key] === "string" ? input[key] : undefined`.
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text.as_str()),
            _ => None,
        }
    }
}

impl<'de> serde::Deserialize<'de> for StateNode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Accepts any JSON value; keeps object entries in arrival (file) order.
        struct NodeVisitor;

        impl<'de> serde::de::Visitor<'de> for NodeVisitor {
            type Value = StateNode;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(StateNode::Other)
            }

            fn visit_bool<E: serde::de::Error>(self, _value: bool) -> Result<Self::Value, E> {
                Ok(StateNode::Other)
            }

            fn visit_i64<E: serde::de::Error>(self, _value: i64) -> Result<Self::Value, E> {
                Ok(StateNode::Other)
            }

            fn visit_u64<E: serde::de::Error>(self, _value: u64) -> Result<Self::Value, E> {
                Ok(StateNode::Other)
            }

            fn visit_f64<E: serde::de::Error>(self, _value: f64) -> Result<Self::Value, E> {
                Ok(StateNode::Other)
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StateNode::Str(value.to_string()))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(StateNode::Str(value))
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(StateNode::Other)
            }

            fn visit_some<D: serde::Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                deserializer.deserialize_any(self)
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(StateNode::Array(items))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(String, StateNode)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, StateNode>()? {
                    // `JSON.parse` keeps the LAST duplicate's value, and `Object.entries` reports
                    // that key once, at its FIRST position.
                    match entries.iter_mut().find(|(existing, _)| *existing == key) {
                        Some(slot) => slot.1 = value,
                        None => entries.push((key, value)),
                    }
                }
                Ok(StateNode::Object(entries))
            }
        }

        deserializer.deserialize_any(NodeVisitor)
    }
}

/// pi `readyActionFromValue` (`goal-driver.ts:62-86`) — the bounded search of `state.json` for a
/// declared next action.
///
/// Note the ORDER within an object: the two explicit keys first, then the `status: "ready"`
/// fallback (which yields a positional description when the node names no action of its own), and
/// only then a recursive descent into the node's own values — **in file order**, which is what
/// [`StateNode`] exists to preserve.
fn ready_action_from_value(value: &StateNode, path_label: &str, depth: usize) -> Option<String> {
    if depth > MAX_STATE_SEARCH_DEPTH {
        return None;
    }
    match value {
        StateNode::Array(items) => items.iter().enumerate().find_map(|(index, item)| {
            ready_action_from_value(item, &format!("{path_label}[{index}]"), depth + 1)
        }),
        StateNode::Object(entries) => {
            for key in ["nextReadyAction", "nextAction"] {
                if let Some(found) = value
                    .get(key)
                    .and_then(StateNode::as_str)
                    .filter(|s| !s.trim().is_empty())
                {
                    return Some(bounded(found));
                }
            }
            if value.get("status").and_then(StateNode::as_str) == Some("ready") {
                for key in ["action", "task", "title", "summary"] {
                    if let Some(found) = value
                        .get(key)
                        .and_then(StateNode::as_str)
                        .filter(|s| !s.trim().is_empty())
                    {
                        return Some(bounded(found));
                    }
                }
                return Some(format!("Continue ready mission state at {path_label}"));
            }
            entries.iter().find_map(|(key, child)| {
                ready_action_from_value(child, &format!("{path_label}.{key}"), depth + 1)
            })
        }
        // pi's guard is `!value || typeof value !== "object"`, so every scalar returns undefined.
        StateNode::Other | StateNode::Str(_) => None,
    }
}

/// pi `missionStateAction` (`goal-driver.ts:88-97`): the state file first, the first OPEN decision
/// second.
fn mission_state_action(
    location: &MissionStoreLocation,
    record: &MissionRecord,
) -> MissionResult<Option<String>> {
    let state_path = mission_state_path(location, &record.id)?;
    if state_path.exists() {
        // Upstream lets a malformed state file throw out of the whole scan; same here.
        let raw = std::fs::read_to_string(&state_path)
            .map_err(|err| super::MissionError::io(&state_path, err))?;
        let value: StateNode = serde_json::from_str(&raw)
            .map_err(|err| super::MissionError::invalid(err.to_string()))?;
        if let Some(action) = ready_action_from_value(&value, "state", 0) {
            return Ok(Some(action));
        }
    }
    Ok(record
        .decisions
        .iter()
        .find(|item| item.status == MissionDecisionStatus::Open)
        .map(|decision| {
            bounded(
                decision
                    .recommendation
                    .clone()
                    .unwrap_or_else(|| format!("Resolve decision: {}", decision.title))
                    .as_str(),
            )
        }))
}

/// pi `retainedResumeTarget` (`goal-driver.ts:99-103`): the retained child matching the LATEST
/// linked run, either as that run itself or as its workflow parent.
fn retained_resume_target<'a>(
    record: &MissionRecord,
    retained_children: &'a [RetainedChild],
) -> Option<&'a RetainedChild> {
    let latest_run = record.runs.last()?;
    retained_children.iter().find(|child| {
        child.run_id == latest_run.run_id
            || child.parent_run_id.as_deref() == Some(latest_run.run_id.as_str())
    })
}

/// pi `nextReadyAction` (`goal-driver.ts:105-116`).
fn next_ready_action(
    location: &MissionStoreLocation,
    record: &MissionRecord,
    retained_children: &[RetainedChild],
) -> MissionResult<String> {
    let state_action = mission_state_action(location, record)?;
    let latest_run = record.runs.last();
    let action = match state_action {
        Some(action) => action,
        None => match latest_run
            .and_then(|run| run.status.as_deref())
            .filter(|s| matches!(*s, "failed" | "paused"))
        {
            Some(_) => format!(
                "Inspect linked run {} and continue the mission",
                latest_run.map_or("", |run| run.run_id.as_str())
            ),
            None => format!("Continue objective: {}", record.objective),
        },
    };
    Ok(match retained_resume_target(record, retained_children) {
        Some(retained) => format!(
            "Resume retained child {} ({}) for: {}",
            retained.run_id,
            retained.agent,
            bounded(&action)
        ),
        None => bounded(&action),
    })
}

/// pi `collectGoalContinuationNotices` (`goal-driver.ts:118-162`) — the turn-end scan.
///
/// A mission produces a notice only if ALL of the following hold: it is owned by
/// `owner_session_id`, it has a `goal`, its status is not terminal, its goal status is `active`
/// (so a PAUSED or BUDGET-EXHAUSTED goal is silent), it has a budget, its usage has not reached
/// that budget, and it has no run in `queued`/`running`/`active`.
///
/// The `runId` on the emitted event is `goal-<missionId>-turn-<turnId>` — deliberately unique per
/// turn, so a mission that stays idle across turns raises a fresh, non-deduplicated notice each
/// time rather than being suppressed after the first.
///
/// # Errors
///
/// Propagates any failure from reading/refreshing a mission or its linked run statuses. Upstream's
/// caller (`extension/index.ts:597-599`) catches and logs; so does this port's call site.
pub fn collect_goal_continuation_notices(
    location: &MissionStoreLocation,
    owner_session_id: &str,
    retained_children: &[RetainedChild],
    turn_id: u64,
    now: Option<i64>,
) -> MissionResult<Vec<GoalContinuationNotice>> {
    let mut notices = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for listed in list_missions(location).records {
        if listed.owner_session_id.as_deref() != Some(owner_session_id)
            || listed.goal.is_none()
            || listed.status.is_terminal()
        {
            continue;
        }
        let mut record = refresh_goal_mission(location, read_mission(location, &listed.id)?)?;
        let Some(goal) = record.goal else { continue };
        if goal.status != MissionGoalStatus::Active {
            continue;
        }
        let Some(budget) = record.budget else {
            continue;
        };
        if record.usage.map_or(0, |u| u.tokens) >= budget.tokens {
            // A no-op update whose sole purpose is to re-run `updateMission`'s budget-exhaustion
            // transition and persist it (`goal-driver.ts:132`).
            record = update_mission(
                location,
                &record.id,
                &MissionUpdateInput {
                    usage: Some(record.usage.unwrap_or(MissionTokenUsage { tokens: 0 })),
                    ..Default::default()
                },
                crate::time::now_epoch_millis(),
                None,
            )?;
            if record.goal.map(|g| g.status) == Some(MissionGoalStatus::BudgetExhausted) {
                continue;
            }
        }
        if record.runs.iter().any(|run| {
            run.status
                .as_deref()
                .is_some_and(|s| ACTIVE_RUN_STATUSES.contains(&s))
        }) {
            continue;
        }
        let Some(budget) = record.budget else {
            continue;
        };
        if !seen.insert(record.id.clone()) {
            continue;
        }
        let budget_tokens = budget.tokens;
        let used = record.usage.map_or(0, |u| u.tokens);
        let remaining = budget_tokens.saturating_sub(used);
        let message = [
            format!("Goal mission needs attention: {}", bounded(&record.title)),
            format!("Mission: {}", record.id),
            format!("Remaining budget: {remaining} tokens ({used}/{budget_tokens} used)"),
            format!(
                "Next ready action: {}",
                next_ready_action(location, &record, retained_children)?
            ),
        ]
        .join("\n");
        notices.push(GoalContinuationNotice {
            mission_id: record.id.clone(),
            event: crate::exec::control::ControlEvent {
                event_type: crate::registration::ControlEventType::NeedsAttention,
                from: None,
                to: crate::background::ActivityState::NeedsAttention,
                ts: now.unwrap_or_else(crate::time::now_epoch_millis),
                run_id: format!("goal-{}-turn-{turn_id}", record.id),
                agent: "goal mission".to_string(),
                index: None,
                message: message.clone(),
                reason: Some(crate::exec::control::ControlEventReason::Idle),
                turns: None,
                tokens: None,
                tool_count: None,
                current_tool: None,
                current_tool_duration_ms: None,
                current_path: None,
                elapsed_ms: None,
                recent_failure_summary: None,
            },
            message,
        });
    }
    Ok(notices)
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
    use crate::missions::store::{create_mission, resolve_mission_store_location};
    use crate::missions::workflow_state::create_mission_workflow_state;
    use crate::missions::{
        MissionCreateInput, MissionDecisionInput, MissionRunMode, MissionTokenBudget,
    };

    fn location(root: &Path) -> MissionStoreLocation {
        resolve_mission_store_location(root, None, Some(&root.join("agent")))
    }

    fn goal_mission(
        loc: &MissionStoreLocation,
        title: &str,
        budget: u64,
        owner: &str,
    ) -> MissionRecord {
        create_mission(
            loc,
            &MissionCreateInput {
                title: title.to_string(),
                objective: format!("keep working on {title}"),
                goal: Some(true),
                budget: Some(MissionTokenBudget { tokens: budget }),
                status: Some(MissionStatus::Active),
                labels: None,
                owner_session_id: Some(owner.to_string()),
            },
            0,
            None,
        )
        .unwrap()
    }

    #[test]
    fn an_idle_goal_mission_raises_one_notice_naming_its_objective() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Ship the parser", 1000, "sess-1");

        let notices =
            collect_goal_continuation_notices(&loc, "sess-1", &[], 7, Some(12_345)).unwrap();
        assert_eq!(notices.len(), 1);
        let notice = &notices[0];
        assert_eq!(notice.mission_id, record.id);
        assert_eq!(
            notice.message,
            format!(
                "Goal mission needs attention: Ship the parser\nMission: {}\nRemaining budget: \
                 1000 tokens (0/1000 used)\nNext ready action: Continue objective: keep working \
                 on Ship the parser",
                record.id
            )
        );
        assert_eq!(notice.event.run_id, format!("goal-{}-turn-7", record.id));
        assert_eq!(notice.event.agent, "goal mission");
        assert_eq!(notice.event.ts, 12_345);
        assert_eq!(
            notice.event.event_type,
            crate::registration::ControlEventType::NeedsAttention
        );
        assert_eq!(
            notice.event.to,
            crate::background::ActivityState::NeedsAttention
        );
        assert_eq!(
            notice.event.reason,
            Some(crate::exec::control::ControlEventReason::Idle)
        );
        assert_eq!(notice.event.message, notice.message);
    }

    #[test]
    fn the_run_id_changes_per_turn_so_a_notice_is_not_deduplicated_away() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Repeat", 1000, "sess-1");
        let first = collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0)).unwrap();
        let second = collect_goal_continuation_notices(&loc, "sess-1", &[], 2, Some(0)).unwrap();
        assert_eq!(first[0].event.run_id, format!("goal-{}-turn-1", record.id));
        assert_eq!(second[0].event.run_id, format!("goal-{}-turn-2", record.id));
        assert_ne!(first[0].event.run_id, second[0].event.run_id);
    }

    #[test]
    fn missions_owned_by_another_session_non_goal_or_terminal_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        goal_mission(&loc, "Other session", 1000, "sess-2");
        create_mission(
            &loc,
            &MissionCreateInput {
                title: "Plain".to_string(),
                objective: "no goal".to_string(),
                status: Some(MissionStatus::Active),
                owner_session_id: Some("sess-1".to_string()),
                ..Default::default()
            },
            0,
            None,
        )
        .unwrap();
        let closed = goal_mission(&loc, "Closed", 1000, "sess-1");
        update_mission(
            &loc,
            &closed.id,
            &MissionUpdateInput {
                status: Some(MissionStatus::Completed),
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();

        assert!(
            collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_paused_goal_is_silent() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Paused", 1000, "sess-1");
        update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                goal: Some(crate::missions::MissionGoalUpdate::Set(
                    crate::missions::MissionGoal {
                        status: MissionGoalStatus::Paused,
                    },
                )),
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        assert!(
            collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_exhausted_budget_transitions_the_goal_and_stops_notifying() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Spent", 100, "sess-1");
        update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_runs: vec![MissionRunLink {
                    run_id: "r".to_string(),
                    mode: MissionRunMode::Single,
                    async_dir: None,
                    child_index: None,
                    agent: None,
                    status: Some("complete".to_string()),
                    started_at: None,
                    completed_at: None,
                    usage: Some(MissionTokenUsage { tokens: 250 }),
                }],
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        assert!(
            collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0))
                .unwrap()
                .is_empty()
        );
        let after = read_mission(&loc, &record.id).unwrap();
        assert_eq!(
            after.goal.unwrap().status,
            MissionGoalStatus::BudgetExhausted
        );
    }

    #[test]
    fn a_live_run_suppresses_the_notice() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Busy", 1000, "sess-1");
        update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_runs: vec![MissionRunLink {
                    run_id: "r".to_string(),
                    mode: MissionRunMode::Single,
                    async_dir: None,
                    child_index: None,
                    agent: None,
                    status: Some("running".to_string()),
                    started_at: None,
                    completed_at: None,
                    usage: None,
                }],
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        assert!(
            collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_linked_runs_status_json_is_refreshed_before_the_idle_test() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Refreshed", 10_000, "sess-1");
        let async_dir = tmp.path().join("async").join("r1");
        std::fs::create_dir_all(&async_dir).unwrap();
        update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_runs: vec![MissionRunLink {
                    run_id: "r1".to_string(),
                    mode: MissionRunMode::Single,
                    async_dir: Some(async_dir.to_string_lossy().into_owned()),
                    child_index: None,
                    agent: None,
                    status: Some("running".to_string()),
                    started_at: None,
                    completed_at: None,
                    usage: None,
                }],
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        // While the status file says running, nothing is raised…
        std::fs::write(async_dir.join("status.json"), r#"{"state":"running"}"#).unwrap();
        assert!(
            collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0))
                .unwrap()
                .is_empty()
        );
        // …and once it settles, the refresh picks it up (and folds in the token total).
        std::fs::write(
            async_dir.join("status.json"),
            r#"{"state":"complete","totalTokens":{"total":42}}"#,
        )
        .unwrap();
        let notices = collect_goal_continuation_notices(&loc, "sess-1", &[], 3, Some(0)).unwrap();
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0]
                .message
                .contains("Remaining budget: 9958 tokens (42/10000 used)")
        );
        let refreshed = read_mission(&loc, &record.id).unwrap();
        assert_eq!(
            refreshed.status,
            MissionStatus::Active,
            "a goal mission stays active"
        );
        assert_eq!(refreshed.runs[0].status.as_deref(), Some("complete"));
        assert!(refreshed.runs[0].completed_at.is_some());
    }

    #[test]
    fn a_failed_latest_run_points_at_inspecting_it() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Broken", 1000, "sess-1");
        update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_runs: vec![MissionRunLink {
                    run_id: "r-bad".to_string(),
                    mode: MissionRunMode::Single,
                    async_dir: None,
                    child_index: None,
                    agent: None,
                    status: Some("failed".to_string()),
                    started_at: None,
                    completed_at: None,
                    usage: None,
                }],
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        let notices = collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0)).unwrap();
        assert!(
            notices[0]
                .message
                .contains("Next ready action: Inspect linked run r-bad and continue the mission"),
            "{}",
            notices[0].message
        );
    }

    #[test]
    fn an_open_decision_outranks_the_objective() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Deciding", 1000, "sess-1");
        update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_decisions: vec![MissionDecisionInput {
                    title: "Which storage engine?".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        let notices = collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0)).unwrap();
        assert!(
            notices[0]
                .message
                .contains("Next ready action: Resolve decision: Which storage engine?"),
            "{}",
            notices[0].message
        );
    }

    /// SUBA-085 — the goal driver's next ready action is `record.decisions.find(open)`
    /// (`goal-driver.ts:94-95` @v0.64.0), so resolving the decision through
    /// `MissionUpdateInput::resolve_decision` (`store.ts:497-508`) is what moves the mission
    /// past it. Pre-fix nothing could flip the status and the same decision was proposed on
    /// every evaluation; now the notice falls through to the objective.
    #[test]
    fn resolving_the_open_decision_moves_the_next_ready_action_past_it() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Deciding", 1000, "sess-1");
        let gated = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_decisions: vec![MissionDecisionInput {
                    title: "Which storage engine?".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        assert_eq!(gated.status, MissionStatus::NeedsDecision);
        let before = collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0)).unwrap();
        assert!(
            before[0]
                .message
                .contains("Next ready action: Resolve decision: Which storage engine?"),
            "{}",
            before[0].message
        );

        let resolved = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                resolve_decision: Some(crate::missions::MissionDecisionResolution {
                    id: gated.decisions[0].id.clone(),
                    resolution: "rocksdb".to_string(),
                }),
                ..Default::default()
            },
            2,
            None,
        )
        .unwrap();
        assert_eq!(resolved.status, MissionStatus::Active);
        let after = collect_goal_continuation_notices(&loc, "sess-1", &[], 2, Some(0)).unwrap();
        assert!(
            after[0]
                .message
                .contains("Next ready action: Continue objective: keep working on Deciding"),
            "{}",
            after[0].message
        );
        assert!(
            !after[0].message.contains("Resolve decision"),
            "{}",
            after[0].message
        );
    }

    #[test]
    fn mission_state_outranks_everything_and_is_searched_by_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Stateful", 1000, "sess-1");
        let mut state = create_mission_workflow_state(&loc, &record.id).unwrap();
        state
            .set(
                "plan",
                serde_json::json!({"phases": [{"status": "done"}, {"status": "ready", "task": "  write   the   docs  "}]}),
            )
            .unwrap();
        let notices = collect_goal_continuation_notices(&loc, "sess-1", &[], 1, Some(0)).unwrap();
        assert!(
            notices[0]
                .message
                .contains("Next ready action: write the docs"),
            "{}",
            notices[0].message
        );
    }

    /// Parse a `state.json` body the way [`mission_state_action`] does — from TEXT, so the
    /// declaration order in the literal is the order under test.
    fn state(json: &str) -> StateNode {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn an_explicit_next_ready_action_key_wins_over_a_ready_node() {
        let value = state(
            r#"{"nested": {"status": "ready", "task": "fallback"}, "nextReadyAction": "explicit"}"#,
        );
        assert_eq!(
            ready_action_from_value(&value, "state", 0).as_deref(),
            Some("explicit")
        );
    }

    #[test]
    fn a_ready_node_with_no_named_action_reports_its_path() {
        let value = state(r#"{"a": {"b": {"status": "ready"}}}"#);
        assert_eq!(
            ready_action_from_value(&value, "state", 0).as_deref(),
            Some("Continue ready mission state at state.a.b")
        );
    }

    #[test]
    fn the_state_search_is_depth_bounded() {
        // 10 levels of nesting: the `status: "ready"` marker sits below the depth-8 bound.
        let mut text = r#"{"status": "ready", "task": "too deep"}"#.to_string();
        for _ in 0..10 {
            text = format!(r#"{{"next": {text}}}"#);
        }
        assert_eq!(ready_action_from_value(&state(&text), "state", 0), None);
    }

    /// `Object.entries` (`goal-driver.ts:81`) walks INSERTION order. `zeta` is declared first, so
    /// `zeta`'s action is the answer — even though `alpha` sorts first.
    ///
    /// This is the case a `serde_json::Map` (a `BTreeMap` without `preserve_order`) got wrong:
    /// it descended alphabetically and returned `"from alpha"`.
    #[test]
    fn the_descent_follows_file_order_not_alphabetical_order() {
        let value = state(
            r#"{"zeta": {"nextAction": "from zeta"}, "alpha": {"nextAction": "from alpha"}}"#,
        );
        assert_eq!(
            ready_action_from_value(&value, "state", 0).as_deref(),
            Some("from zeta")
        );

        // …and the mirror image: swapping only the DECLARATION order swaps only the answer.
        let swapped = state(
            r#"{"alpha": {"nextAction": "from alpha"}, "zeta": {"nextAction": "from zeta"}}"#,
        );
        assert_eq!(
            ready_action_from_value(&swapped, "state", 0).as_deref(),
            Some("from alpha")
        );
    }

    /// The reported PATH label follows the same order, so a positional description names the node
    /// upstream would have named.
    #[test]
    fn the_reported_path_label_follows_file_order() {
        let value = state(r#"{"zulu": {"status": "ready"}, "alfa": {"status": "ready"}}"#);
        assert_eq!(
            ready_action_from_value(&value, "state", 0).as_deref(),
            Some("Continue ready mission state at state.zulu")
        );
    }

    /// `JSON.parse` keeps the LAST duplicate's value and `Object.entries` reports the key once, at
    /// its FIRST position — so `dup` still leads the descent and carries `"second"`.
    #[test]
    fn a_duplicate_key_keeps_the_last_value_at_the_first_position() {
        let value = state(
            r#"{"dup": {"nextAction": "first"}, "later": {"nextAction": "later"}, "dup": {"nextAction": "second"}}"#,
        );
        assert_eq!(
            ready_action_from_value(&value, "state", 0).as_deref(),
            Some("second")
        );
    }

    /// Scalars are skipped without being descended into, and an array is searched in index order.
    #[test]
    fn arrays_are_searched_in_index_order_and_scalars_are_skipped() {
        let value = state(
            r#"{"n": 1, "b": true, "nul": null, "s": "plain", "items": [{"k": 1}, {"nextAction": "second item"}]}"#,
        );
        assert_eq!(
            ready_action_from_value(&value, "state", 0).as_deref(),
            Some("second item")
        );
    }

    #[test]
    fn bounded_collapses_whitespace_and_ellipsizes_at_180_characters() {
        assert_eq!(bounded("  a \n b\tc  "), "a b c");
        let long = "x".repeat(400);
        let result = bounded(&long);
        assert_eq!(result.chars().count(), MAX_ACTION_LENGTH);
        assert!(result.ends_with('\u{2026}'));
    }

    #[test]
    fn a_retained_child_wraps_the_action_in_a_resume_instruction() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = goal_mission(&loc, "Resumable", 1000, "sess-1");
        update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_runs: vec![MissionRunLink {
                    run_id: "wf-1".to_string(),
                    mode: MissionRunMode::Workflow,
                    async_dir: None,
                    child_index: None,
                    agent: None,
                    status: Some("complete".to_string()),
                    started_at: None,
                    completed_at: None,
                    usage: None,
                }],
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        let retained = [RetainedChild {
            run_id: "child-9".to_string(),
            parent_run_id: Some("wf-1".to_string()),
            agent: "scout".to_string(),
        }];
        let notices =
            collect_goal_continuation_notices(&loc, "sess-1", &retained, 1, Some(0)).unwrap();
        assert!(
            notices[0].message.contains(
                "Next ready action: Resume retained child child-9 (scout) for: Continue \
                 objective: keep working on Resumable"
            ),
            "{}",
            notices[0].message
        );
    }
}
