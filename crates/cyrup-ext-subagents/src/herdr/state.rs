//! The pure lifecycle → [`PaneAgentState`] machine.
//!
//! No clock, no I/O, no socket, no `Arc`: every input is a method call and every output is an
//! `Option<`[`StateReport`]`>` that is `Some` **exactly** when the report herdr should see has
//! changed. That is what lets the whole transition table be pinned by unit tests that need
//! neither a herdr nor a socket, and it is what keeps the reporter task (a later file) from
//! opening a connection per tool call — herdr's socket is **one request per connection**
//! (`handle_connection_with_stop` reads one line, answers, and returns,
//! `tmp/herdr/src/api/server.rs:156-317`), so a report is a whole connect/write/read/close and
//! de-duplication is correctness of cost, not an optimisation.
//!
//! # The ordering, and why it is this way round
//!
//! ```text
//! blocked  if a human is being waited on          // parked on the human — highest priority
//! working  else if any run is in flight           // some work is happening
//! idle     otherwise                              // control is the human's
//! ```
//!
//! `blocked` outranks `working` because a cyrup that is blocked is *also* mid-turn — the
//! permission dialog is raised from inside a tool call, so [`Self::edge_depth`] is non-zero the
//! whole time it is up. Ranking `working` first would mean the pane never says `blocked` at all,
//! and `blocked` is the reason this feature exists: it is the signal that tells herdr's sidebar
//! which pane needs the human.
//!
//! # The two states this machine never produces
//!
//! - **`unknown`** — cyrup always knows which of the three it is, so reporting "I cannot tell"
//!   would be a fabrication. [`PaneAgentState::Unknown`] exists because herdr's enum has it; this
//!   model has no path to it.
//! - **`done`** — it is not reportable at all. herdr has two enums:
//!   [`PaneAgentState`] (`tmp/herdr/src/api/schema/common.rs:149-156`) is the four-value set a
//!   client may *report*, and `AgentStatus` (`:158-166`) is the five-value set herdr *derives*
//!   and filters on. `done` lives only in the second, and herdr computes it as *idle and not yet
//!   seen* — `pane_agent_status(Idle, seen: false) => Done`
//!   (`tmp/herdr/src/app/api_helpers.rs:96-107`), with `seen` flipping when the human looks at
//!   the pane's tab. So cyrup reports `idle` and herdr renders "finished while you were looking
//!   away" for free. Sending `"done"` would be a serde failure at herdr and come back
//!   `invalid_request`.
//!
//! # Counting a fleet
//!
//! cyrup is not a single-agent CLI: a turn can have a root agent thinking, several foreground
//! runs in flight, and detached background runners that outlive the process that started them.
//! Three sources, one predicate, each counted the way its own lifetime allows:
//!
//! 1. **The root turn and foreground runs** are *edges* — [`StateModel::agent_start`] /
//!    [`StateModel::agent_end`] and [`StateModel::foreground_run_started`] /
//!    [`StateModel::foreground_run_finished`] — folded into one refcount,
//!    [`Self::edge_depth`]. Refcounted rather than a boolean precisely so **a finishing child
//!    does not flip the pane idle mid-turn**: the pane only needs the sign of the count, and one
//!    of several concurrent runs ending must not take it to zero.
//! 2. **Background runs** are a *level*, [`Self::active_background_runs`], recomputed at every
//!    resync from the fleet projection. A detached runner started by a previous cyrup process is
//!    still going and will never deliver a "started" edge to this one, so an edge count could
//!    never see it; a level always does.
//!
//! Every decrement saturates. `agent_end` without a matching `agent_start` is a shape this
//! codebase really produces (a session resumed mid-turn ends a turn it never saw begin), and an
//! unsigned wrap would pin the pane at `working` **for ever** — herdr never reclaims state from a
//! `cyrup:` source: agent state carries no TTL (`tmp/herdr/src/terminal/state.rs:18-25`) and the
//! process-exit override only fires for agents herdr can name
//! (`tmp/herdr/src/terminal/state.rs:401-407`, and `"cyrup"` is deliberately not one — see
//! [`super::AGENT`]).

