//! Every fixed string the `subagent` tool advertises or refuses with, plus the action table and
//! the fuzzy-match used to suggest a near-miss action.

use crate::background::{run_status, RunState};

/// The `subagent` tool's full multi-section description (R-SA-128, C8) — ported verbatim from
/// pi-subagents' registered tool description (`src/extension/index.ts:461-495`), the string the LLM
/// actually reads to decide how to drive the tool. Reproducing it faithfully is what lets a caller
/// discover the management (`action: "list"/"get"/…`), control (`status`/`interrupt`/`resume`/
/// `append-step`), CHAIN, and PARALLEL shapes at all — not just the SINGLE shape the pre-C8 schema
/// advertised. The pi tool-description executable spec (`test/unit/tool-description.test.ts`) pins
/// several substrings of this text (the `action: "list"` inspect line, `executable/non-disabled`,
/// `proactive skill subagent suggestions`, the `output?,reads?,progress?` PARALLEL shape, the
/// `timeoutMs`/`maxRuntimeMs` `only for foreground runs` / `omit for async/background runs` note);
/// this crate's own `subagent_tool_schema_exposes_the_full_pi_parameter_union` test re-pins them.
pub(crate) const SUBAGENT_TOOL_DESCRIPTION: &str = r#"Delegate to subagents or manage agent definitions.

EXECUTION (use exactly ONE mode):
• Before executing, use { action: "list" } to inspect configured agents/chains. Only execute agents listed as executable/non-disabled.
• SINGLE: { agent, task? } - one task; omit task for self-contained agents
• CHAIN: { chain: [{agent:"agent-a"}, {parallel:[{agent:"agent-b",count:3}]}] } - sequential pipeline with optional parallel fan-out
• PARALLEL: { tasks: [{agent,task,count?,output?,reads?,progress?}, ...], concurrency?: number, worktree?: true } - concurrent execution (worktree: isolate each task in a git worktree)
• Optional context: { context: "fresh" | "fork" } (explicit value overrides every child; when omitted, each requested agent uses its own defaultContext, otherwise "fresh"; inspect agent defaults via { action: "list" })
• Optional timeout: { timeoutMs } or { maxRuntimeMs } sets a run-level max runtime for foreground and async/background runs
• If { action: "list" } shows proactive skill subagent suggestions, consider a small fresh-context fanout for broad tasks where one of those skills would materially help

CHAIN TEMPLATE VARIABLES (use in task strings):
• {task} - The original task/request from the user
• {previous} - Text response from the previous step (empty for first step)
• {chain_dir} - Shared directory for chain files (e.g., <tmpdir>/pi-subagents-<scope>/chain-runs/abc123/)

Example: { chain: [{agent:"agent-a", task:"Analyze {task}"}, {agent:"agent-b", task:"Plan based on {previous}"}] }

MANAGEMENT (use action field, omit agent/task/chain/tasks):
• { action: "list" } - discover executable agents/chains
• { action: "get", agent: "name" } - full detail; packaged agents use dotted runtime names like "package.agent"
• { action: "models", agent?: "name" } - show the runtime-loaded builtin subagent model mapping, optionally filtered to one builtin
• { action: "create", config: { name: "custom-agent", package: "code-analysis", systemPrompt, systemPromptMode, inheritProjectContext, inheritSkills, defaultContext, ... } }
• { action: "update", agent: "code-analysis.custom-agent", config: { package: "analysis", ... } } - merge
• { action: "delete", agent: "code-analysis.custom-agent" }
• Use chainName for chain operations; packaged chains also use dotted runtime names

CONTROL:
• { action: "status", id: "..." } - inspect an async/background run by id or prefix
• { action: "interrupt", id?: "..." } - soft-interrupt the current child turn and leave the run paused
• { action: "resume", id: "...", message: "...", index?: 0 } - interrupt then follow up with a live async child, or revive a completed async/foreground child from its session
• { action: "steer", id: "...", message: "...", index?: 0 } - queue non-terminal guidance for a live async child WITHOUT interrupting it
• { action: "stop", id?: "..." } - TERMINALLY stop a running or queued async run and its whole descendant subtree; the run ends "stopped" and CANNOT be resumed
• { action: "append-step", id: "...", chain: [{agent:"agent-c", task:"Use {previous}"}] } - append one step to the tail of a running async chain

DIAGNOSTICS:
• { action: "doctor" } - read-only report for runtime paths, discovery, sessions, and intercom"#;

/// pi's steer-on-a-foreground-run refusal (`subagent-executor.ts:3217` @v0.34.0), rebranded on the
/// product noun exactly as [`crate::extension::SubagentExecutor::control_steer`]'s success text already is.
///
/// It is a distinct message, not a variant of "no live run directory": the run the caller named
/// exists and is running RIGHT NOW — steering simply has no transport to it, because the steer
/// inbox is an async-run directory and a foreground run has none. That is why the text points at
/// `interrupt`/`resume`, which do work on a foreground run, instead of implying the id was wrong.
/// G77 — pi's stop-on-a-foreground-run refusal (`subagent-executor.ts:4797` @v0.43.0), verbatim.
/// A distinct message from [`STEER_FOREGROUND_RUN_REFUSAL`]: it points at `interrupt` only, because
/// `resume` on a live foreground run is not the alternative to a *stop*.
pub(crate) const STOP_FOREGROUND_RUN_REFUSAL: &str =
    "action='stop' supports async runs only. Use action='interrupt' for foreground runs.";

