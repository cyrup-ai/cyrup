//! SUBA-093 — the FLAT status-index layout of a run's step list: pi's `flatIndex`
//! (`src/runs/background/subagent-runner.ts:1294,2612-2705` @v0.64.0).
//!
//! A run has two index spaces, and before this module they were silently the same integer:
//!
//! * the **top-level step index** — the position of a [`RunnerStep`] in
//!   [`crate::background::runner_main::RunnerConfig::steps`], which is what
//!   [`crate::background::runner_main`]'s step loop advances its cursor through;
//! * the **flat status index** — the position of one CHILD in `RunStatus::steps`, which is what
//!   every per-child artifact of a run is keyed by: the `subagent.step.*`/`subagent.child-status`
//!   `stepIndex`, `output-<i>.log` ([`crate::background::RunPaths::step_output_log`]), the per-step
//!   steer inbox/ack/capability directories, the child's intercom presence label, the live
//!   telemetry fold, and — the reason this module exists — the `childId` a child-scoped `stop`
//!   resolves against ([`crate::background::child_identity`]).
//!
//! Upstream flattens a `ParallelGroup` at DECLARATION: one status step per member, each with its
//! own `flatIndex`, so `markChildStopRequested`/`stopChildStep` address one member
//! (`subagent-runner.ts:2618-2652` builds the per-task entries, `:4221,:4268` uses `fi` per
//! member). cyrup published ONE `<parallel:N tasks>` entry for the whole group, so a `tasks[]`
//! fan-out's members had no live per-child status and `childId: "step:0"` resolved to the group —
//! stopping all of it. The functions here are that flatten, kept pure (no I/O, no clock, no
//! status handle) so the layout can be pinned by plain unit tests and so every consumer derives
//! the same mapping instead of re-deriving it inline.
//!
//! # What is NOT flattened (and why)
//!
//! A [`RunnerStep::DynamicGroup`] occupies exactly ONE flat slot, which is also what upstream
//! declares for it (`subagent-runner.ts:2656-2670` pushes a single `expand:<agent>` entry and
//! `flatStepCount++`). Upstream then SPLICES that one entry into one-per-materialized-item when
//! the fan-out actually expands (`:4155`), shifting every later group's `start`; cyrup does not,
//! because a dynamic group's width is only known at dispatch time and the splice would move the
//! flat base of every later step mid-run. That is a recorded SUBA-093 residual, not an oversight:
//! a dynamic group therefore stays addressable as a single child, exactly as it is today.

use std::ops::Range;

use crate::background::StepStatus;
use crate::spawn::chain_graph::RunnerStep;

/// How many entries `step` contributes to `RunStatus::steps` — pi's `flatStepCount` increment per
/// declared step (`subagent-runner.ts:2617-2705` @v0.64.0).
///
/// A [`RunnerStep::ParallelGroup`] contributes one per member (`for (const task of step.parallel)`,
/// `:2622`), so a zero-width group contributes NOTHING, exactly as upstream's loop does. Every
/// other shape contributes exactly one.
#[must_use]
pub fn flat_step_width(step: &RunnerStep) -> usize {
    match step {
        RunnerStep::ParallelGroup(group) => group.steps.len(),
        RunnerStep::SingleStep(_)
        | RunnerStep::DynamicGroup(_)
        | RunnerStep::ImportAsyncRoot(_) => 1,
    }
}

/// The flat status index the step at `top_index` starts at — pi's `flatStepCount` at the moment
/// that step was declared (`subagent-runner.ts:2620,2657,2672`).
///
/// `top_index` past the end of `steps` yields the total width (the position a step appended by
/// [`crate::background::control::ChainAppendRequest`] would take), which is why appends never
/// disturb an already-computed base: they only ever extend the tail.
#[must_use]
pub fn flat_base(steps: &[RunnerStep], top_index: usize) -> usize {
    steps
        .iter()
        .take(top_index)
        .map(flat_step_width)
        .sum::<usize>()
}

/// The half-open flat-index range the step at `top_index` owns. Empty for a zero-width
/// [`RunnerStep::ParallelGroup`] and for a `top_index` past the end of `steps`.
#[must_use]
pub fn flat_range(steps: &[RunnerStep], top_index: usize) -> Range<usize> {
    let base = flat_base(steps, top_index);
    let width = steps.get(top_index).map_or(0, flat_step_width);
    base..base + width
}

/// The total number of `RunStatus::steps` entries `steps` declares — pi's `initialFlatStepCount`
/// (`subagent-runner.ts:2613`).
#[must_use]
pub fn flat_total(steps: &[RunnerStep]) -> usize {
    steps.iter().map(flat_step_width).sum()
}