use std::collections::BTreeSet;

use cyrup_herdr::schema::PaneAgentState;

use crate::background::RunId;

/// The message reported alongside `blocked` when a human dialog is up.
///
/// Short and decorative by design: `message` is free-form on
/// [`cyrup_herdr::schema::panes::PaneReportAgentParams`], but presentation belongs in
/// `pane.report_metadata` (`socket-api.mdx:717-718`), so this says *why* the pane is blocked and
/// nothing more. See [`super::label`] for the half that may carry text.
pub const HUMAN_WAIT_MESSAGE: &str = "awaiting approval";

/// The fallback reason for a subagent that needs the human but named no reason — pi's own default
/// (`tmp/pi-subagents` @ `v0.68.0` `src/integrations/herdr-status.ts:124,310`,
/// `"subagent needs attention"`).
pub const DEFAULT_ATTENTION_MESSAGE: &str = "subagent needs attention";

/// One `pane.report_agent` payload, minus the identity fields every report shares.
///
/// [`super::SOURCE`], [`super::AGENT`] and the pane id are constant for the life of the process
/// and belong to the reporter, not to the model; `seq` is assigned at **wire** time, not here, so
/// that a state edge overtaking a queued decorative refresh still carries the higher sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateReport {
    /// The semantic state — the field herdr's waits, notifications and rollups key on.
    pub state: PaneAgentState,
    /// A short, free-form reason. `Some` only for [`PaneAgentState::Blocked`].
    pub message: Option<String>,
}

/// One run's attention status as a resync sees it — the input to
/// [`StateModel::resync_attention`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunAttention {
    /// The run this row is about.
    pub run_id: RunId,
    /// Whether the run is currently asking for the human.
    pub needs_attention: bool,
    /// The reason, when the run named one. A control-notice *reason token*, never a rendered
    /// notice body — see [`super::label`] for why that distinction is a privacy rule and not a
    /// style preference.
    pub message: Option<String>,
}

/// The pure state machine.
///
/// Construct one per session, drive it from lifecycle edges, and put every `Some` it returns on
/// the wire. See the [module docs](self) for the ordering and the counting rules.
#[derive(Clone, Debug, Default)]
pub struct StateModel {
    /// The root turn plus every foreground run in flight, as a saturating refcount.
    edge_depth: u32,
    /// Active detached background runs, recomputed as a level at every resync.
    active_background_runs: usize,
    /// Whether a human dialog is up right now, mirrored from the session's
    /// `HumanInteractionLock` (`crates/cyrup-ext/src/host/services.rs`).
    ///
    /// A **level, not a refcount**, because the lock itself is not a refcount: it is a
    /// single-permit `Semaphore`, so at most one human prompt is ever open and "held" is the whole
    /// of the fact. The AUG sketched a `u32`; one permit makes it unnecessary.
    human_waiting: bool,
    /// Runs asking for the human, in the order they asked — **insertion order, deliberately**.
    ///
    /// pi reports the most recently raised label (`[...attentionLabels.values()].at(-1)`,
    /// `herdr-status.ts:265`) off a JS `Map`, which is insertion-ordered. A `BTreeMap<RunId, _>`
    /// would order by run id, i.e. by a random UUID, and "the newest thing asking for you" would
    /// become "whichever run id sorts last" — a silent behaviour change disguised as a container
    /// choice.
    attention: Vec<(RunId, String)>,
    /// Runs whose attention the human has already been shown, suppressed until they ask again.
    acknowledged: BTreeSet<RunId>,
    /// The last report handed out, for de-duplication. `None` means "herdr has not seen anything
    /// from us", which is also what [`Self::invalidate_last_report`] restores after a failed send.
    last_reported: Option<StateReport>,
}

impl StateModel {
    /// A model for a session that has not started a turn: no runs, no waits, nothing reported.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // --- the edges -------------------------------------------------------------------------

