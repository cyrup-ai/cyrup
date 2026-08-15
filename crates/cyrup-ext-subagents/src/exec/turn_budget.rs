//! Per-run child assistant-TURN budgets — a 1:1 port of
//! `pi-subagents/src/runs/shared/turn-budget.ts` @v0.43.0 (98 lines, ported whole).
//!
//! A turn budget is two numbers:
//!
//! * `maxTurns` — the SOFT limit. The first assistant turn at or past it earns the child a one-time
//!   wrap-up note in the run's recent output and flips `wrapUpRequested`; nothing is killed.
//! * `graceTurns` — how many further assistant turns are tolerated after the soft limit before the
//!   child is aborted. Omitted normalizes to [`DEFAULT_TURN_BUDGET_GRACE_TURNS`] (= 1).
//!
//! **The mechanism is PARENT-side, and this is the one place the filed item (`SUBA-008`) was
//! wrong.** Its Fix line says to mirror `exec/tool_budget.rs`'s "env-handoff shape". There is no
//! env handoff: `git grep -n TURN_BUDGET v0.43.0 -- src/` matches only `turn-budget.ts` itself, and
//! upstream never ships a `PI_SUBAGENT_TURN_BUDGET`. The child is *told* about the budget through
//! [`append_turn_budget_system_prompt`] — a system-prompt block, not an env var — and the budget is
//! *enforced* by the supervising process counting the child's assistant `message_end` events off
//! its NDJSON stdout (`execution.ts:910-924`) and signalling it down when the hard limit passes.
//! The tool budget is the opposite shape (env var + child-side refusal, `tool-budget.ts:70-80`),
//! which is exactly why the two must not be built the same way.
//!
//! The decision at each turn is [`turn_budget_decision`], and it deliberately has a "defer" arm:
//! upstream will NOT abort a child in the middle of tool work, because killing between a tool call
//! and its result throws away the work rather than the turn. The run then ends with
//! `termination-deferred` and a note saying so, rather than a kill.

use serde::{Deserialize, Serialize};

/// pi `DEFAULT_TURN_BUDGET_GRACE_TURNS` (`turn-budget.ts:3`).
pub const DEFAULT_TURN_BUDGET_GRACE_TURNS: u64 = 1;

/// pi `ResolvedTurnBudget` (`shared/types.ts:113-116`): a budget with `graceTurns` defaulted, which
/// is the only form anything downstream of [`resolve_turn_budget_config`] ever sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTurnBudget {
    /// pi `maxTurns` — the SOFT limit, an integer >= 1.
    pub max_turns: u64,
    /// pi `graceTurns` — additional assistant turns tolerated past the soft limit, >= 0.
    pub grace_turns: u64,
}

/// pi `TurnBudgetOutcome` (`shared/types.ts:140`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TurnBudgetOutcome {
    /// pi `"within-budget"` — the child is below `maxTurns`.
    WithinBudget,
    /// pi `"wrap-up-requested"` — at or past `maxTurns`, still inside the grace window.
    WrapUpRequested,
    /// pi `"termination-deferred"` — past the hard limit, but tool work was in flight so the abort
    /// was withheld.
    TerminationDeferred,
    /// pi `"exceeded"` — past the hard limit and aborted.
    Exceeded,
}

/// pi `TurnBudgetState extends ResolvedTurnBudget` (`shared/types.ts:142-148`).
///
/// The three `*_at_turn` fields are `Optional` upstream and are omitted from the wire when unset —
/// `skip_serializing_if` reproduces that, so a `status.json` written by a run with no turn budget
/// is byte-identical to one written before this type existed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnBudgetState {
    /// pi's spread of `ResolvedTurnBudget` — `maxTurns`.
    pub max_turns: u64,
    /// pi's spread of `ResolvedTurnBudget` — `graceTurns`.
    pub grace_turns: u64,
    /// pi `outcome`.
    pub outcome: TurnBudgetOutcome,
    /// pi `turnCount` — assistant turns observed when this state was stamped.
    pub turn_count: u64,
    /// pi `wrapUpRequestedAtTurn?` — always `maxTurns` when set, upstream's own literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap_up_requested_at_turn: Option<u64>,
    /// pi `terminationDeferredAtTurn?` — the FIRST turn a deferral happened, carried forward across
    /// later deferrals (`execution.ts:773-777` passes the previous value back in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_deferred_at_turn: Option<u64>,
    /// pi `exceededAtTurn?`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exceeded_at_turn: Option<u64>,
}