/// G77 — pi's stop-on-a-NESTED-run refusal (`subagent-executor.ts:4796` @v0.43.0), verbatim:
/// `if (resolved?.kind === "nested") return { … text: "action='stop' supports current-session
/// top-level async runs only." … }`.
///
/// A nested run is one spawned INSIDE another run's subtree (it lives under that root's
/// `nested-subagent-runs` tree, not in this session's own async root), so it has no entry in the
/// session's async store and `stopAsyncRun` could never address it. Upstream refuses it with its own
/// sentence rather than the generic "no stoppable async run" one, because the id the caller gave is
/// real — it is simply out of this verb's scope.
pub(crate) const STOP_NESTED_RUN_REFUSAL: &str =
    "action='stop' supports current-session top-level async runs only.";

/// G77 — pi's terminal fallback for `action: "stop"` (`subagent-executor.ts:4812` @v0.43.0),
/// verbatim: reached when `stopAsyncRun` returns `null`, which happens iff
/// `getAsyncStopTarget` found no target at all (`async-stop-action.ts:29-30`) — i.e. no `dir` was
/// given and the id is not a tracked async job of this session.
///
/// Distinct from `"No running or queued async run was found for '{id}'."`, which upstream reserves
/// for a target that WAS found but whose reconciled state is neither `running` nor `queued`
/// (`async-stop-action.ts:39-44`). cyrup previously collapsed both onto the second text; the split
/// is restored here with `run_status::resolve_run_id` — an id that resolves to nothing in this
/// session's async namespace is upstream's "no target", and an id that resolves but reconciles
/// non-actionable is upstream's "not running or queued".
pub(crate) const STOP_NO_STOPPABLE_RUN_REFUSAL: &str = "No stoppable async run found in this session.";

/// SUBA-057 — pi's *"is `<state>`, not running."* refusal, which
/// `dismissRecoveredWorkflow` spells THREE times with identical text
/// (`runs/foreground/async-dismiss-action.ts:45`, `:57`, `:70` @v0.47.1): once against the record
/// as read, once against the result-file re-reconcile, and once against the post-write
/// re-reconcile. Factored into one function here for the same reason `SUBAGENT_ACTIONS` is one
/// slice — three hand-written copies of a byte-identical sentence are three chances to drift.
///
/// The state word is [`run_status::run_state_label`], the same lowercase renderer every other
/// human-facing state string in this crate uses, matching upstream's raw `status.state` (its
/// `AsyncStatus.state` is already the lowercase wire word).
pub(crate) fn dismiss_not_running_refusal(run_id: &str, state: RunState) -> String {
    format!(
        "Recovered workflow '{run_id}' is {}, not running.",
        run_status::run_state_label(state)
    )
}

pub(crate) const STEER_FOREGROUND_RUN_REFUSAL: &str = "action='steer' currently supports live async Cyrup \
     child sessions only; use action='interrupt' or action='resume' for foreground runs.";

/// The fanout-child's restricted tool description — pi's exact 3-line text, joined with `\n`
/// (`extension/fanout-child.ts:177-181` @v0.43.0; the same block sat at `:159-163` @v0.34.0). It
/// tells the model up front which management/control actions remain available and which mutation
/// actions are blocked in this mode, rather than only discovering the block via a runtime
/// [`cyrup_core::ToolError`] from [`crate::extension::SubagentTool::route_management_action`].
///
/// Both lines are now upstream's, verbatim, and both used to diverge:
///
/// * The **blocked** line is v0.43.0's, which renamed the lead-in from "Agent config mutation
///   actions" to "Mutating management actions" and added an eighth verb, `grant-spawn-budget`
///   (`fanout-child.ts:180`). Naming it here is accurate on both sides, though the two sides now
///   refuse it at DIFFERENT gates: upstream refuses it because it is on
///   `MUTATING_MANAGEMENT_ACTIONS`; cyrup — since SUBA-046 gave the verb a real dispatch arm,
///   [`crate::extension::SubagentTool::route_grant_spawn_budget`] — refuses it at that arm's FIRST gate, pi's own
///   `if (deps.allowMutatingManagementActions === false || !ctx.hasUI)`
///   (`subagent-executor.ts:4458` @v0.43.0), which a fanout-child registration fails on the
///   `allow_mutating_management` half. A child therefore gets upstream's verbatim "available only
///   from the root interactive parent session." refusal rather than an unknown-action error, which
///   is what upstream's own child would get were its denylist not consulted first.
///   [`crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS`] is still deliberately NOT
///   extended to match: it gates [`crate::extension::SubagentTool::route_management_action`], which
///   `grant-spawn-budget` does not route through, and upstream's v0.43.0 set has 26 entries
///   (`subagent-executor.ts:151`), almost all of them naming actions this crate has not ported
///   (`watchdog.configure`, `mission.*`, `inspector.*`, `project.*`, `schedule.*`, `refine*`), and
///   grafting one of them onto a 7-entry port would make the runtime denylist message advertise a
///   verb with no handler.
/// * The **allowed** line names `steer` (which the dispatcher genuinely answers) and, since this
///   change, no longer names `stop` — upstream's allowed list has NEVER carried `stop`, at
///   `fanout-child.ts:161` @v0.34.0 nor `:179` @v0.43.0. This is an ADVERTISING change only, and
///   deliberately so on both sides: `stop` is in
///   [`crate::extension::SubagentTool::route_action`]'s control arm and is absent from
///   [`crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS`] (as it is from upstream's own
///   26-entry set), so a fanout child that calls `action: "stop"` is served here exactly as it is
///   served upstream. Upstream simply does not put it in the eight verbs it volunteers, and the
///   child-safe description's contract is "advertise pi's exact text", not "enumerate every
///   reachable verb" — the full enumeration lives in the tool's JSON Schema `action` enum
///   ([`crate::extension::tool::schema::subagent_tool_parameters`]), which does name `stop` and which both registrations share.
pub(crate) const CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION: &str = "Delegate to subagents from child-safe fanout mode.\nAllowed management/control actions: list, get, status, interrupt, resume, steer, append-step, doctor.\nMutating management actions (create, update, delete, eject, disable, enable, reset, grant-spawn-budget) are blocked in this mode.";