    /// The root turn begins — `HostEvent::AgentStart` (`crates/cyrup-ext/src/event.rs:329`).
    ///
    /// Two things happen, both ported from pi's `agentStarted()` (`herdr-status.ts:372-375`):
    /// the refcount rises, and **every outstanding attention is acknowledged and cleared**. A
    /// notice the human has already been shown must not re-block the pane on the next turn; if
    /// the run is still asking, its next notice — or the next resync — raises it again.
    ///
    /// This edge is one cyrup has and pi's subagents bridge does not: pi's pane looks idle while
    /// the host itself is thinking, because only async *children* feed its bridge. cyrup counts
    /// its own turn, so the pane says `working` while cyrup is working.
    /// `[CYRUP-EXCEEDS-UPSTREAM]`
    pub fn agent_start(&mut self) -> Option<StateReport> {
        self.edge_depth = self.edge_depth.saturating_add(1);
        for (run_id, _) in &self.attention {
            self.acknowledged.insert(run_id.clone());
        }
        self.attention.clear();
        self.settle()
    }

    /// The root turn ends — `HostEvent::AgentEnd` (`crates/cyrup-ext/src/event.rs:330`).
    /// Saturating: see the [module docs](self) on why an unmatched end is a real shape.
    pub fn agent_end(&mut self) -> Option<StateReport> {
        self.edge_depth = self.edge_depth.saturating_sub(1);
        self.settle()
    }

    /// A foreground run enters the foreground-control registry.
    ///
    /// **One increment per RUN, never per child.** A parallel run with nine children is one run
    /// in flight; nine increments would still have the right sign but would make the count mean
    /// nothing, and the count is the only thing standing between "a child finished" and "the pane
    /// went idle mid-turn".
    pub fn foreground_run_started(&mut self) -> Option<StateReport> {
        self.edge_depth = self.edge_depth.saturating_add(1);
        self.settle()
    }

    /// A foreground run leaves the registry. Saturating, for the same reason as
    /// [`Self::agent_end`].
    pub fn foreground_run_finished(&mut self) -> Option<StateReport> {
        self.edge_depth = self.edge_depth.saturating_sub(1);
        self.settle()
    }

    /// The number of active detached background runs, as a **level** recomputed at each resync
    /// from the fleet projection (`AsyncRunView::is_active`, i.e. `Queued | Running`).
    pub fn set_active_background_runs(&mut self, active: usize) -> Option<StateReport> {
        self.active_background_runs = active;
        self.settle()
    }

    /// A human dialog opened or closed — the held↔unheld edge of the session's
    /// `HumanInteractionLock` (`crates/cyrup-ext/src/host/services.rs`).
    ///
    /// That lock is **the** authoritative "awaiting the human" signal in cyrup: every companion
    /// that opens a prompt acquires it first — the permission dialog
    /// (`crates/cyrup-permission-system/src/extension/prompt.rs:176`), the ask-forwarder
    /// (`crates/cyrup-permission-system/src/forwarding.rs:1231`), MCP's dialog owner
    /// (`crates/cyrup-mcp/src/owner.rs:659`), intercom's clarify
    /// (`crates/cyrup-intercom/src/seams.rs:369`) and flux's ask tool
    /// (`crates/cyrup-flux/src/ask_tool.rs:185`) — and they all reach the same instance through
    /// `HostServices::human_interaction_lock`. It is a fact, not a heuristic — unlike
    /// [`Self::raise_attention`], which is fed by an idle-threshold rule — which is why it
    /// outranks it when both are up.
    ///
    /// Idempotent by de-duplication: a second `true` while already waiting returns `None`, so an
    /// observer that fires on every change rather than only on the edge cannot produce a
    /// duplicate report.
    pub fn set_human_waiting(&mut self, waiting: bool) -> Option<StateReport> {
        self.human_waiting = waiting;
        self.settle()
    }

    // --- attention -------------------------------------------------------------------------