/// The freshly declared, `Pending` [`StepStatus`] entries one [`RunnerStep`] contributes to
/// `RunStatus::steps` — pi's per-shape `initialStatusSteps.push` (`subagent-runner.ts:2618-2705`).
///
/// A `ParallelGroup` yields one entry per member, named by that member's OWN agent (`agent:
/// task.agent`, `:2626`) rather than the synthesized `<parallel:N tasks>` label the collapsed
/// single entry carried; the group label survives on the run's `events.jsonl` lines and its
/// `SingleResult`, which stay one-per-top-level-step
/// ([`crate::background::runner_main`]'s `step_display_agent`). A `DynamicGroup` keeps cyrup's
/// `<dynamic:<collect>>` label on its single entry — upstream's placeholder is `expand:<agent>`
/// (`:2659`), a rename with no behavioural content that would churn this crate's renderers, so it
/// is deliberately not taken here.
#[must_use]
pub fn pending_step_statuses_for(step: &RunnerStep) -> Vec<StepStatus> {
    match step {
        RunnerStep::SingleStep(spec) => vec![StepStatus::pending(spec.agent.clone())],
        RunnerStep::ImportAsyncRoot(spec) => vec![StepStatus::pending(spec.agent.clone())],
        RunnerStep::ParallelGroup(group) => group
            .steps
            .iter()
            .map(|task| StepStatus::pending(task.agent.clone()))
            .collect(),
        RunnerStep::DynamicGroup(dynamic) => {
            vec![StepStatus::pending(format!(
                "<dynamic:{}>",
                dynamic.collect
            ))]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spawn::chain_graph::{DynamicGroupSpec, OnEmpty, ParallelGroupSpec, SingleStepSpec};

    fn spec(agent: &str) -> SingleStepSpec {
        SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.to_string(),
            task: "t".to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        }
    }

    fn single(agent: &str) -> RunnerStep {
        RunnerStep::SingleStep(spec(agent))
    }

    fn group(agents: &[&str]) -> RunnerStep {
        RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: agents.iter().map(|a| spec(a)).collect(),
            concurrency: 4,
            fail_fast: false,
            worktree: false,
        })
    }

    fn dynamic() -> RunnerStep {
        RunnerStep::DynamicGroup(DynamicGroupSpec {
            expand: "outputs.plan".to_string(),
            item: None,
            key: None,
            max_items: None,
            on_empty: OnEmpty::Skip,
            template: Box::new(spec("worker")),
            collect: "results".to_string(),
            collect_schema: None,
            concurrency: 4,
            fail_fast: false,
            acceptance: None,
        })
    }

    /// pi `flatStepCount` per shape (`subagent-runner.ts:2617-2705` @v0.64.0): a parallel group is
    /// as wide as its member list; every other shape is exactly one.
    #[test]
    fn a_parallel_group_is_as_wide_as_its_member_list_and_every_other_shape_is_one() {
        assert_eq!(flat_step_width(&single("a")), 1);
        assert_eq!(flat_step_width(&dynamic()), 1);
        assert_eq!(flat_step_width(&group(&["a", "b", "c"])), 3);
        assert_eq!(flat_step_width(&group(&[])), 0);
    }

    /// The two index spaces are genuinely different: top-level step 2 of
    /// `[single, parallel(3), single]` starts at flat index 4.
    #[test]
    fn flat_base_accumulates_group_widths_ahead_of_the_cursor() {
        let steps = vec![single("a"), group(&["x", "y", "z"]), single("b")];
        assert_eq!(flat_base(&steps, 0), 0);
        assert_eq!(flat_base(&steps, 1), 1);
        assert_eq!(flat_base(&steps, 2), 4);
        assert_eq!(flat_total(&steps), 5);
        assert_eq!(flat_range(&steps, 1), 1..4);
        assert_eq!(flat_range(&steps, 2), 4..5);
    }

    /// A base past the end is the append position, so `append_steps` never has to renumber
    /// anything already published.
    #[test]
    fn a_base_past_the_end_is_the_append_position_and_its_range_is_empty() {
        let steps = vec![single("a"), group(&["x", "y"])];
        assert_eq!(flat_base(&steps, 9), 3);
        assert_eq!(flat_range(&steps, 9), 3..3);
    }

    /// SUBA-093's headline: a `tasks[]` fan-out publishes one status entry PER MEMBER, named by
    /// that member's own agent — not one `<parallel:N tasks>` entry for the whole group.
    #[test]
    fn a_parallel_group_publishes_one_pending_entry_per_member_named_by_its_own_agent() {
        let statuses = pending_step_statuses_for(&group(&["scout", "builder", "critic"]));
        let agents: Vec<&str> = statuses.iter().map(|s| s.agent.as_str()).collect();
        assert_eq!(agents, ["scout", "builder", "critic"]);
        assert!(
            statuses
                .iter()
                .all(|s| s.status == crate::background::StepState::Pending)
        );
    }

    /// A dynamic group stays ONE entry (upstream declares one placeholder too); a single step is
    /// named by its own agent.
    #[test]
    fn a_dynamic_group_and_a_single_step_each_publish_exactly_one_entry() {
        let dyn_agents: Vec<String> = pending_step_statuses_for(&dynamic())
            .into_iter()
            .map(|s| s.agent)
            .collect();
        assert_eq!(dyn_agents, ["<dynamic:results>".to_string()]);
        let single_agents: Vec<String> = pending_step_statuses_for(&single("solo"))
            .into_iter()
            .map(|s| s.agent)
            .collect();
        assert_eq!(single_agents, ["solo".to_string()]);
    }
}
