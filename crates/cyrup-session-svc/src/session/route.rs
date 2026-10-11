//! SUBA-229 — where a custom message goes: pi's `sendCustomMessage` decision as one pure function.
//!
//! The functional core shared by [`super::AgentSession::send_custom_message`] and the injection
//! pump's busy phase ([`super::AgentSession::queue_turn_injections_onto_live_run`]). Both `match`
//! on [`Route`] exhaustively, with no catch-all arm, so a new variant is a compile error at every
//! caller instead of a silent default. Before it existed the two disagreed: `send_custom_message`
//! queued a turn-triggering message onto a live run (pi's branch 2), while the pump held every
//! such message until the session was idle and then started a run of its own.

use crate::event::DeliverAs;

/// One arm per branch of pi's `sendCustomMessage`, plus pi's branch 3 named on its own.
///
/// pi (`packages/coding-agent/src/core/agent-session.ts:1947-1969` @v0.87.1; the same branches at
/// `:2306-2328` @v1.1.0), quoted:
///
/// ```ts
/// if (options?.deliverAs === "nextTurn") {
///     this._pendingNextTurnMessages.push(appMessage);                       // NextTurn
/// } else if (this.isStreaming && options?.triggerTurn !== false) {
///     if (options?.deliverAs === "followUp") {
///         this.agent.followUp(appMessage);                                   // FollowUp
///     } else {
///         this.agent.steer(appMessage);                                      // Steer
///     }
/// } else if (options?.triggerTurn) {
///     if (this._isEmittingAgentSettled) {
///         this._deferredSettledActions.push(async () => await this._runAgentPrompt(appMessage));
///         return;
///     }
///     await this._runAgentPrompt(appMessage);                                // RunTurn
/// } else if (this.isStreaming) {
///     this._pendingCustomMessages.push(appMessage);                          // Defer
/// } else {
///     this._appendCustomMessage(appMessage);                                 // AppendIdle
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// Branch 1, `deliverAs: "nextTurn"`: staged to ride the next prompt, whatever else is set.
    NextTurn,
    /// Branch 2 with `deliverAs: "followUp"`: `agent.followUp` on the live run.
    FollowUp,
    /// Branch 2 with any other `deliverAs`: `agent.steer` on the live run.
    Steer,
    /// Branch 3: not streaming and `triggerTurn` truthy, so the message starts a run of its own.
    RunTurn,
    /// Branch 4: streaming and `triggerTurn === false`: deferred to the turn boundary.
    Defer,
    /// Branch 5: not streaming and no trigger: appended to the session now.
    AppendIdle,
}

/// pi's `sendCustomMessage` decision ([`Route`]), pure.
///
/// - `deliver_as` is pi's `options?.deliverAs`.
/// - `trigger_turn` is pi's RAW `options?.triggerTurn`: branch 2 tests `!== false` and branch 3
///   tests truthiness, so an absent key and an explicit `false` take different branches
///   (SEAM-127) and the type keeps all three values.
/// - `run_active` is pi's `isStreaming`, which is the session latch `_isAgentRunActive` and so
///   includes the post-`agent_end` gap ([`super::AgentSession::is_run_active`], AGENT-030).
#[must_use]
pub(crate) fn route(
    deliver_as: Option<DeliverAs>,
    trigger_turn: Option<bool>,
    run_active: bool,
) -> Route {
    // Each line is one pi branch, in pi's order; the guard is that branch's condition.
    let queues_onto_run = run_active && trigger_turn != Some(false);
    match deliver_as {
        Some(DeliverAs::NextTurn) => Route::NextTurn,
        Some(DeliverAs::FollowUp) if queues_onto_run => Route::FollowUp,
        Some(DeliverAs::Steer) | None if queues_onto_run => Route::Steer,
        Some(DeliverAs::Steer | DeliverAs::FollowUp) | None => {
            if trigger_turn == Some(true) {
                Route::RunTurn
            } else if run_active {
                Route::Defer
            } else {
                Route::AppendIdle
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::{Route, route};
    use crate::event::DeliverAs;

    /// [`route`] over EVERY input: four `deliverAs` values (pi's absent key and its three strings),
    /// three `triggerTurn` values (absent, `false`, `true`) and both run states — 24 cases. The
    /// expected table is pi's `sendCustomMessage` (`agent-session.ts:1947-1969` @v0.87.1) evaluated
    /// by hand. Kills: any reordering of the branches; `triggerTurn` collapsed to a bool (absent and
    /// `false` would agree); `followUp` routed as a steer; `nextTurn` losing to a live run.
    #[test]
    fn route_is_pis_send_custom_message_decision_for_every_input() {
        use Route::{AppendIdle, Defer, FollowUp, NextTurn, RunTurn, Steer};
        let deliver = [
            None,
            Some(DeliverAs::Steer),
            Some(DeliverAs::FollowUp),
            Some(DeliverAs::NextTurn),
        ];
        let trigger = [None, Some(false), Some(true)];
        // expected[deliver][trigger] = (idle, streaming)
        let expected = [
            [(AppendIdle, Steer), (AppendIdle, Defer), (RunTurn, Steer)],
            [(AppendIdle, Steer), (AppendIdle, Defer), (RunTurn, Steer)],
            [
                (AppendIdle, FollowUp),
                (AppendIdle, Defer),
                (RunTurn, FollowUp),
            ],
            [
                (NextTurn, NextTurn),
                (NextTurn, NextTurn),
                (NextTurn, NextTurn),
            ],
        ];
        let mut cases = 0;
        for (d, deliver_as) in deliver.into_iter().enumerate() {
            for (t, trigger_turn) in trigger.into_iter().enumerate() {
                let (idle, streaming) = expected[d][t];
                assert_eq!(
                    route(deliver_as, trigger_turn, false),
                    idle,
                    "idle, deliverAs {deliver_as:?}, triggerTurn {trigger_turn:?}"
                );
                assert_eq!(
                    route(deliver_as, trigger_turn, true),
                    streaming,
                    "streaming, deliverAs {deliver_as:?}, triggerTurn {trigger_turn:?}"
                );
                cases += 2;
            }
        }
        assert_eq!(cases, 24, "every combination is checked");
    }
}