/// pi `resolveTurnBudgetConfig` (`turn-budget.ts:5-24`): validate a raw JSON value into a
/// [`ResolvedTurnBudget`], or return upstream's own error string verbatim.
///
/// `Ok(None)` is pi's `{}` return for `raw === undefined`. Every rejection is `Err(message)` with
/// `label` interpolated exactly as upstream does, so a frontmatter rejection and a tool-param
/// rejection read differently — the caller chooses the label.
///
/// Upstream's `unknownField` scan is ported literally, including that it reports only the FIRST
/// unknown key (`Object.keys(raw).find(...)`), and `Object.keys` order is insertion order for
/// string keys, which `serde_json`'s `preserve_order` map iteration also gives.
///
/// # Errors
/// Returns pi's own validation message when the value is not a plain object, carries any key other
/// than `maxTurns`/`graceTurns`, has a `maxTurns` that is not an integer >= 1, or has a
/// `graceTurns` that is present and not an integer >= 0.
pub fn resolve_turn_budget_config(
    raw: Option<&serde_json::Value>,
    label: &str,
) -> Result<Option<ResolvedTurnBudget>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    // pi `if (!raw || typeof raw !== "object" || Array.isArray(raw))`. JS `null` is `typeof
    // "object"` but falsy, so it takes this arm too — `Value::Null` therefore belongs here, NOT on
    // the `raw === undefined` fast path above.
    let Some(obj) = raw.as_object() else {
        return Err(format!(
            "{label} must be an object with maxTurns and optional graceTurns."
        ));
    };
    if let Some(unknown) = obj
        .keys()
        .find(|key| key.as_str() != "maxTurns" && key.as_str() != "graceTurns")
    {
        return Err(format!("{label}.{unknown} is not supported."));
    }

    let max_turns = match obj.get("maxTurns").and_then(as_non_negative_integer) {
        Some(n) if n >= 1 => n,
        _ => return Err(format!("{label}.maxTurns must be an integer >= 1.")),
    };

    // pi `budget.graceTurns ?? DEFAULT_TURN_BUDGET_GRACE_TURNS` — `??` accepts only
    // `undefined`/`null`, so an absent key AND an explicit `null` both take the default, while any
    // other non-integer value falls through to the rejection below.
    let grace_turns = match obj.get("graceTurns") {
        None | Some(serde_json::Value::Null) => DEFAULT_TURN_BUDGET_GRACE_TURNS,
        Some(value) => match as_non_negative_integer(value) {
            Some(n) => n,
            None => return Err(format!("{label}.graceTurns must be an integer >= 0.")),
        },
    };

    Ok(Some(ResolvedTurnBudget {
        max_turns,
        grace_turns,
    }))
}

/// pi's `typeof x !== "number" || !Number.isInteger(x) || x < 0` gate, expressed once.
///
/// `serde_json` keeps `2.0` as an `f64`, and JS `Number.isInteger(2.0)` is `true`, so a float with
/// no fractional part is accepted exactly as upstream accepts it — rejecting it here would refuse
/// a `{"maxTurns": 2.0}` that pi runs.
fn as_non_negative_integer(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                return Some(u);
            }
            let f = n.as_f64()?;
            (f.is_finite() && f.fract() == 0.0 && f >= 0.0).then_some(f as u64)
        }
        _ => None,
    }
}

/// pi `appendTurnBudgetSystemPrompt` (`turn-budget.ts:26-39`): fold the budget notice onto the
/// child's system prompt so it can self-pace. Returns `system_prompt` untouched when there is no
/// budget — upstream's `if (!budget) return systemPrompt`, including the un-trimmed passthrough.
#[must_use]
pub fn append_turn_budget_system_prompt(
    system_prompt: &str,
    budget: Option<&ResolvedTurnBudget>,
) -> String {
    let Some(budget) = budget else {
        return system_prompt.to_string();
    };
    let grace = if budget.grace_turns == 1 {
        "1 additional assistant turn".to_string()
    } else {
        format!("{} additional assistant turns", budget.grace_turns)
    };
    let block = [
        "## Turn budget".to_string(),
        format!(
            "This child run has a soft budget of {} assistant turn{}.",
            budget.max_turns,
            if budget.max_turns == 1 { "" } else { "s" }
        ),
        format!("After that, {grace} may be allowed only for a final wrap-up."),
        "When you approach or reach the soft budget, stop starting new tool work and return the final answer immediately.".to_string(),
        "This runner uses process-mode execution, so live steering after launch may be unavailable; treat this instruction as the wrap-up request.".to_string(),
        "If you continue past the soft budget plus grace turns, the supervisor may abort the process and return only partial output.".to_string(),
    ]
    .join("\n");
    let trimmed = system_prompt.trim();
    if trimmed.is_empty() {
        block
    } else {
        format!("{trimmed}\n\n{block}")
    }
}