/// SUBA-038/SUBA-065 — the tool's advertised management/control verbs, as ONE source of truth.
///
/// pi has `SUBAGENT_ACTIONS` (`shared/types.ts:1885` @v0.43.0 / `:1968` @v0.47.1) and both the JSON
/// Schema `action` enum and the unknown-action message read it. cyrup hand-wrote the list in three
/// places and two of them drifted: the management unknown-action text omitted the four `watchdog.*`
/// verbs that DO dispatch, and the control-arm text omitted `stop`. A model recovering from a typo
/// was therefore steered away from verbs that exist.
///
/// Order is pi's own (`stop` between `steer` and `append-step`, `shared/types.ts:1885`). This is
/// cyrup's CURRENT surface, not upstream's full 53 — the missing verbs have their own items
/// (SUBA-016, SUBA-046, SUBA-055, SUBA-057, …) and each adds its name here when it lands.
/// SUBA-049 — how long `action: "steer"` waits for the child's acknowledgment before answering
/// `pending`. pi `ackTimeoutMs ?? 3_000` (`runs/foreground/async-steering-action.ts`'s
/// `waitForSteeringAction` call).
///
/// Three seconds is a deliberate compromise upstream already struck: the child polls its inbox
/// every 250 ms, so a live child normally answers in well under one, and a child that is mid-tool
/// will not answer at all until it reaches a safe point — waiting longer would block the
/// orchestrator's turn on work that has no bounded end.
pub(crate) const STEER_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(3_000);

/// How often the ack wait re-reads the directory. Deliberately finer than the child's own 250 ms
/// write cadence so the answer is reported in the poll after it lands rather than up to a full
/// child-interval later.
pub(crate) const STEER_ACK_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// Read-only accessor for [`SUBAGENT_ACTIONS`], so other modules can assert against the ONE list
/// without it becoming writable or duplicable. Added for SUBA-055's
/// `the_tool_reference_topic_names_every_dispatched_verb`, which is what keeps the packaged
/// tool-reference page from silently falling behind the verbs this build dispatches.
#[cfg(test)]
#[must_use]
pub(crate) fn subagent_actions() -> &'static [&'static str] {
    SUBAGENT_ACTIONS
}

pub(crate) const SUBAGENT_ACTIONS: &[&str] = &[
    "list",
    "get",
    "models",
    // SUBA-055 — pi's own position for this verb (`shared/types.ts:1968` @v0.47.1:
    // `… "models", "children.list", "guide", "create", …`). `children.list` is NOT added with it
    // and the reason is recorded rather than left to inference: upstream's `children.list` lists
    // RETAINED children — completed single runs held open for follow-up under a
    // `parentWorkflowRunId` — which is part of the unported `workflowScript` shape, so the verb
    // would advertise a listing that is always empty. That residual stays open under SUBA-005's
    // unowned-verb list; this half is the `guide` action and its packaged docs.
    "guide",
    "create",
    "update",
    "delete",
    "eject",
    "disable",
    "enable",
    "reset",
    "status",
    // SUBA-046 — pi's own position for this verb (`shared/types.ts:1885` @v0.43.0: `… "status",
    // "grant-spawn-budget", "interrupt", …`). It was already advertised in the child-safe tool
    // description while landing on the unknown-action arm; now it dispatches.
    "grant-spawn-budget",
    "interrupt",
    "resume",
    "steer",
    "stop",
    // SUBA-057 — pi's own position for this verb (`shared/types.ts:2084` @v0.47.1: `… "steer",
    // "stop", "dismiss", "append-step", …`). Dispatched by `route_control_action`.
    "dismiss",
    "append-step",
    "doctor",
    "mission.create",
    "mission.list",
    "mission.show",
    "mission.update",
    "mission.attach-run",
    "mission.close",
    "watchdog.status",
    "watchdog.check",
    "watchdog.configure",
    "watchdog.recommend-model",
];

/// SUBA-065 — pi `DESTRUCTIVE_MANAGEMENT_ACTIONS`
/// (`runs/foreground/subagent-executor.ts:168` @v0.47.1), ported VERBATIM including the verbs cyrup
/// does not yet dispatch: the set exists to make the did-you-mean rule STRICTER for these
/// candidates, so carrying a name cyrup has not implemented costs nothing and carrying one fewer
/// would silently loosen the gate the day that verb lands.
///
/// Distinct from [`crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS`], which is a
/// different, smaller set serving the child-safe denylist.
const DESTRUCTIVE_MANAGEMENT_ACTIONS: &[&str] = &[
    "delete",
    "eject",
    "disable",
    "reset",
    "mission.close",
    "worktree.discard",
    "refine.rollback",
    "inspector.close",
    "project.close",
    "stop",
    "interrupt",
    "reject-checkpoint",
    "schedule.delete",
];

/// SUBA-065 — pi `editDistance` (`subagent-executor.ts:170-184` @v0.47.1): the standard
/// single-row Levenshtein, ported with upstream's own row-reuse shape so the two cannot drift on an
/// edge case.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for left_index in 1..=left.len() {
        // Every index below is in range by construction (`previous.len() == right.len() + 1`), but
        // the crate denies `clippy::indexing_slicing`, so each read goes through `get`/`get_mut`
        // with a saturating fallback rather than a panic path.
        let mut diagonal = previous.first().copied().unwrap_or(0);
        if let Some(slot) = previous.get_mut(0) {
            *slot = left_index;
        }
        for right_index in 1..=right.len() {
            let above = previous.get(right_index).copied().unwrap_or(0);
            let left_char = left.get(left_index - 1);
            let right_char = right.get(right_index - 1);
            let next = if left_char.is_some() && left_char == right_char {
                diagonal
            } else {
                let west = previous.get(right_index - 1).copied().unwrap_or(0);
                diagonal.min(above).min(west) + 1
            };
            if let Some(slot) = previous.get_mut(right_index) {
                *slot = next;
            }
            diagonal = above;
        }
    }
    previous.last().copied().unwrap_or(0)
}

