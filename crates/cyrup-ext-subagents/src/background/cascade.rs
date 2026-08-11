//! Ancestor→descendant control cascade for background (async) runs — pi
//! `src/runs/background/subagent-runner.ts:1535-1594` @v0.34.0
//! (`interruptNestedAsyncDescendants` / `timeoutNestedAsyncDescendants`), called from
//! `interruptRunner` (`:2026`) and `timeoutRunner` (`:2061`).
//!
//! # The gap this closes
//!
//! A background run is a DETACHED OS process in its own process group
//! (`spawn/mod.rs`'s `command.process_group(0)`, and `background/spawn_detached.rs` for the hop-1
//! runner itself). Detachment is deliberate and non-negotiable — it is exactly what lets a
//! background run outlive the orchestrator that started it (R-SA-070/071). But it also means the
//! OS gives an ancestor NO leverage over a descendant: signalling this runner's process group
//! does not reach a background grandchild, because that grandchild left the group by design the
//! moment it was spawned.
//!
//! So before this module existed, interrupting a background run stopped only that run: it marked
//! its own steps `Paused`, wrote its own `status.json`, and returned — while every background run
//! it had itself spawned kept burning tokens indefinitely, unreachable and unattributable. The
//! deeper the tree, the worse: a depth-3 fan-out could leave dozens of live processes behind a
//! single "stopped" run. The same held for a deadline expiry.
//!
//! # The mechanism
//!
//! The nested-run registry ([`crate::spawn::nested_events`]) already knows every descendant: its
//! id, its live/queued `state`, its `async_dir`, and its `pid`. And every background runner
//! already watches its own control inbox (`background/control.rs`, R-SA-082). So the cascade is
//! purely a matter of walking the registry and dropping the SAME control-inbox request file into
//! each live descendant's own directory that an external caller would drop — no new transport, no
//! IPC, no signal that could not cross the process-group boundary anyway. Each descendant then
//! runs the identical local interrupt/timeout path it would run for a directly-addressed request,
//! and cascades onward to ITS own descendants, so a single request at the root walks the whole
//! subtree one hop at a time.
//!
//! Delivery is best-effort per target and never fatal to the caller: an unreachable or
//! already-gone descendant is reported as a failure record for the run's event log (pi's
//! `subagent.nested.interrupt_failed` / `subagent.nested.timeout_failed` events) and the walk
//! continues to the next target. Failing to reach one descendant must never prevent reaching the
//! rest, and must never turn an interrupt into an error.

use std::path::PathBuf;

use crate::spawn::nested_events::{
    NestedRoute, NestedRunSummary, project_nested_events, resolve_nested_async_dir,
};

use super::control;

/// Which control-inbox verb the cascade delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeVerb {
    /// Soft, resumable pause — `control/interrupt.json` plus the best-effort `SIGUSR2` wake-up
    /// (pi `deliverInterruptRequest`, source `"ancestor-interrupt"`).
    Interrupt,
    /// Terminal deadline failure — `control/timeout.json`, no signal (pi `deliverTimeoutRequest`,
    /// source `"ancestor-timeout"`).
    Timeout,
    /// G77 — explicit terminal stop: `control/stop.json`, no signal (pi
    /// `stopNestedAsyncDescendants`, `subagent-runner.ts:2281-2310` @v0.43.0, which calls
    /// `deliverStopRequest({ asyncDir, pid, source: "ancestor-stop" })` for every `running`/`queued`
    /// descendant and logs `subagent.nested.stop_failed` per unreachable one). Distinct from
    /// [`Self::Timeout`] for the same reason the two verbs are distinct at the local level: a
    /// stopped subtree ends `Stopped` and is not resumable, a timed-out one ends `Failed`.
    Stop,
}

impl CascadeVerb {
    /// pi's literal `source` string stamped into the delivered request.
    #[must_use]
    pub fn source(self) -> &'static str {
        match self {
            Self::Interrupt => "ancestor-interrupt",
            Self::Timeout => "ancestor-timeout",
            Self::Stop => "ancestor-stop",
        }
    }

    /// The event type appended to the ancestor's own `events.jsonl` for a delivery that failed
    /// (pi `subagent.nested.interrupt_failed` / `subagent.nested.timeout_failed`).
    #[must_use]
    pub fn failure_event_type(self) -> &'static str {
        match self {
            Self::Interrupt => "subagent.nested.interrupt_failed",
            Self::Timeout => "subagent.nested.timeout_failed",
            Self::Stop => "subagent.nested.stop_failed",
        }
    }
}