/// pi `turnBudgetSoftNote` (`turn-budget.ts:41-43`).
#[must_use]
pub fn turn_budget_soft_note(budget: &ResolvedTurnBudget, turn_count: u64) -> String {
    format!(
        "Turn budget wrap-up was requested after {turn_count} assistant turn{} (soft limit {}, grace {}). Process-mode live steering is unavailable, so the child was warned at launch to wrap up by this budget. Output may be partial.",
        if turn_count == 1 { "" } else { "s" },
        budget.max_turns,
        budget.grace_turns
    )
}

/// pi `turnBudgetExceededMessage` (`turn-budget.ts:45-47`).
#[must_use]
pub fn turn_budget_exceeded_message(budget: &ResolvedTurnBudget, turn_count: u64) -> String {
    format!(
        "Subagent exceeded turn budget after {turn_count} assistant turn{} (soft limit {} + grace {}).",
        if turn_count == 1 { "" } else { "s" },
        budget.max_turns,
        budget.grace_turns
    )
}

/// pi `turnBudgetDeferredNote` (`turn-budget.ts:49-51`).
#[must_use]
pub fn turn_budget_deferred_note(budget: &ResolvedTurnBudget, turn_count: u64) -> String {
    format!(
        "Turn-budget termination was deferred at {turn_count} assistant turn{} (soft limit {} + grace {}) because the assistant started tool work. The run ended before another safe assistant boundary; output may be partial.",
        if turn_count == 1 { "" } else { "s" },
        budget.max_turns,
        budget.grace_turns
    )
}

/// pi `formatTurnBudgetOutput` (`turn-budget.ts:53-57`): the abort message alone when there is no
/// partial output, otherwise the message followed by whatever the child had produced.
#[must_use]
pub fn format_turn_budget_output(message: &str, output: &str) -> String {
    if output.trim().is_empty() {
        message.to_string()
    } else {
        format!("{message}\n\nPartial output before turn-budget abort:\n{output}")
    }
}

/// pi `initialTurnBudgetState` (`turn-budget.ts:59-61`): the state stamped on a run BEFORE it has
/// taken a turn, so a caller reading `details.turnBudget` sees the budget that is in force even if
/// the child dies at startup.
#[must_use]
pub fn initial_turn_budget_state(budget: &ResolvedTurnBudget) -> TurnBudgetState {
    TurnBudgetState {
        max_turns: budget.max_turns,
        grace_turns: budget.grace_turns,
        outcome: TurnBudgetOutcome::WithinBudget,
        turn_count: 0,
        wrap_up_requested_at_turn: None,
        termination_deferred_at_turn: None,
        exceeded_at_turn: None,
    }
}

/// pi `turnBudgetState` (`turn-budget.ts:63-71`): the wrap-up-requested / exceeded state.
///
/// `wrapUpRequestedAtTurn` is upstream's literal `budget.maxTurns`, NOT the observed `turnCount` —
/// it records the threshold that was crossed, which is why it is a constant of the budget.
#[must_use]
pub fn turn_budget_state(
    budget: &ResolvedTurnBudget,
    turn_count: u64,
    exceeded: bool,
) -> TurnBudgetState {
    TurnBudgetState {
        max_turns: budget.max_turns,
        grace_turns: budget.grace_turns,
        outcome: if exceeded {
            TurnBudgetOutcome::Exceeded
        } else {
            TurnBudgetOutcome::WrapUpRequested
        },
        turn_count,
        wrap_up_requested_at_turn: Some(budget.max_turns),
        termination_deferred_at_turn: None,
        exceeded_at_turn: exceeded.then_some(turn_count),
    }
}

/// pi `turnBudgetDeferredState` (`turn-budget.ts:73-86`).
///
/// `termination_deferred_at_turn` is upstream's DEFAULTED parameter
/// (`terminationDeferredAtTurn = turnCount`): the caller passes the state's previous value, and JS
/// substitutes the default when that value is `undefined`. `Option::unwrap_or` is the same rule, so
/// a second deferral keeps naming the FIRST turn at which termination was withheld.
#[must_use]
pub fn turn_budget_deferred_state(
    budget: &ResolvedTurnBudget,
    turn_count: u64,
    termination_deferred_at_turn: Option<u64>,
) -> TurnBudgetState {
    TurnBudgetState {
        max_turns: budget.max_turns,
        grace_turns: budget.grace_turns,
        outcome: TurnBudgetOutcome::TerminationDeferred,
        turn_count,
        wrap_up_requested_at_turn: Some(budget.max_turns),
        termination_deferred_at_turn: Some(termination_deferred_at_turn.unwrap_or(turn_count)),
        exceeded_at_turn: None,
    }
}