/// SUBA-065 — pi `hasSingleAdjacentTransposition` (`subagent-executor.ts:186-192` @v0.47.1):
/// equal-length strings differing only by one swap of adjacent characters (`statsu` vs `status`),
/// which Levenshtein scores 2 and would otherwise miss.
fn has_single_adjacent_transposition(left: &str, right: &str) -> bool {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len() != right.len() {
        return false;
    }
    let Some(mismatch) = left.iter().zip(right.iter()).position(|(l, r)| l != r) else {
        return false;
    };
    // pi indexes `left[mismatch + 1]` / `right[mismatch + 1]` directly; JS yields `undefined` past
    // the end and `undefined === undefined` is TRUE, so a mismatch at the final index would pass
    // upstream's first two tests. It still fails the tail comparison only if the tails differ —
    // and with `mismatch` at the last index both tails are empty, so upstream would return TRUE for
    // e.g. ("ab", "ac"). That is unreachable in practice (a final-character mismatch means
    // `left[mismatch] != right[mismatch]`, and the test `left[mismatch] === right[mismatch+1]`
    // compares a real char against `undefined`, which is FALSE). Rust's bounds check reaches the
    // same answer without the ambiguity.
    let Some(&l_next) = left.get(mismatch + 1) else {
        return false;
    };
    let Some(&r_next) = right.get(mismatch + 1) else {
        return false;
    };
    let (Some(&l_at), Some(&r_at)) = (left.get(mismatch), right.get(mismatch)) else {
        return false;
    };
    let (Some(l_tail), Some(r_tail)) = (left.get(mismatch + 2..), right.get(mismatch + 2..)) else {
        return false;
    };
    l_at == r_next && l_next == r_at && l_tail == r_tail
}

/// SUBA-038/SUBA-065 — pi `unknownSubagentActionMessage`
/// (`runs/foreground/subagent-executor.ts:195-208` @v0.47.1, from `28b9222`). At v0.43.0 the text
/// was the bare `Unknown action: ${action}. Valid: …` (`:4861`); this is the richer v0.47.1 form
/// that supersedes it.
///
/// The ASYMMETRIC destructive rule is the load-bearing half and is reproduced exactly: an ordinary
/// candidate matches on `distance <= max(1, floor(len/4))` OR a single adjacent transposition, but
/// a DESTRUCTIVE candidate matches only on `distance === 1 && requested.length >=
/// candidate.length - 1`. That is what stops a loose typo being nudged toward `delete`. Porting a
/// naive did-you-mean without it would be strictly worse than the wall of names it replaces.
pub(crate) fn unknown_subagent_action_message(action: &str) -> String {
    let requested = action.to_lowercase();
    let suggestion = SUBAGENT_ACTIONS.iter().find(|candidate| {
        let distance = edit_distance(&requested, candidate);
        if DESTRUCTIVE_MANAGEMENT_ACTIONS.contains(*candidate) {
            return distance == 1 && requested.chars().count() + 1 >= candidate.chars().count();
        }
        distance <= std::cmp::max(1, candidate.chars().count() / 4)
            || has_single_adjacent_transposition(&requested, candidate)
    });
    let next_step = "Use subagent({ action: \"status\" }) to inspect runs or subagent({ action: \"list\" }) to inspect agents.";
    let valid_actions = format!("Valid: {}.", SUBAGENT_ACTIONS.join(", "));
    match suggestion {
        Some(candidate) => {
            format!("Unknown action: {action}. Did you mean {candidate}? {next_step} {valid_actions}")
        }
        None => format!("Unknown action: {action}. {next_step} {valid_actions}"),
    }
}