/// One descendant the cascade could not reach. `target_run_id` is `None` for a failure that
/// happened before any individual target was selected (i.e. the registry projection itself
/// failed), matching pi's two distinct `*_failed` event shapes — one with `targetRunId`, one
/// without.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CascadeFailure {
    /// The descendant run id, when the failure is attributable to one.
    pub target_run_id: Option<String>,
    /// The underlying error text.
    pub message: String,
}

/// What one [`cascade_to_nested_async_descendants`] call actually did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CascadeReport {
    /// Run ids that received the request file.
    pub delivered: Vec<String>,
    /// Per-target (or registry-wide) failures, for the caller's event log.
    pub failures: Vec<CascadeFailure>,
}

/// Flatten the descendant tree in pi's `nestedRuns` generator order: each child, then that
/// child's own children, then the children hanging off each of its steps.
///
/// Both nesting axes matter and neither is redundant: `children` carries runs a descendant
/// spawned directly, while `steps[].children` carries runs spawned by an individual chain STEP of
/// that descendant — a chain of three steps that each fan out has its grandchildren only on the
/// second axis. Walking one and not the other silently spares half the subtree.
fn flatten_nested_runs(children: &[NestedRunSummary], out: &mut Vec<NestedRunSummary>) {
    for child in children {
        out.push(child.clone());
        if let Some(grandchildren) = &child.children {
            flatten_nested_runs(grandchildren, out);
        }
        if let Some(steps) = &child.steps {
            for step in steps {
                if let Some(step_children) = &step.children {
                    flatten_nested_runs(step_children, out);
                }
            }
        }
    }
}

/// Whether a projected descendant is still worth addressing (pi: `if (run.state !== "running" &&
/// run.state !== "queued") continue;`). A `queued` descendant is included deliberately — it has a
/// live runner process that has not yet started its first step, and skipping it would let exactly
/// the runs that are about to start the most work escape the cascade.
fn is_live_state(state: &str) -> bool {
    matches!(state, "running" | "queued")
}

/// The descendant's own run directory, subject to the same containment check every other control
/// op in this crate applies.
///
/// [CYRUP-DELTA] pi writes `run.asyncDir ?? resolveNestedAsyncDir(rootRunId, run)`, i.e. it trusts
/// the registry's raw `asyncDir` string first and only containment-checks it as a fallback. cyrup
/// uses the containment-checked resolution as the SOLE source: `resolve_nested_async_dir` accepts
/// any `async_dir` that lives inside `<nested-runs>/<root_run_id>/<run_id>`, which every
/// legitimately registered descendant does, so the two agree for every real target — but a
/// registry entry whose `asyncDir` points outside that subtree is skipped here rather than being
/// handed a control-request write at an attacker-chosen path. The nested-event sink is written by
/// descendant processes, so its contents are exactly the kind of input `validate_contains_root`
/// exists for.
fn target_dir(root_run_id: &str, run: &NestedRunSummary) -> Option<PathBuf> {
    resolve_nested_async_dir(root_run_id, run)
}