/// What the supervisor does at this assistant turn (pi `turnBudgetDecision`,
/// `turn-budget.ts:88-98`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnBudgetDecision {
    /// pi `"continue"`.
    Continue,
    /// pi `"defer"` — past the hard limit, but tool work is in flight; withhold the abort.
    Defer,
    /// pi `"abort"`.
    Abort,
}

/// pi `turnBudgetDecision` (`turn-budget.ts:88-98`), branch for branch.
///
/// The first arm is the load-bearing one and reads backwards at a glance: a TERMINAL assistant stop
/// always continues, no matter how far past the hard limit the run is. That is upstream refusing to
/// kill a child that has just finished — there is nothing left to abort, and aborting would replace
/// a complete answer with a partial-output message.
#[must_use]
pub fn turn_budget_decision(
    budget: &ResolvedTurnBudget,
    turn_count: u64,
    terminal_assistant_stop: bool,
    tool_work_active_or_starting: bool,
    enforce_hard_limit: bool,
) -> TurnBudgetDecision {
    let hard_limit = budget.max_turns + budget.grace_turns;
    if terminal_assistant_stop || turn_count < hard_limit {
        return TurnBudgetDecision::Continue;
    }
    if tool_work_active_or_starting && !enforce_hard_limit {
        return TurnBudgetDecision::Defer;
    }
    TurnBudgetDecision::Abort
}

/// pi's SIGTERM escalation delay after the turn-budget SIGINT (`execution.ts:747-751`).
pub const TURN_BUDGET_TERMINATION_DELAY_MS: u64 = 1_000;

/// pi's SIGKILL escalation delay after the turn-budget SIGINT (`execution.ts:752-756`).
///
/// Measured from the SAME instant as [`TURN_BUDGET_TERMINATION_DELAY_MS`], not from the SIGTERM:
/// upstream arms both `setTimeout`s back to back inside `requestTurnBudgetAbort`, so the real
/// SIGTERM→SIGKILL gap is 3 s, not 4 s.
pub const TURN_BUDGET_HARD_KILL_DELAY_MS: u64 = 4_000;

/// The supervisor-side turn-budget latch — pi's `turnBudgetSoftReached` local plus the
/// `result.turnBudget` / `result.turnBudgetExceeded` / `result.wrapUpRequested` fields that
/// `updateTurnBudget` writes (`execution.ts:759-782`), gathered into one value so the drive loop
/// carries a single mutable thing rather than four parallel locals.
#[derive(Clone, Debug, Default)]
pub struct TurnBudgetTracker {
    budget: Option<ResolvedTurnBudget>,
    enforce_hard_limit: bool,
    soft_reached: bool,
    state: Option<TurnBudgetState>,
    exceeded: bool,
    wrap_up_requested: bool,
}

/// What [`TurnBudgetTracker::observe_assistant_turn`] asks the caller to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnBudgetEffect {
    /// Nothing to do this turn.
    None,
    /// pi `appendRecentOutput(progress, [turnBudgetSoftNote(...)])` (`execution.ts:769`) — emit the
    /// one-time wrap-up note into the run's recent output. Raised at most once per run.
    SoftNote(String),
    /// pi `requestTurnBudgetAbort(turnCount)` (`execution.ts:781`) — signal the child down and end
    /// the run with this message. Carries the soft note when the same turn triggered both, since
    /// upstream raises the note first and then aborts inside the same `updateTurnBudget` call.
    Abort {
        /// pi `turnBudgetExceededMessage(budget, turnCount)`, used as BOTH `result.error` and
        /// `result.finalOutput` (`execution.ts:740-741`).
        message: String,
        /// The soft note, when this turn also crossed the soft limit for the first time.
        soft_note: Option<String>,
    },
}

impl TurnBudgetTracker {
    /// A tracker for a run that declared a budget. `enforce_hard_limit` is pi's
    /// `options.enforceHardTurnLimit` (`subagent-executor.ts:240`), which suppresses the
    /// mid-tool-work deferral.
    #[must_use]
    pub fn new(budget: Option<ResolvedTurnBudget>, enforce_hard_limit: bool) -> Self {
        Self {
            budget,
            enforce_hard_limit,
            soft_reached: false,
            // pi `...(options.turnBudget ? { turnBudget: initialTurnBudgetState(options.turnBudget) }
            // : {})` (`execution.ts:399`): the initial state is stamped when the run is BUILT, not
            // on the first turn, so a child that dies before taking one still reports its budget.
            state: budget.as_ref().map(initial_turn_budget_state),
            exceeded: false,
            wrap_up_requested: false,
        }
    }