    /// A run is asking for the human — pi `raiseAttention` (`herdr-status.ts:289-296`).
    ///
    /// A run already raised is left exactly as it is (upstream returns early on
    /// `attentionLabels.has(runId)`), so a repeated notice never relabels the pane. Raising
    /// un-acknowledges the run, so a run that asks again after being seen is shown again.
    pub fn raise_attention(
        &mut self,
        run_id: &RunId,
        message: Option<&str>,
    ) -> Option<StateReport> {
        if self.attention.iter().any(|(id, _)| id == run_id) {
            return None;
        }
        self.acknowledged.remove(run_id);
        self.attention.push((
            run_id.clone(),
            message
                .map(str::to_string)
                .unwrap_or_else(|| DEFAULT_ATTENTION_MESSAGE.to_string()),
        ));
        self.settle()
    }

    /// A run stopped asking, or ended — pi's async-complete arm (`herdr-status.ts:353-361`),
    /// which drops both the label and the acknowledgement.
    pub fn clear_attention(&mut self, run_id: &RunId) -> Option<StateReport> {
        self.attention.retain(|(id, _)| id != run_id);
        self.acknowledged.remove(run_id);
        self.settle()
    }

    /// Rebuild the attention set from the authoritative active-run projection — pi `replaceRuns`'
    /// attention half (`herdr-status.ts:297-320`).
    ///
    /// Acknowledgement semantics, ported rung for rung: a run that is no longer asking drops its
    /// acknowledgement; a run that is asking but has been acknowledged stays suppressed; a run
    /// that has left the active set entirely drops its acknowledgement so a future run of the
    /// same id starts clean. A run already raised keeps the message it was raised with unless
    /// this resync names one.
    pub fn resync_attention(&mut self, runs: &[RunAttention]) -> Option<StateReport> {
        let mut next: Vec<(RunId, String)> = Vec::new();
        let mut active: BTreeSet<RunId> = BTreeSet::new();
        for run in runs {
            active.insert(run.run_id.clone());
            if !run.needs_attention {
                self.acknowledged.remove(&run.run_id);
                continue;
            }
            if self.acknowledged.contains(&run.run_id) {
                continue;
            }
            let message = run
                .message
                .clone()
                .or_else(|| {
                    self.attention
                        .iter()
                        .find(|(id, _)| *id == run.run_id)
                        .map(|(_, label)| label.clone())
                })
                .unwrap_or_else(|| DEFAULT_ATTENTION_MESSAGE.to_string());
            next.push((run.run_id.clone(), message));
        }
        self.acknowledged.retain(|id| active.contains(id));
        self.attention = next;
        self.settle()
    }

    // --- reading ---------------------------------------------------------------------------

    /// The report herdr *should* hold right now, whether or not it has changed. The resync and
    /// TTL-refresh paths read this; the edges above return it only when it differs.
    #[must_use]
    pub fn desired(&self) -> StateReport {
        if self.human_waiting {
            return StateReport {
                state: PaneAgentState::Blocked,
                message: Some(HUMAN_WAIT_MESSAGE.to_string()),
            };
        }
        if let Some((_, message)) = self.attention.last() {
            return StateReport {
                state: PaneAgentState::Blocked,
                message: Some(message.clone()),
            };
        }
        if self.is_running() {
            return StateReport {
                state: PaneAgentState::Working,
                message: None,
            };
        }
        StateReport {
            state: PaneAgentState::Idle,
            message: None,
        }
    }