/// pi's refusal text for an `action` that is present but blank, adapted — see
/// [`crate::extension::tool::params::normalize_public_subagent_execution`] for the `[CYRUP-DELTA]` on its second clause.
pub(crate) const BLANK_ACTION_REFUSAL: &str = "action must be a non-empty management/control action, or omit \
                                    action and provide an execution shape (agent/task, tasks, or \
                                    chain).";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::extension::executor::SubagentExecutor;
    use crate::extension::executor::notices::ForegroundControlEntry;
    use crate::extension::executor::paths::default_async_root_in;
    use crate::extension::testsupport::dispatch_tool;
    use crate::extension::testsupport::scoped_tool;
    use crate::extension::testsupport::seed_running_run;
    use crate::extension::testsupport::tool_text;
    use crate::extension::tool::SubagentTool;
    use crate::extension::tool::schema::subagent_tool_parameters;
    use cyrup_core::CancelToken;
    use cyrup_core::Tool;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn child_safe_tool_advertises_pis_restricted_three_line_description() {
        let executor = Arc::new(SubagentExecutor::new());
        let full = SubagentTool::new(executor.clone(), PathBuf::from("/tmp"));
        let child_safe = SubagentTool::new_child_safe(executor, PathBuf::from("/tmp"));

        assert_eq!(
            Tool::description(&full),
            SUBAGENT_TOOL_DESCRIPTION,
            "the root orchestrator tool keeps the full description"
        );
        assert_eq!(
            Tool::description(&child_safe),
            "Delegate to subagents from child-safe fanout mode.\n\
             Allowed management/control actions: list, get, status, interrupt, resume, steer, append-step, doctor.\n\
             Mutating management actions (create, update, delete, eject, disable, enable, reset, grant-spawn-budget) are blocked in this mode.",
            "the child-safe tool must advertise pi's exact fanout-child.ts:178-180 @v0.43.0 text"
        );
        // The allowed list is upstream's EIGHT verbs and no more. `stop` in particular must not
        // reappear here: it is dispatchable from a fanout child (see
        // `child_safe_mode_still_dispatches_the_unadvertised_stop_action`) but upstream does not
        // volunteer it, and re-adding it is the exact divergence this test now guards.
        let allowed = Tool::description(&child_safe)
            .lines()
            .nth(1)
            .and_then(|line| line.strip_prefix("Allowed management/control actions: "))
            .and_then(|list| list.strip_suffix('.'))
            .expect("the child-safe description's second line is the allowed list");
        assert_eq!(
            allowed.split(", ").collect::<Vec<_>>(),
            vec![
                "list",
                "get",
                "status",
                "interrupt",
                "resume",
                "steer",
                "append-step",
                "doctor"
            ],
            "pi's allowed list is these eight verbs in this order (`fanout-child.ts:179` @v0.43.0)"
        );
        // The advertised blocked list must name exactly the denylist the dispatcher enforces —
        // a child told only about create/update/delete would discover the eject/disable/enable/reset
        // block by runtime error instead, which is the whole failure this description exists to
        // prevent.
        for action in crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS {
            assert!(
                Tool::description(&child_safe).contains(action),
                "child-safe description must name the blocked action '{action}'"
            );
        }
        assert_ne!(
            Tool::description(&child_safe),
            Tool::description(&full),
            "a fanout child must NOT advertise the full orchestrator description"
        );
    }

    /// SUBA-065 — pi `unknownSubagentActionMessage` (`subagent-executor.ts:195-208` @v0.47.1,
    /// from `28b9222`), including the ASYMMETRIC destructive-candidate rule.
    ///
    /// THE INTERESTING HALF is not the suggestion, it is the gate: a naive did-you-mean would nudge
    /// a typo toward `delete`, `eject`, `stop` or `interrupt`. Upstream applies
    /// `distance === 1 && requested.length >= candidate.length - 1` to destructive candidates and
    /// the looser `distance <= max(1, floor(len/4)) || singleAdjacentTransposition` to everything
    /// else. Ported verbatim; this test is what stops someone "improving" the message casually.
    #[test]
    fn the_unknown_action_message_suggests_safely() {
        // A near miss on a SAFE verb is suggested.
        let statu = unknown_subagent_action_message("statu");
        assert!(statu.starts_with("Unknown action: statu. Did you mean status?"), "{statu}");
        // …and the message always carries the next-step hint and the full valid list.
        assert!(statu.contains(r#"Use subagent({ action: "status" }) to inspect runs"#), "{statu}");
        assert!(statu.ends_with(&format!("Valid: {}.", SUBAGENT_ACTIONS.join(", "))), "{statu}");

        // A transposition Levenshtein scores 2 is still caught, on a safe verb.
        let statsu = unknown_subagent_action_message("statsu");
        assert!(statsu.contains("Did you mean status?"), "{statsu}");

        // A DESTRUCTIVE candidate needs distance EXACTLY 1 AND `requested.length >=
        // candidate.length - 1`. `dele`/`del` are distance 2 and 3 from `delete`, and `res` is
        // distance 2 from `reset` — a naive prefix-or-typo heuristic would happily suggest all
        // three, which is exactly the nudge-toward-`delete` failure upstream's asymmetry prevents.
        for loose_typo in ["dele", "del", "res"] {
            let message = unknown_subagent_action_message(loose_typo);
            for destructive in DESTRUCTIVE_MANAGEMENT_ACTIONS {
                assert!(
                    !message.contains(&format!("Did you mean {destructive}?")),
                    "a loose typo {loose_typo:?} must never be nudged toward the destructive \
                     '{destructive}': {message}"
                );
            }
        }

        // The rule is not "never suggest a destructive verb": distance 1 with a long-enough
        // request still does, which is upstream's own behaviour.
        let delet = unknown_subagent_action_message("delet");
        assert!(delet.contains("Did you mean delete?"), "{delet}");

        // No near candidate at all → the plain form, with no "Did you mean".
        let nothing = unknown_subagent_action_message("frobnicate");
        assert_eq!(
            nothing,
            format!(
                "Unknown action: frobnicate. Use subagent({{ action: \"status\" }}) to inspect runs \
                 or subagent({{ action: \"list\" }}) to inspect agents. Valid: {}.",
                SUBAGENT_ACTIONS.join(", ")
            )
        );

        // The list the message publishes must be exactly what dispatches — the drift SUBA-038
        // named. Derived from the schema, which is itself derived from `SUBAGENT_ACTIONS`.
        let advertised: Vec<String> = subagent_tool_parameters()["properties"]["action"]["enum"]
            .as_array()
            .expect("the action property advertises an enum")
            .iter()
            .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
            .collect();
        assert_eq!(advertised, SUBAGENT_ACTIONS.iter().map(|a| (*a).to_string()).collect::<Vec<_>>());
    }

    /// SUBA-065's two primitives, pinned against upstream's own algorithms so a later refactor of
    /// the message cannot quietly change which candidates match.
    #[test]
    fn edit_distance_and_transposition_match_pis_definitions() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("status", "status"), 0);
        assert_eq!(edit_distance("statu", "status"), 1);
        assert_eq!(edit_distance("statsu", "status"), 2);
        assert_eq!(edit_distance("delet", "delete"), 1);
        assert_eq!(edit_distance("dele", "delete"), 2);

        assert!(has_single_adjacent_transposition("statsu", "status"));
        assert!(!has_single_adjacent_transposition("status", "status"));
        assert!(!has_single_adjacent_transposition("statu", "status"), "different lengths");
        // `acb` and `abc` differ only by the adjacent swap at index 1.
        assert!(has_single_adjacent_transposition("acb", "abc"));
        // A mismatch at the FINAL index has no adjacent partner to swap with.
        assert!(!has_single_adjacent_transposition("ab", "ac"));
    }

    /// G77 — the refusal shapes, each with pi's verbatim text. None of them is "unknown
    /// subagent action", and none of them is a panic.
    ///
    /// The two not-found texts are DISTINCT upstream and were previously collapsed onto one here:
    ///
    /// * an id that names nothing at all never reaches `stopAsyncRun`'s actionability guard —
    ///   `getAsyncStopTarget` returns `undefined` (`async-stop-action.ts:18-20`), `stopAsyncRun`
    ///   returns `null`, and the executor falls through to its own terminal
    ///   `"No stoppable async run found in this session."` (`subagent-executor.ts:4810-4814`);
    /// * an id that DOES name a run whose reconciled state is neither `running` nor `queued` is the
    ///   only input that reaches `"No running or queued async run was found for '{id}'."`
    ///   (`async-stop-action.ts:39-44`).
    #[tokio::test]
    async fn the_stop_action_refusals_use_pis_verbatim_texts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = scoped_tool(dir.path()).await;

        // An id naming NOTHING: upstream's `stopAsyncRun` → `null` fallback (`:4812`).
        let err = dispatch_tool(&tool, serde_json::json!({ "action": "stop", "id": "nosuchrun001" }))
            .await
            .expect_err("an unknown run must be refused");
        assert!(
            err.to_string().contains(STOP_NO_STOPPABLE_RUN_REFUSAL),
            "an id that resolves to no run at all never reaches `stopAsyncRun`'s running/queued \
             guard upstream — it is the `null`-target fallback: {err}"
        );

        // An id naming a run that EXISTS but has already finished: upstream's actionability guard
        // (`async-stop-action.ts:39-44`). This is the ONLY input that earns the other text.
        let finished = seed_running_run(dir.path(), "stopfinished1", &["scout"]);
        {
            let mut status: crate::background::RunStatus = serde_json::from_slice(
                &std::fs::read(&finished.status).expect("read seeded status"),
            )
            .expect("valid status json");
            status.state = crate::background::RunState::Complete;
            status.pid = None;
            std::fs::write(
                &finished.status,
                serde_json::to_vec(&status).expect("serialize status"),
            )
            .expect("rewrite status as terminal");
        }
        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "stop", "id": "stopfinished1" }),
        )
        .await
        .expect_err("a finished run is not stoppable");
        assert!(
            err.to_string()
                .contains("No running or queued async run was found for 'stopfinished1'."),
            "a resolvable-but-terminal run must get the actionability text, not the no-target one: \
             {err}"
        );

        // No selector at all (`subagent-executor.ts:4789`). There is deliberately NO
        // most-recently-active default for `stop` — upstream has one only for `interrupt`.
        let err = dispatch_tool(&tool, serde_json::json!({ "action": "stop" }))
            .await
            .expect_err("a selector-less stop must be refused, never defaulted");
        assert!(
            err.to_string().contains("action='stop' requires id or dir."),
            "a stop is unrecoverable; guessing a target is exactly what upstream refuses: {err}"
        );
    }

    /// G77 — the advertise-vs-dispatch invariant for `stop`, both directions in one test: the
    /// schema names it in pi's own position within `SUBAGENT_ACTIONS`, every surface that TELLS a
    /// model the action set mentions it, and the dispatcher genuinely answers it.
    #[test]
    fn the_stop_action_is_advertised_everywhere_it_is_dispatched() {
        let params = subagent_tool_parameters();
        let action_values: Vec<&str> = params["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        let stop_at = action_values
            .iter()
            .position(|v| *v == "stop")
            .expect("`stop` must be an advertised action");
        let steer_at = action_values.iter().position(|v| *v == "steer").expect("steer");
        let append_at = action_values
            .iter()
            .position(|v| *v == "append-step")
            .expect("append-step");
        assert!(
            steer_at < stop_at && stop_at < append_at,
            "pi orders `… steer, stop, append-step …` (`shared/types.ts:1885` @v0.43.0): {action_values:?}"
        );

        // pi's three addressing properties each name `action='stop'` (`extension/schemas.ts:266,
        // 269,272` @v0.43.0) — a model told the verb exists but shown no property that mentions it
        // has to guess how to address it.
        for key in ["id", "runId", "dir"] {
            let description = params["properties"][key]["description"]
                .as_str()
                .unwrap_or_default();
            assert!(
                description.contains("action='stop'"),
                "the `{key}` property description must name action='stop': {description}"
            );
        }

        // The ROOT orchestrator's prose surface names it too.
        assert!(SUBAGENT_TOOL_DESCRIPTION.contains("{ action: \"stop\""));

        // The child-safe prose surface deliberately does NOT — upstream's allowed list has never
        // carried `stop`, at `fanout-child.ts:161` @v0.34.0 nor `:179` @v0.43.0. This assertion
        // replaces an earlier `contains("stop")` that pinned the divergence in place. The verb
        // stays REACHABLE from a fanout child regardless; that half is proved by
        // `child_safe_mode_still_dispatches_the_unadvertised_stop_action` below, and by the shared
        // schema enum asserted above, which both registrations advertise.
        assert!(
            !CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION.contains("stop"),
            "pi's child-safe description names neither `stop` nor a blocked verb containing it: \
             {CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION}"
        );
    }

    /// SUBA-025 — the ADVERTISED description is resolved from `subagents.toolDescriptionMode` and
    /// from a `subagent-tool-description.md` override, exercised through the real
    /// `cyrup_core::Tool::description()` the host reads.
    ///
    /// Pre-fix this could not hold at all: `SubagentTool::new` assigned the `SUBAGENT_TOOL_DESCRIPTION`
    /// constant unconditionally and `SubagentExtensionConfig` had no `toolDescriptionMode` key, so
    /// `config.json` naming one had it dropped by serde in silence and every registration — compact,
    /// custom, or default — advertised the same bytes. Both halves below therefore observed the FULL
    /// text where they now observe the configured one.
    ///
    /// Driven through the same two calls `init`'s Full arm makes (`build_subagent_tool_description`
    /// then `with_description`) rather than through `InitApi`, which needs a host.
    #[test]
    fn the_advertised_description_honours_the_configured_mode_and_the_file_override() {
        use crate::registration::tool_description::{
            build_subagent_tool_description, ToolDescriptionOptions,
            COMPACT_SUBAGENT_TOOL_DESCRIPTION, CUSTOM_TOOL_DESCRIPTION_FILE,
            SUBAGENT_SAFETY_GUIDANCE,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let executor = Arc::new(SubagentExecutor::new());
        let options = ToolDescriptionOptions {
            cwd: dir.path().to_path_buf(),
            agent_dir: dir.path().join("agent"),
        };
        let build = |raw: Option<serde_json::Value>| {
            let mut warnings = Vec::new();
            let description = build_subagent_tool_description(
                raw.as_ref(),
                SUBAGENT_TOOL_DESCRIPTION,
                &options,
                &mut warnings,
            );
            SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf())
                .with_description(description)
        };

        // The key is really a key: it survives a `config.json` round-trip rather than being dropped.
        let config: crate::registration::SubagentExtensionConfig =
            serde_json::from_value(serde_json::json!({ "toolDescriptionMode": "compact" }))
                .expect("config parses");
        assert_eq!(
            config.tool_description_mode.as_ref().and_then(|v| v.as_str()),
            Some("compact")
        );

        // Default: pi's `full`.
        assert_eq!(build(None).description(), SUBAGENT_TOOL_DESCRIPTION);
        // Configured `compact`: the short form, which is what makes the knob worth setting.
        assert_eq!(
            build(config.tool_description_mode.clone()).description(),
            COMPACT_SUBAGENT_TOOL_DESCRIPTION
        );
        assert!(
            build(config.tool_description_mode).description().len()
                < SUBAGENT_TOOL_DESCRIPTION.len(),
            "the compact form exists to save context"
        );

        // A file override REPLACES the description — and cannot drop the safety guidance.
        std::fs::create_dir_all(dir.path().join(".cyrup")).expect("project config dir");
        std::fs::write(
            dir.path().join(".cyrup").join(CUSTOM_TOOL_DESCRIPTION_FILE),
            "House rule: delegate only to the reviewer agent.",
        )
        .expect("write override");
        let custom = build(Some(serde_json::json!("custom")));
        assert_eq!(
            custom.description(),
            format!(
                "House rule: delegate only to the reviewer agent.\n\n{SUBAGENT_SAFETY_GUIDANCE}"
            )
        );

        // The child-safe registration is NOT resolved — upstream's fanout child builds its own
        // literal (`extension/fanout-child.ts:159` @v0.34.0) and never calls the resolver.
        assert_eq!(
            SubagentTool::new_child_safe(Arc::clone(&executor), dir.path().to_path_buf())
                .description(),
            CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION
        );
    }

    /// pi's public execution boundary — `executor.executePublic(...)`, which BOTH model-facing
    /// registrations call at v0.43.0 (`extension/index.ts:508,532`; `extension/fanout-child.ts:184`)
    /// where v0.34.0 called `executor.execute(...)`.
    ///
    /// Two of its effects are independent of the `workflowScript` cutover the rest of it
    /// implements, and both were divergences here (see
    /// [`normalize_public_subagent_execution`] for the full ported/unported split):
    ///
    /// * an `action` that is present but BLANK is refused with a dedicated message. cyrup's
    ///   `Option<String>` made `Some("")` a present action and answered `unknown subagent action
    ///   ''`, which upstream produces at NEITHER tag — at v0.34.0 `""` is JS-falsy and falls
    ///   through to the execution shapes (`subagent-executor.ts:3602`), at v0.43.0 it is refused
    ///   here;
    /// * a surviving `action` is TRIMMED before dispatch (`subagent-executor.ts:5334-5335`), so
    ///   `" doctor "` routes to `doctor` instead of failing as an unknown action.
    ///
    /// Driven through the real `Tool::execute` on BOTH registrations, because upstream applies the
    /// boundary to both and applying it to only one would be a new divergence.
    #[tokio::test]
    async fn the_public_execution_boundary_refuses_a_blank_action_and_trims_a_real_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = Arc::new(SubagentExecutor::new());
        let root = SubagentTool::new(executor.clone(), dir.path().to_path_buf());
        let child_safe = SubagentTool::new_child_safe(executor, dir.path().to_path_buf());

        for (label, tool) in [("root", &root), ("child-safe", &child_safe)] {
            for blank in ["", "   ", "\t\n "] {
                let err = dispatch_tool(tool, serde_json::json!({ "action": blank }))
                    .await
                    .expect_err("a blank action must be refused, not dispatched");
                assert_eq!(
                    err.to_string(),
                    BLANK_ACTION_REFUSAL,
                    "{label} registration, action={blank:?}"
                );
                assert!(
                    !err.to_string().contains("Unknown action:"),
                    "{label}: a blank action is not an UNKNOWN action — that was the pre-fix text"
                );
            }

            // A padded real action still dispatches, to its own arm.
            let out = dispatch_tool(tool, serde_json::json!({ "action": "  doctor  " }))
                .await
                .expect("a padded action must be trimmed and dispatched");
            assert!(
                tool_text(&out).starts_with("Subagents doctor report"),
                "{label}: ' doctor ' must reach the doctor arm: {}",
                tool_text(&out)
            );

            // …and a genuinely unknown action keeps its own, different refusal.
            let err = dispatch_tool(tool, serde_json::json!({ "action": "nonesuch" }))
                .await
                .expect_err("an unknown action is still an error");
            assert!(
                err.to_string().starts_with("Unknown action: nonesuch."),
                "{label}: {err}"
            );
        }
    }

    /// G90: pi's steer refusals, each with its exact text.
    ///
    /// **Amended.** This test used to assert that an UNRESOLVABLE id produced
    /// `"has no live run directory to steer."` — which pinned a collapse, not a behaviour.
    /// Upstream distinguishes three id-addressed outcomes with three DIFFERENT messages
    /// (`subagent-executor.ts:3211-3219` @v0.34.0), and cyrup reported one of them for all three:
    ///
    /// * `:3217` a live FOREGROUND run → "use action='interrupt' or action='resume' for foreground
    ///   runs" — was reported as a missing run directory, telling a user whose run is on screen
    ///   RIGHT NOW that it does not exist;
    /// * `:3218` nothing resolves at all → `No async run found for '<id>'.` — a typo was likewise
    ///   reported as a lost directory, pointing at the wrong problem;
    /// * `steerAsyncRun:3580` resolved, but the run dir is gone → the message actually quoted here.
    ///
    /// All three are now asserted. Nothing that was asserted before was removed: the blank-message,
    /// no-target and index-out-of-range cases are unchanged, and the third case above keeps the
    /// exact text this test always pinned — it is simply given an input that really is that case.
    #[tokio::test]
    async fn steer_action_enforces_pis_four_refusals() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_running_run(dir.path(), "guardrun0001", &["scout", "auditor"]);
        let tool = scoped_tool(dir.path()).await;

        let blank = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "steer", "id": "guardrun0001", "message": "   " }),
        )
        .await
        .expect_err("a blank message must be refused");
        assert!(blank.to_string().contains("action='steer' requires message."), "{blank}");

        let no_target =
            dispatch_tool(&tool, serde_json::json!({ "action": "steer", "message": "go" }))
                .await
                .expect_err("no id and no dir must be refused");
        assert!(no_target.to_string().contains("action='steer' requires id or dir."), "{no_target}");

        let out_of_range = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "steer", "id": "guardrun0001", "message": "go", "index": 9 }),
        )
        .await
        .expect_err("an out-of-range index must be refused");
        assert!(
            out_of_range
                .to_string()
                .contains("Async run 'guardrun0001' has 2 children. Index 9 is out of range."),
            "{out_of_range}"
        );

        // pi `:3218` — the selector resolves to NOTHING. This is a typo, not a lost directory, and
        // it must say so.
        let missing = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "steer", "id": "nosuchrun0001", "message": "go" }),
        )
        .await
        .expect_err("an unresolvable run must be refused");
        assert!(
            missing.to_string().contains("No async run found for 'nosuchrun0001'."),
            "an unresolvable id must be reported as NOT FOUND, not as a run whose directory went \
             missing; got: {missing}"
        );

        // `steerAsyncRun:3580` — the id DOES resolve (its run directory exists) but the run has
        // neither a status nor a result file, so there is nothing live to steer. This is the case
        // the message quoted below actually describes, and the only one it should ever cover.
        let hollow = default_async_root_in(&crate::paths::Roots::from_env(), dir.path()).join("hollowrun0001");
        std::fs::create_dir_all(&hollow).expect("mkdir hollow run dir");
        let no_dir = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "steer", "id": "hollowrun0001", "message": "go" }),
        )
        .await
        .expect_err("a resolvable run with no live state must be refused");
        assert!(
            no_dir.to_string().contains("has no live run directory to steer."),
            "{no_dir}"
        );

        // pi `:3217` — a live FOREGROUND run. It exists, it is running, and steering simply has no
        // transport to it, so the refusal points at the two verbs that DO work on it.
        let executor = Arc::new(SubagentExecutor::new());
        {
            let mut controls = executor
                .foreground_controls()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.insert(
                "fgrun0001".to_string(),
                ForegroundControlEntry {
                    interrupt: CancelToken::new(),
                    current_agent: Some("scout".to_string()),
                    current_index: Some(0),
                    current_activity_state: None,
                    mode: crate::background::RunMode::Single,
                    description: None,
                    current_tool: None,
                    current_path: None,
                    turn_count: None,
                    tool_count: None,
                    tokens: None,
                    started_at: crate::time::now_epoch_millis(),
                    updated_at: crate::time::now_epoch_millis(),
                },
            );
        }
        let foreground_tool = SubagentTool::new(executor, dir.path().to_path_buf());
        let foreground = dispatch_tool(
            &foreground_tool,
            serde_json::json!({ "action": "steer", "id": "fgrun0001", "message": "go" }),
        )
        .await
        .expect_err("a live foreground run must be refused");
        assert_eq!(
            foreground.to_string(),
            STEER_FOREGROUND_RUN_REFUSAL,
            "a live foreground run must get pi's OWN foreground refusal — pointing at \
             interrupt/resume — not a claim that the run does not exist"
        );
    }

}