    /// Is any budget in force? (pi's `if (!budget) return` guard.)
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.budget.is_some()
    }

    /// pi `result.turnBudget`.
    #[must_use]
    pub fn state(&self) -> Option<TurnBudgetState> {
        self.state
    }

    /// pi `result.turnBudgetExceeded`.
    #[must_use]
    pub fn exceeded(&self) -> bool {
        self.exceeded
    }

    /// pi `result.wrapUpRequested`.
    #[must_use]
    pub fn wrap_up_requested(&self) -> bool {
        self.wrap_up_requested
    }

    /// The resolved budget, for the terminal-output formatters.
    #[must_use]
    pub fn budget(&self) -> Option<&ResolvedTurnBudget> {
        self.budget.as_ref()
    }

    /// pi `updateTurnBudget(turnCount, terminalAssistantStop, toolWorkActiveOrStarting)`
    /// (`execution.ts:759-782`), called on every assistant `message_end`.
    ///
    /// `timed_out` is pi's `result.timedOut` guard read at entry; the caller passes its own live
    /// value rather than the tracker duplicating a timeout it does not own.
    pub fn observe_assistant_turn(
        &mut self,
        turn_count: u64,
        terminal_assistant_stop: bool,
        tool_work_active_or_starting: bool,
        timed_out: bool,
    ) -> TurnBudgetEffect {
        let Some(budget) = self.budget else {
            return TurnBudgetEffect::None;
        };
        if timed_out || self.exceeded {
            return TurnBudgetEffect::None;
        }
        if turn_count < budget.max_turns {
            self.state = Some(TurnBudgetState {
                max_turns: budget.max_turns,
                grace_turns: budget.grace_turns,
                outcome: TurnBudgetOutcome::WithinBudget,
                turn_count,
                wrap_up_requested_at_turn: None,
                termination_deferred_at_turn: None,
                exceeded_at_turn: None,
            });
            return TurnBudgetEffect::None;
        }

        let mut soft_note = None;
        if !self.soft_reached {
            self.soft_reached = true;
            self.wrap_up_requested = true;
            soft_note = Some(turn_budget_soft_note(&budget, turn_count));
        }

        let decision = turn_budget_decision(
            &budget,
            turn_count,
            terminal_assistant_stop,
            tool_work_active_or_starting,
            self.enforce_hard_limit,
        );
        if decision == TurnBudgetDecision::Defer {
            self.state = Some(turn_budget_deferred_state(
                &budget,
                turn_count,
                self.state.and_then(|s| s.termination_deferred_at_turn),
            ));
            return match soft_note {
                Some(note) => TurnBudgetEffect::SoftNote(note),
                None => TurnBudgetEffect::None,
            };
        }

        self.state = Some(turn_budget_state(&budget, turn_count, false));
        if decision == TurnBudgetDecision::Abort {
            // pi `requestTurnBudgetAbort` (`execution.ts:733-757`), inlined here for the parts that
            // are pure state; the signal ladder is the caller's, because only it holds the child.
            self.exceeded = true;
            self.wrap_up_requested = true;
            self.state = Some(turn_budget_state(&budget, turn_count, true));
            return TurnBudgetEffect::Abort {
                message: turn_budget_exceeded_message(&budget, turn_count),
                soft_note,
            };
        }
        match soft_note {
            Some(note) => TurnBudgetEffect::SoftNote(note),
            None => TurnBudgetEffect::None,
        }
    }

    /// pi's terminal output composition (`execution.ts:1251-1258`): the message folded onto the
    /// run's delivered output once the child has settled, or `None` when the budget never bit.
    ///
    /// Upstream's three arms in order — exceeded, then termination-deferred, then a
    /// wrap-up-requested run that finished on its own. Only the FIRST wraps the partial output;
    /// the other two are notes appended by the caller.
    #[must_use]
    pub fn terminal_note(&self) -> Option<TurnBudgetTerminalNote> {
        let budget = self.budget?;
        let state = self.state?;
        if self.exceeded {
            return Some(TurnBudgetTerminalNote::Exceeded(
                turn_budget_exceeded_message(&budget, state.turn_count),
            ));
        }
        match state.outcome {
            TurnBudgetOutcome::TerminationDeferred => Some(TurnBudgetTerminalNote::Note(
                turn_budget_deferred_note(
                    &budget,
                    state
                        .termination_deferred_at_turn
                        .unwrap_or(state.turn_count),
                ),
            )),
            TurnBudgetOutcome::WrapUpRequested if self.wrap_up_requested => Some(
                TurnBudgetTerminalNote::Note(turn_budget_soft_note(
                    &budget,
                    state.wrap_up_requested_at_turn.unwrap_or(state.turn_count),
                )),
            ),
            _ => None,
        }
    }
}