    /// Whether any work is in flight — the edge refcount or the background level.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.edge_depth > 0 || self.active_background_runs > 0
    }

    /// The last report this model handed out, or `None` if it has handed out none.
    #[must_use]
    pub fn last_reported(&self) -> Option<&StateReport> {
        self.last_reported.as_ref()
    }

    /// Re-emit the desired report even if it has not changed — the TTL-refresh and
    /// post-reconnect path. Always `Some`, unlike every edge above.
    pub fn resend(&mut self) -> Option<StateReport> {
        self.last_reported = None;
        self.settle()
    }

    /// Forget what herdr was told, so the next edge or [`Self::resend`] reports again.
    ///
    /// The reporter calls this when a send **failed**: reporting agent state is best effort and
    /// must never disturb the agent, but de-duplicating against a report herdr never received
    /// would strand the pane on stale state until the next genuine change. Separate from
    /// [`Self::resend`] because a failed send has no report to hand back — the caller is already
    /// holding the one that did not land.
    pub fn invalidate_last_report(&mut self) {
        self.last_reported = None;
    }

    /// Recompute, de-duplicate, and record. `Some` exactly on a change of the `(state, message)`
    /// pair — a changed *reason* under an unchanged `blocked` is a change, because it is what the
    /// human reads.
    fn settle(&mut self) -> Option<StateReport> {
        let next = self.desired();
        if self.last_reported.as_ref() == Some(&next) {
            return None;
        }
        self.last_reported = Some(next.clone());
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn run(token: &str) -> RunId {
        RunId::from_token(token.to_string())
    }

    /// The whole ordering in one table: `blocked` beats `working` beats `idle`, over every
    /// combination of the three inputs. The `(true, true, *) => Blocked` rows are the load-bearing
    /// ones — a blocked cyrup is always *also* mid-turn, so ranking `working` first would mean the
    /// pane never says `blocked` at all.
    #[test]
    fn blocked_outranks_working_outranks_idle() {
        for waiting in [false, true] {
            for running in [false, true] {
                for attention in [false, true] {
                    let mut model = StateModel::new();
                    if running {
                        model.agent_start();
                    }
                    if attention {
                        model.raise_attention(&run("r1"), Some("child asked"));
                    }
                    model.set_human_waiting(waiting);
                    let expected = if waiting || attention {
                        PaneAgentState::Blocked
                    } else if running {
                        PaneAgentState::Working
                    } else {
                        PaneAgentState::Idle
                    };
                    assert_eq!(
                        model.desired().state,
                        expected,
                        "waiting={waiting} running={running} attention={attention}"
                    );
                }
            }
        }
    }

    /// The lock is a fact and the notice is a heuristic, so the lock's reason is the one the
    /// human sees when both are up.
    #[test]
    fn the_human_wait_lock_owns_the_message_when_both_are_raised() {
        let mut model = StateModel::new();
        model.raise_attention(&run("r1"), Some("child asked"));
        model.set_human_waiting(true);
        assert_eq!(model.desired().message.as_deref(), Some(HUMAN_WAIT_MESSAGE));
        model.set_human_waiting(false);
        assert_eq!(model.desired().message.as_deref(), Some("child asked"));
    }

    /// THE refcount test. Two runs in flight, one ends: the pane must not flip idle mid-turn.
    /// Gutting this by making `edge_depth` a boolean, or by counting children instead of runs,
    /// turns the second `foreground_run_finished` into an `Idle` report.
    #[test]
    fn a_finishing_child_does_not_flip_the_pane_idle() {
        let mut model = StateModel::new();
        assert_eq!(
            model.agent_start().map(|r| r.state),
            Some(PaneAgentState::Working)
        );
        assert_eq!(model.foreground_run_started(), None, "already working");
        assert_eq!(model.foreground_run_finished(), None, "still working");
        assert_eq!(
            model.last_reported().map(|r| r.state),
            Some(PaneAgentState::Working)
        );
        assert_eq!(
            model.agent_end().map(|r| r.state),
            Some(PaneAgentState::Idle),
            "the last holder leaving is what ends the turn"
        );
    }

    /// An `AgentEnd` with no matching `AgentStart` is a real shape (a session resumed mid-turn).
    /// An unsigned wrap here would pin the pane at `working` for ever — herdr never reclaims
    /// state from a `cyrup:` source.
    #[test]
    fn an_unmatched_end_saturates_instead_of_wrapping() {
        let mut model = StateModel::new();
        model.agent_end();
        model.foreground_run_finished();
        assert!(!model.is_running());
        assert_eq!(model.desired().state, PaneAgentState::Idle);
        assert_eq!(
            model.agent_start().map(|r| r.state),
            Some(PaneAgentState::Working),
            "one start after two spurious ends must still reach working"
        );
    }

    /// Background runs are a level, not an edge: a detached runner from a previous process shows
    /// up at the first resync with no "started" edge ever delivered.
    #[test]
    fn background_runs_count_as_a_level() {
        let mut model = StateModel::new();
        assert_eq!(
            model.set_active_background_runs(2).map(|r| r.state),
            Some(PaneAgentState::Working)
        );
        assert_eq!(model.set_active_background_runs(1), None, "still working");
        assert_eq!(
            model.set_active_background_runs(0).map(|r| r.state),
            Some(PaneAgentState::Idle)
        );
    }

    /// De-duplication: only a changed `(state, message)` pair goes on the wire. Every report is a
    /// whole socket connection, so this is what keeps the reporter from connecting per tool call.
    #[test]
    fn only_a_changed_pair_is_reported() {
        let mut model = StateModel::new();
        assert!(model.set_human_waiting(true).is_some());
        assert_eq!(model.set_human_waiting(true), None, "duplicate edge");
        assert!(model.set_human_waiting(false).is_some());
        // A changed reason under an unchanged state IS a change: it is what the human reads.
        model.raise_attention(&run("r1"), Some("first"));
        let second = model.raise_attention(&run("r2"), Some("second"));
        assert_eq!(second.and_then(|r| r.message).as_deref(), Some("second"));
    }

    /// `unknown` and `done` have no path out of this model: `unknown` would be a fabrication, and
    /// `done` is not reportable at all — herdr derives it from `idle` + unseen.
    #[test]
    fn unknown_and_done_are_never_produced() {
        let mut model = StateModel::new();
        let mut seen = Vec::new();
        let mut push = |report: Option<StateReport>| {
            if let Some(report) = report {
                seen.push(report.state);
            }
        };
        push(model.agent_start());
        push(model.raise_attention(&run("r1"), None));
        push(model.set_human_waiting(true));
        push(model.set_human_waiting(false));
        push(model.clear_attention(&run("r1")));
        push(model.agent_end());
        push(model.set_active_background_runs(1));
        push(model.set_active_background_runs(0));
        assert!(!seen.is_empty());
        assert!(
            seen.iter().all(|s| matches!(
                s,
                PaneAgentState::Idle | PaneAgentState::Working | PaneAgentState::Blocked
            )),
            "reported {seen:?}"
        );
    }

    /// A new root turn acknowledges everything outstanding and clears it — pi `agentStarted()`.
    /// Without this, a notice the human already saw re-blocks the pane every turn.
    #[test]
    fn a_new_turn_acknowledges_outstanding_attention() {
        let mut model = StateModel::new();
        let r1 = run("r1");
        model.raise_attention(&r1, Some("child asked"));
        assert_eq!(model.desired().state, PaneAgentState::Blocked);
        assert_eq!(
            model.agent_start().map(|r| r.state),
            Some(PaneAgentState::Working),
            "the new turn clears the notice"
        );
        // Still acknowledged: a resync that reports the same run as still asking stays suppressed.
        assert_eq!(
            model.resync_attention(&[RunAttention {
                run_id: r1.clone(),
                needs_attention: true,
                message: Some("child asked".to_string()),
            }]),
            None
        );
        assert_eq!(model.desired().state, PaneAgentState::Working);
        // But a fresh notice from that run raises it again.
        assert_eq!(
            model
                .raise_attention(&r1, Some("child asked again"))
                .map(|r| r.state),
            Some(PaneAgentState::Blocked)
        );
    }

    /// A raise for a run already raised is a no-op — it must never relabel the pane.
    #[test]
    fn a_repeated_raise_never_relabels() {
        let mut model = StateModel::new();
        let r1 = run("r1");
        assert!(model.raise_attention(&r1, Some("first")).is_some());
        assert_eq!(model.raise_attention(&r1, Some("second")), None);
        assert_eq!(model.desired().message.as_deref(), Some("first"));
    }

    /// The raise with no reason takes pi's own default sentence.
    #[test]
    fn an_unexplained_notice_gets_pis_default_reason() {
        let mut model = StateModel::new();
        model.raise_attention(&run("r1"), None);
        assert_eq!(
            model.desired().message.as_deref(),
            Some(DEFAULT_ATTENTION_MESSAGE)
        );
    }

    /// Insertion order, not run-id order: the newest run asking is the one the human is told
    /// about. `r1` sorts before `r2`, so a key-ordered container would answer "first" here.
    #[test]
    fn the_newest_raise_is_the_one_reported() {
        let mut model = StateModel::new();
        model.raise_attention(&run("r2"), Some("second"));
        model.raise_attention(&run("r1"), Some("first"));
        assert_eq!(model.desired().message.as_deref(), Some("first"));
    }

    /// `resync_attention` ports pi's `replaceRuns` acknowledgement rungs: not-asking drops the
    /// acknowledgement, and a run leaving the active set drops it too.
    #[test]
    fn a_resync_rebuilds_attention_and_prunes_acknowledgements() {
        let mut model = StateModel::new();
        let r1 = run("r1");
        model.raise_attention(&r1, Some("asked"));
        model.agent_start(); // acknowledges r1
        model.agent_end();

        // Still asking, still acknowledged => suppressed.
        model.resync_attention(&[RunAttention {
            run_id: r1.clone(),
            needs_attention: true,
            message: None,
        }]);
        assert_eq!(model.desired().state, PaneAgentState::Idle);

        // Stops asking => the acknowledgement is dropped...
        model.resync_attention(&[RunAttention {
            run_id: r1.clone(),
            needs_attention: false,
            message: None,
        }]);
        // ...so asking again raises.
        assert_eq!(
            model
                .resync_attention(&[RunAttention {
                    run_id: r1.clone(),
                    needs_attention: true,
                    message: Some("asked again".to_string()),
                }])
                .map(|r| r.state),
            Some(PaneAgentState::Blocked)
        );

        // Leaving the active set entirely clears the row.
        assert_eq!(
            model.resync_attention(&[]).map(|r| r.state),
            Some(PaneAgentState::Idle)
        );
    }

    /// A resync that names no reason keeps the reason the run was raised with.
    #[test]
    fn a_resync_keeps_an_existing_reason() {
        let mut model = StateModel::new();
        let r1 = run("r1");
        model.raise_attention(&r1, Some("the original reason"));
        model.resync_attention(&[RunAttention {
            run_id: r1,
            needs_attention: true,
            message: None,
        }]);
        assert_eq!(
            model.desired().message.as_deref(),
            Some("the original reason")
        );
    }

    /// A failed send must not be de-duplicated away: the reporter forgets what herdr was told and
    /// the next edge reports again.
    #[test]
    fn an_invalidated_report_is_sent_again() {
        let mut model = StateModel::new();
        let first = model.set_human_waiting(true).expect("first report");
        assert_eq!(model.set_human_waiting(true), None, "de-duplicated");

        // Through the DE-DUPLICATOR, not through `resend`. `resend` clears `last_reported` on its
        // own way in, so it answers `Some` whether or not the invalidate happened — which is how
        // this test used to pass with `invalidate_last_report` gutted to a no-op. What the
        // invalidate actually buys is that an UNCHANGED edge after a failed send is sent again
        // instead of being swallowed, and that is what the reporter depends on.
        model.invalidate_last_report();
        assert_eq!(
            model.set_human_waiting(true),
            Some(first.clone()),
            "a report herdr never received must be re-sent on the next edge"
        );
        assert_eq!(
            model.set_human_waiting(true),
            None,
            "and once it HAS landed, the de-duplicator is back in force"
        );
        assert_eq!(model.resend(), Some(first));
    }

    /// The TTL-refresh path re-emits even when nothing changed.
    #[test]
    fn resend_repeats_an_unchanged_report() {
        let mut model = StateModel::new();
        model.agent_start();
        assert_eq!(
            model.resend().map(|r| r.state),
            Some(PaneAgentState::Working)
        );
    }
}