/// Deliver `verb` to every live nested async descendant reachable through `route`.
///
/// Never returns an error: every failure — a registry that will not project, a descendant whose
/// directory cannot be resolved, an unwritable inbox — becomes a [`CascadeFailure`] in the report
/// so the caller can log it and carry on. An interrupt must not be downgraded into a failed run
/// because one grandchild was already gone.
pub async fn cascade_to_nested_async_descendants(
    route: &NestedRoute,
    verb: CascadeVerb,
) -> CascadeReport {
    let mut report = CascadeReport::default();

    let registry = match project_nested_events(route) {
        Ok(registry) => registry,
        Err(err) => {
            report.failures.push(CascadeFailure {
                target_run_id: None,
                message: err.to_string(),
            });
            return report;
        }
    };

    let mut runs = Vec::new();
    flatten_nested_runs(&registry.children, &mut runs);

    for run in runs {
        if !is_live_state(&run.state) {
            continue;
        }
        let Some(dir) = target_dir(&route.root_run_id, &run) else {
            continue;
        };
        // `pid` is projected as i64 from the descendant's own status record; a non-positive value
        // is not addressable and is treated as "unknown pid" (file inbox only), never cast into a
        // bogus signal target.
        let pid = run
            .pid
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0);

        let delivered = match verb {
            CascadeVerb::Interrupt => {
                control::deliver_interrupt_request(&dir, pid, verb.source(), None).await
            }
            CascadeVerb::Timeout => {
                control::deliver_timeout_request(&dir, verb.source(), None).await
            }
            // G77: like `Timeout`, no wake-up signal — upstream's `deliverStopRequest` body is a
            // bare `requestAsyncStop(...)` (`runs/background/control-channel.ts:600`) despite its input shape
            // accepting a `pid`, so `pid` is deliberately unused on this arm.
            CascadeVerb::Stop => control::deliver_stop_request(&dir, verb.source(), None).await,
        };
        match delivered {
            Ok(_) => report.delivered.push(run.id.clone()),
            Err(err) => report.failures.push(CascadeFailure {
                target_run_id: Some(run.id.clone()),
                message: err.to_string(),
            }),
        }
    }

    report
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

    fn summary(id: &str, state: &str) -> NestedRunSummary {
        NestedRunSummary {
            id: id.to_string(),
            parent_run_id: "root".to_string(),
            parent_step_index: None,
            parent_agent: None,
            depth: 0,
            path: Vec::new(),
            async_dir: None,
            pid: None,
            session_id: None,
            session_file: None,
            intercom_target: None,
            owner_intercom_target: None,
            leaf_intercom_target: None,
            owner_state: None,
            control_inbox: None,
            capability_token: None,
            mode: None,
            state: state.to_string(),
            agent: None,
            agents: None,
            current_step: None,
            chain_step_count: None,
            activity_state: None,
            last_activity_at: None,
            current_tool: None,
            current_tool_started_at: None,
            current_path: None,
            turn_count: None,
            tool_count: None,
            total_tokens: None,
            total_cost: None,
            started_at: None,
            ended_at: None,
            last_update: None,
            error: None,
            steps: None,
            children: None,
        }
    }

    #[test]
    fn flatten_walks_both_nesting_axes_in_pi_order() {
        let mut grandchild_via_children = summary("gc-children", "running");
        grandchild_via_children.children = Some(vec![summary("ggc", "running")]);

        let mut step = crate::spawn::nested_events::NestedStepSummary {
            agent: "a".to_string(),
            status: "running".to_string(),
            session_file: None,
            activity_state: None,
            last_activity_at: None,
            current_tool: None,
            current_tool_started_at: None,
            current_path: None,
            turn_count: None,
            tool_count: None,
            started_at: None,
            ended_at: None,
            error: None,
            children: None,
        };
        step.children = Some(vec![summary("gc-step", "running")]);

        let mut child = summary("child", "running");
        child.children = Some(vec![grandchild_via_children]);
        child.steps = Some(vec![step]);

        let mut out = Vec::new();
        flatten_nested_runs(&[child], &mut out);
        let ids: Vec<&str> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["child", "gc-children", "ggc", "gc-step"]);
    }

    #[test]
    fn only_running_and_queued_states_are_addressed() {
        assert!(is_live_state("running"));
        assert!(is_live_state("queued"));
        // G77: `"stopped"` joins the dead set — a stopped descendant is terminal and must not be
        // re-targeted by a later cascade (pi's own `run.state !== "running" && run.state !==
        // "queued"` guard, `subagent-runner.ts:2296`).
        for dead in ["complete", "failed", "paused", "stopped", "cancelled", ""] {
            assert!(!is_live_state(dead), "{dead} must not be addressed");
        }
    }

    #[test]
    fn verb_sources_and_failure_events_match_pi_literals() {
        assert_eq!(CascadeVerb::Interrupt.source(), "ancestor-interrupt");
        assert_eq!(CascadeVerb::Timeout.source(), "ancestor-timeout");
        // G77 — pi `stopNestedAsyncDescendants` (`subagent-runner.ts:2281-2311`).
        assert_eq!(CascadeVerb::Stop.source(), "ancestor-stop");
        assert_eq!(
            CascadeVerb::Interrupt.failure_event_type(),
            "subagent.nested.interrupt_failed"
        );
        assert_eq!(
            CascadeVerb::Timeout.failure_event_type(),
            "subagent.nested.timeout_failed"
        );
        assert_eq!(
            CascadeVerb::Stop.failure_event_type(),
            "subagent.nested.stop_failed"
        );
    }
}