/// The two shapes [`TurnBudgetTracker::terminal_note`] can produce, kept distinct because upstream
/// composes them differently: the exceeded message REPLACES the output (with the partial appended
/// under a header), while the other two are appended to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnBudgetTerminalNote {
    /// pi `fullOutput = formatTurnBudgetOutput(turnBudgetExceededMessage(...), fullOutput)`
    /// (`execution.ts:1252`).
    Exceeded(String),
    /// pi `fullOutput = fullOutput.trim() ? `${fullOutput}\n\n${note}` : note`
    /// (`execution.ts:1254-1258`).
    Note(String),
}

/// pi's composition for the two non-fatal notes (`execution.ts:1255`/`:1258`).
///
/// The note goes FIRST — `` `${note}\n\n${fullOutput}` `` — unlike the timeout and exceeded forms,
/// which put the message first and label the remainder as partial. Getting this backwards would
/// bury a wrap-up notice under a long answer.
#[must_use]
pub fn prepend_turn_budget_note(output: &str, note: &str) -> String {
    if output.trim().is_empty() {
        note.to_string()
    } else {
        format!("{note}\n\n{output}")
    }
}

#[cfg(test)]
mod tests {
    // The same test-module allowance every other test module in this crate carries: the workspace
    // denies these crate-wide (`lib.rs:20-24`) for PRODUCTION code, and a test's `panic!`/`expect`
    // IS its failure mechanism. Without it `cargo clippy --all-targets -p cyrup-ext-subagents`
    // fails on this module alone.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_returns_none_for_an_absent_budget_and_defaults_grace_to_one() {
        assert_eq!(resolve_turn_budget_config(None, "turnBudget"), Ok(None));
        assert_eq!(
            resolve_turn_budget_config(Some(&json!({"maxTurns": 3})), "turnBudget"),
            Ok(Some(ResolvedTurnBudget {
                max_turns: 3,
                grace_turns: DEFAULT_TURN_BUDGET_GRACE_TURNS,
            }))
        );
        // `graceTurns: 0` is legal and is NOT the default — the ?? only fires on null/undefined.
        assert_eq!(
            resolve_turn_budget_config(Some(&json!({"maxTurns": 3, "graceTurns": 0})), "turnBudget"),
            Ok(Some(ResolvedTurnBudget {
                max_turns: 3,
                grace_turns: 0,
            }))
        );
    }

    #[test]
    fn every_rejection_uses_upstreams_own_message_with_the_callers_label() {
        // Non-object shapes, including JS `null` (typeof "object" but falsy) and an array.
        for raw in [json!(null), json!(7), json!("x"), json!([1])] {
            assert_eq!(
                resolve_turn_budget_config(Some(&raw), "Agent 'r' turnBudget frontmatter"),
                Err("Agent 'r' turnBudget frontmatter must be an object with maxTurns and optional graceTurns.".to_string()),
                "{raw}"
            );
        }
        assert_eq!(
            resolve_turn_budget_config(Some(&json!({"maxTurns": 2, "nope": 1})), "turnBudget"),
            Err("turnBudget.nope is not supported.".to_string())
        );
        for bad in [json!({}), json!({"maxTurns": 0}), json!({"maxTurns": 1.5})] {
            assert_eq!(
                resolve_turn_budget_config(Some(&bad), "turnBudget"),
                Err("turnBudget.maxTurns must be an integer >= 1.".to_string()),
                "{bad}"
            );
        }
        assert_eq!(
            resolve_turn_budget_config(Some(&json!({"maxTurns": 2, "graceTurns": -1})), "turnBudget"),
            Err("turnBudget.graceTurns must be an integer >= 0.".to_string())
        );
        // A float with no fractional part is an integer to `Number.isInteger`, so it must pass.
        assert_eq!(
            resolve_turn_budget_config(Some(&json!({"maxTurns": 2.0})), "turnBudget"),
            Ok(Some(ResolvedTurnBudget {
                max_turns: 2,
                grace_turns: 1
            }))
        );
    }

    #[test]
    fn the_system_prompt_block_is_upstreams_verbatim_text_and_pluralizes_both_counts() {
        let one = append_turn_budget_system_prompt(
            "  base  ",
            Some(&ResolvedTurnBudget {
                max_turns: 1,
                grace_turns: 1,
            }),
        );
        assert!(one.starts_with("base\n\n## Turn budget\n"), "{one}");
        assert!(one.contains("a soft budget of 1 assistant turn."), "{one}");
        assert!(
            one.contains("After that, 1 additional assistant turn may be allowed"),
            "{one}"
        );
        let many = append_turn_budget_system_prompt(
            "",
            Some(&ResolvedTurnBudget {
                max_turns: 4,
                grace_turns: 2,
            }),
        );
        assert!(many.starts_with("## Turn budget\n"), "{many}");
        assert!(many.contains("a soft budget of 4 assistant turns."), "{many}");
        assert!(
            many.contains("After that, 2 additional assistant turns may be allowed"),
            "{many}"
        );
        // No budget must be an exact passthrough, INCLUDING the untrimmed whitespace pi keeps.
        assert_eq!(append_turn_budget_system_prompt("  x  ", None), "  x  ");
    }

    #[test]
    fn a_terminal_assistant_stop_is_never_aborted_however_far_past_the_hard_limit() {
        let budget = ResolvedTurnBudget {
            max_turns: 2,
            grace_turns: 1,
        };
        assert_eq!(
            turn_budget_decision(&budget, 99, true, false, false),
            TurnBudgetDecision::Continue
        );
        assert_eq!(
            turn_budget_decision(&budget, 99, true, true, true),
            TurnBudgetDecision::Continue
        );
        // Below the hard limit (2 + 1 = 3) continues even mid-tool-work.
        assert_eq!(
            turn_budget_decision(&budget, 2, false, true, false),
            TurnBudgetDecision::Continue
        );
        // At the hard limit with tool work in flight, upstream DEFERS rather than killing …
        assert_eq!(
            turn_budget_decision(&budget, 3, false, true, false),
            TurnBudgetDecision::Defer
        );
        // … unless the caller asked for the hard limit to be enforced.
        assert_eq!(
            turn_budget_decision(&budget, 3, false, true, true),
            TurnBudgetDecision::Abort
        );
        assert_eq!(
            turn_budget_decision(&budget, 3, false, false, false),
            TurnBudgetDecision::Abort
        );
    }

    #[test]
    fn the_tracker_walks_within_budget_then_wrap_up_then_abort_and_raises_the_note_once() {
        let mut tracker = TurnBudgetTracker::new(
            Some(ResolvedTurnBudget {
                max_turns: 2,
                grace_turns: 1,
            }),
            false,
        );
        // Stamped BEFORE any turn — a child that dies at startup still reports its budget.
        assert_eq!(
            tracker.state().map(|s| s.outcome),
            Some(TurnBudgetOutcome::WithinBudget)
        );
        assert_eq!(tracker.state().map(|s| s.turn_count), Some(0));

        assert_eq!(
            tracker.observe_assistant_turn(1, false, false, false),
            TurnBudgetEffect::None
        );
        assert_eq!(
            tracker.state().map(|s| s.outcome),
            Some(TurnBudgetOutcome::WithinBudget)
        );
        assert!(!tracker.wrap_up_requested());

        // Turn 2 == maxTurns: the one-time soft note, and `wrapUpRequested` flips.
        let effect = tracker.observe_assistant_turn(2, false, false, false);
        let TurnBudgetEffect::SoftNote(note) = effect else {
            panic!("expected the soft note, got {effect:?}");
        };
        assert_eq!(
            note,
            "Turn budget wrap-up was requested after 2 assistant turns (soft limit 2, grace 1). \
             Process-mode live steering is unavailable, so the child was warned at launch to wrap \
             up by this budget. Output may be partial."
        );
        assert!(tracker.wrap_up_requested());
        assert_eq!(
            tracker.state().map(|s| s.outcome),
            Some(TurnBudgetOutcome::WrapUpRequested)
        );
        assert_eq!(
            tracker.state().and_then(|s| s.wrap_up_requested_at_turn),
            Some(2)
        );

        // Turn 3 == maxTurns + graceTurns, no tool work: abort, and the note is NOT repeated.
        let effect = tracker.observe_assistant_turn(3, false, false, false);
        let TurnBudgetEffect::Abort { message, soft_note } = effect else {
            panic!("expected an abort, got {effect:?}");
        };
        assert_eq!(soft_note, None, "the soft note must be raised exactly once");
        assert_eq!(
            message,
            "Subagent exceeded turn budget after 3 assistant turns (soft limit 2 + grace 1)."
        );
        assert!(tracker.exceeded());
        assert_eq!(
            tracker.state().and_then(|s| s.exceeded_at_turn),
            Some(3),
            "exceededAtTurn is the OBSERVED turn, unlike wrapUpRequestedAtTurn"
        );
        // Once exceeded, further turns are inert (pi's `result.turnBudgetExceeded` guard).
        assert_eq!(
            tracker.observe_assistant_turn(4, false, false, false),
            TurnBudgetEffect::None
        );
    }

    #[test]
    fn a_deferral_keeps_naming_the_first_turn_it_was_deferred_at() {
        let budget = ResolvedTurnBudget {
            max_turns: 1,
            grace_turns: 0,
        };
        let mut tracker = TurnBudgetTracker::new(Some(budget), false);
        // Turn 1 is already at the hard limit (1 + 0) with tool work in flight → defer.
        let effect = tracker.observe_assistant_turn(1, false, true, false);
        assert!(matches!(effect, TurnBudgetEffect::SoftNote(_)), "{effect:?}");
        assert_eq!(
            tracker.state().map(|s| s.outcome),
            Some(TurnBudgetOutcome::TerminationDeferred)
        );
        assert_eq!(
            tracker.state().and_then(|s| s.termination_deferred_at_turn),
            Some(1)
        );
        // A SECOND deferral at turn 2 must not renumber the deferral point — this is upstream's
        // defaulted `terminationDeferredAtTurn = turnCount` parameter being fed its own old value.
        assert_eq!(
            tracker.observe_assistant_turn(2, false, true, false),
            TurnBudgetEffect::None
        );
        assert_eq!(
            tracker.state().and_then(|s| s.termination_deferred_at_turn),
            Some(1)
        );
        assert_eq!(tracker.state().map(|s| s.turn_count), Some(2));
        assert!(!tracker.exceeded());

        let Some(TurnBudgetTerminalNote::Note(note)) = tracker.terminal_note() else {
            panic!("a deferred run owes the deferral note");
        };
        assert_eq!(
            note,
            "Turn-budget termination was deferred at 1 assistant turn (soft limit 1 + grace 0) \
             because the assistant started tool work. The run ended before another safe assistant \
             boundary; output may be partial."
        );
    }

    #[test]
    fn an_unarmed_tracker_is_inert_and_stamps_no_state() {
        let mut tracker = TurnBudgetTracker::new(None, false);
        assert!(!tracker.is_armed());
        assert_eq!(tracker.state(), None);
        assert_eq!(
            tracker.observe_assistant_turn(99, false, false, false),
            TurnBudgetEffect::None
        );
        assert_eq!(tracker.terminal_note(), None);
        assert!(!tracker.exceeded());
        assert!(!tracker.wrap_up_requested());
    }

    #[test]
    fn a_timed_out_run_stops_updating_the_budget() {
        let mut tracker = TurnBudgetTracker::new(
            Some(ResolvedTurnBudget {
                max_turns: 1,
                grace_turns: 0,
            }),
            false,
        );
        assert_eq!(
            tracker.observe_assistant_turn(5, false, false, true),
            TurnBudgetEffect::None,
            "pi's `if (... result.timedOut ...) return` at execution.ts:761"
        );
        assert!(!tracker.exceeded());
    }

    #[test]
    fn the_exceeded_output_keeps_partial_text_under_upstreams_header() {
        assert_eq!(format_turn_budget_output("msg", "   "), "msg");
        assert_eq!(
            format_turn_budget_output("msg", "half an answer"),
            "msg\n\nPartial output before turn-budget abort:\nhalf an answer"
        );
        assert_eq!(prepend_turn_budget_note("", "note"), "note");
        assert_eq!(
            prepend_turn_budget_note("out", "note"),
            "note\n\nout",
            "upstream puts the NOTE first (execution.ts:1255) — the opposite of the exceeded form"
        );
    }

    #[test]
    fn the_state_serializes_under_upstreams_key_names_and_omits_unset_turns() {
        let state = initial_turn_budget_state(&ResolvedTurnBudget {
            max_turns: 3,
            grace_turns: 2,
        });
        assert_eq!(
            serde_json::to_value(state).expect("serialize"),
            json!({"maxTurns": 3, "graceTurns": 2, "outcome": "within-budget", "turnCount": 0})
        );
        let exceeded = turn_budget_state(
            &ResolvedTurnBudget {
                max_turns: 3,
                grace_turns: 2,
            },
            9,
            true,
        );
        assert_eq!(
            serde_json::to_value(exceeded).expect("serialize"),
            json!({
                "maxTurns": 3,
                "graceTurns": 2,
                "outcome": "exceeded",
                "turnCount": 9,
                "wrapUpRequestedAtTurn": 3,
                "exceededAtTurn": 9
            })
        );
    }
}
