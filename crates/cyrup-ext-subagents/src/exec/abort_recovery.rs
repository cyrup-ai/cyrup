//! SUBA-118 — the abort-recovery decision: a child that died from a provider/transport abort AFTER
//! doing useful work can be resumed once, on the SAME model, against its retained session, instead of
//! burning a fallback candidate on a fresh start that throws that work away.
//!
//! A verbatim port of `src/runs/shared/abort-recovery.ts` @pi-subagents v0.71.0. The whole file is one
//! pure decision function plus its evidence helpers: [`plan_abort_recovery`] reads the attempt's
//! transcript and settle witnesses and answers *resume once* or *settle, for this reason*. It does no
//! I/O, spawns nothing and holds no state, which is what makes the settle ladder testable reason by
//! reason.
//!
//! # The ladder's order is the load-bearing part
//!
//! [`plan_abort_recovery`] walks thirteen settle conditions in upstream's EXACT order
//! (`abort-recovery.ts:100-115`). Two orderings in particular are not incidental:
//!
//! - `stopped`/`interrupted` is tested BEFORE the process-signal rung, so an operator's explicit stop
//!   always wins over any recovery reasoning (`:105`, and the comment upstream puts at `:106-108`).
//! - the process-signal rung is EXEMPTED when the abort looks compaction-induced (`:109`). An external
//!   runner reports a process signal when its own terminal drain reaps a child that a compaction abort
//!   left open — that cleanup signal is not evidence the child was killed, and letting it veto the
//!   resume is precisely the bug the exemption exists to prevent.
//!
//! # `CYRUP-DELTA` — the transcript is JSON, not typed messages
//!
//! Upstream reads `readonly Message[]` off its flat `SingleResult.messages`. This crate has no such
//! field; the equivalent evidence is `crate::exec::progress::AgentProgress::message_end_events`, which
//! holds every `message_end` the child emitted REGARDLESS of role — `assistant`, `user` and
//! `toolResult` alike (`exec/ndjson.rs:284` names all three, and `exec/progress.rs:119` pushes
//! unconditionally). So the inputs here are `&[serde_json::Value]`, walked with the same
//! JSON-inspection convention `exec/output.rs` already uses, and upstream's `record()` guard — which
//! rejects `null` and arrays — becomes `Value::as_object`.

use std::sync::LazyLock;

use regex::Regex;

/// `ABORT_RECOVERY_PROMPT` (`abort-recovery.ts:3`) — verbatim. It is the task text the resumed child
/// is launched with, so every word of it is behaviour.
pub const ABORT_RECOVERY_PROMPT: &str = "The prior run ended from a provider/transport abort after useful progress. Continue from the current files and transcript. Do not restart. Fix any validation failure or write the required report. Finish with final output.";

/// `AbortRecoveryPlan` (`abort-recovery.ts:5-7`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortRecoveryPlan {
    /// Resume the retained child session once, on the same model, with [`ABORT_RECOVERY_PROMPT`].
    /// Upstream carries the prompt in the variant; it is a single constant, so it is not repeated
    /// here.
    Resume,
    /// Do not resume. `reason` is upstream's short settle reason, and `diagnostic` is present exactly
    /// when the abort looked compaction-induced — the case where a settle is worth explaining to the
    /// operator because a resume was very nearly possible (`abort-recovery.ts:94-98`).
    Settle {
        reason: String,
        diagnostic: Option<String>,
    },
}

// CYRUP-DELTA — the three patterns are `regex` crate `Regex`es, compiled once. They carry `(?-u:…)`
// on every `\b` and `\w` because upstream builds these `RegExp`s WITHOUT the `u` flag, which makes
// JavaScript's `\b`/`\w` ASCII-only; the Rust engine's default is Unicode-aware, and the difference is
// observable (see the VL-S13 note on this crate's `regex` dependency, which hit the same distinction
// on a security control). `Regex::is_match` is infallible, so none of these needs a fail-closed
// wrapper.

/// `PROVIDER_ABORT_PATTERN` (`abort-recovery.ts:9`).
static PROVIDER_ABORT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    Regex::new(
        r"(?i)(?:provider|transport|connection|stream|socket|request).*(?:abort|closed|reset|ended|terminated|error|fail)|(?:abort|closed|reset|ended|terminated|error|fail).*(?:provider|transport|connection|stream|socket|request)",
    )
    .unwrap_or_else(|_| unreachable!("PROVIDER_ABORT_PATTERN is a literal and always compiles"))
});

/// `ABORT_ERROR_PATTERN` (`abort-recovery.ts:10`).
static ABORT_ERROR_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?-u:\b)(?:operation|request|response|stream|connection|transport|provider)?\s*(?:was\s+)?aborted(?-u:\b)",
    )
    .unwrap_or_else(|_| unreachable!("ABORT_ERROR_PATTERN is a literal and always compiles"))
});

/// `TOOL_FAILURE_PREFIX` (`abort-recovery.ts:11`) — the SUPPRESSOR. A tool that exited non-zero
/// reports text like `bash failed (exit 1): connection reset`, which
/// [`PROVIDER_ABORT_PATTERN`] matches on its tail. Without this prefix test an ordinary tool failure
/// would read as a provider abort and license a resume.
static TOOL_FAILURE_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?-u:[\w.:@/-])+ failed (?:(?:\(exit \d+\):)|(?:with exit code \d+))(?:\s|$)")
        .unwrap_or_else(|_| unreachable!("TOOL_FAILURE_PREFIX is a literal and always compiles"))
});

/// `isProviderAbortError(error)` (`abort-recovery.ts:13-15`): NOT a tool failure, AND matching either
/// abort pattern. The `TOOL_FAILURE_PREFIX` test runs on the TRIMMED string (upstream's
/// `error.trim()`) while the two abort patterns run on the raw one, which is upstream's asymmetry, not
/// an oversight — the prefix is anchored and would miss a leading-whitespace string otherwise.
fn is_provider_abort_error(error: &str) -> bool {
    !TOOL_FAILURE_PREFIX.is_match(error.trim())
        && (PROVIDER_ABORT_PATTERN.is_match(error) || ABORT_ERROR_PATTERN.is_match(error))
}

/// `record(value)` (`abort-recovery.ts:17-21`): an object, and specifically not `null` and not an
/// array. `Value::as_object` is exactly that set.
fn record(value: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    value.as_object()
}

/// `contentParts(message)` (`abort-recovery.ts:23-25`): the `content` array, or empty when it is
/// absent or not an array.
fn content_parts(message: &serde_json::Map<String, serde_json::Value>) -> &[serde_json::Value] {
    message
        .get("content")
        .and_then(serde_json::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn role_is(message: &serde_json::Map<String, serde_json::Value>, role: &str) -> bool {
    message.get("role").and_then(serde_json::Value::as_str) == Some(role)
}

/// `terminalAssistant(messages)` (`abort-recovery.ts:29-35`): the LAST assistant message and its
/// index. When there is none, the index is `messages.length` — upstream returns that deliberately, and
/// [`plan_abort_recovery`] uses it as the `hasUsefulProgress` scan limit.
fn terminal_assistant(
    messages: &[serde_json::Value],
) -> (Option<&serde_json::Map<String, serde_json::Value>>, usize) {
    for index in (0..messages.len()).rev() {
        if let Some(message) = messages.get(index).and_then(record)
            && role_is(message, "assistant")
        {
            return (Some(message), index);
        }
    }
    (None, messages.len())
}

/// `zeroOutputUsage(message)` (`abort-recovery.ts:37-40`): a `usage` object IS present, and its
/// `output ?? outputTokens ?? 0` is zero. An ABSENT `usage` is NOT zero-output — upstream's
/// `usage !== undefined &&` makes that explicit, and it matters: a message with no usage record is no
/// evidence either way.
fn zero_output_usage(message: &serde_json::Map<String, serde_json::Value>) -> bool {
    let Some(usage) = message.get("usage").and_then(record) else {
        return false;
    };
    let output = usage
        .get("output")
        .or_else(|| usage.get("outputTokens"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    output == 0
}

/// `hasUsefulProgress(messages, terminalIndex)` (`abort-recovery.ts:42-50`): anything BEFORE
/// `terminal_index` that counts as work — an assistant message with non-empty content, or a
/// `toolResult` that is not flagged `isError`. Note `isError !== true`: an ABSENT `isError` counts as
/// useful, only an explicit `true` does not.
fn has_useful_progress(messages: &[serde_json::Value], terminal_index: usize) -> bool {
    for index in 0..terminal_index {
        let Some(message) = messages.get(index).and_then(record) else {
            continue;
        };
        if role_is(message, "assistant") && !content_parts(message).is_empty() {
            return true;
        }
        if role_is(message, "toolResult")
            && message.get("isError").and_then(serde_json::Value::as_bool) != Some(true)
        {
            return true;
        }
    }
    false
}

/// `hasUnresolvedToolCall(messages)` (`abort-recovery.ts:52-66`): every assistant `toolCall` part's
/// non-empty string `id` is pending until a `toolResult` message names it in `toolCallId`. Anything
/// still pending means the transcript ends mid-tool, which is not a state to resume from.
fn has_unresolved_tool_call(messages: &[serde_json::Value]) -> bool {
    let mut pending: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for raw_message in messages {
        let Some(message) = record(raw_message) else {
            continue;
        };
        if role_is(message, "assistant") {
            for raw_part in content_parts(message) {
                let Some(part) = record(raw_part) else {
                    continue;
                };
                if part.get("type").and_then(serde_json::Value::as_str) == Some("toolCall")
                    && let Some(id) = part.get("id").and_then(serde_json::Value::as_str)
                    && !id.is_empty()
                {
                    pending.insert(id);
                }
            }
        } else if role_is(message, "toolResult")
            && let Some(id) = message
                .get("toolCallId")
                .and_then(serde_json::Value::as_str)
        {
            pending.remove(id);
        }
    }
    !pending.is_empty()
}

/// `planAbortRecovery`'s input object (`abort-recovery.ts:68-84`). Every optional upstream field is a
/// plain `bool`/`Option` here because a JS `undefined` and a `false` are indistinguishable at every
/// one of these rungs (each is read only as a truthiness test).
#[derive(Debug, Default)]
pub struct AbortRecoveryInput<'a> {
    /// Upstream's `messages` — see this module's `CYRUP-DELTA` on the transcript shape.
    pub messages: &'a [serde_json::Value],
    /// `processSignal`: the signal an external runner reported, if any.
    pub process_signal: Option<&'a str>,
    /// `sessionAvailable`: upstream's `Boolean(options.sessionFile && existsSync(options.sessionFile))`
    /// — the retained child session both exists and is on disk. There is nothing to resume without it.
    pub session_available: bool,
    /// `alreadyResumed`: this candidate has already spent its one resume.
    pub already_resumed: bool,
    pub stopped: bool,
    pub interrupted: bool,
    pub timed_out: bool,
    pub tool_budget_exhausted: bool,
    pub usage_budget_exhausted: bool,
    pub structured_output_failed: bool,
    pub acceptance_failed: bool,
    /// `currentTool`: a tool still in flight when the attempt settled.
    pub current_tool: Option<&'a str>,
    /// `afterCompactionSettlement`: the child settled its drain while a compaction was in progress —
    /// the marker that makes this abort look compaction-induced.
    pub after_compaction_settlement: bool,
}

/// `planAbortRecovery(input)` (`abort-recovery.ts:68-116`). The settle ladder, in upstream's exact
/// order; see this module's header for the two orderings that are load-bearing.
#[must_use]
pub fn plan_abort_recovery(input: &AbortRecoveryInput<'_>) -> AbortRecoveryPlan {
    let (message, terminal_index) = terminal_assistant(input.messages);
    let terminal_error = message
        .and_then(|m| m.get("errorMessage"))
        .and_then(serde_json::Value::as_str);
    // `:88-90` — an EMPTY terminal assistant message that also reported zero output tokens: the child
    // produced nothing on its last turn.
    let empty_zero_usage_terminal =
        message.is_some_and(|m| content_parts(m).is_empty() && zero_output_usage(m));
    // `:91-92` — `stopReason: "aborted"`, or a `stopReason: "error"` whose own message reads as a
    // provider abort.
    let stop_reason = message
        .and_then(|m| m.get("stopReason"))
        .and_then(serde_json::Value::as_str);
    let terminal_assistant_abort = stop_reason == Some("aborted")
        || (stop_reason == Some("error") && terminal_error.is_some_and(is_provider_abort_error));
    let abort_candidate = empty_zero_usage_terminal && terminal_assistant_abort;
    let compaction_abort_candidate = input.after_compaction_settlement && abort_candidate;

    // `:94-98` — the diagnostic rides on EVERY settle reason, but only for a compaction-looking
    // abort. Upstream's template, verbatim, including the trailing period.
    let settle = |reason: &str| AbortRecoveryPlan::Settle {
        reason: reason.to_string(),
        diagnostic: compaction_abort_candidate.then(|| {
            format!("Compaction-induced child abort could not be resumed safely: {reason}.")
        }),
    };

    if input.already_resumed {
        return settle("resume already attempted");
    }
    if !input.session_available {
        return settle("retained session unavailable");
    }
    // `:105` — BEFORE the process-signal rung: an explicit stop always wins.
    if input.stopped || input.interrupted {
        return settle("explicit stop or interrupt");
    }
    // `:106-109` — upstream's own comment: "An external-job runner reports a process signal when its
    // terminal drain ends a child left open by a compaction abort. The compaction + settlement markers
    // make that cleanup signal non-authoritative; explicit stop/interrupt still wins."
    if input.process_signal.is_some() && !compaction_abort_candidate {
        return settle("process terminated by signal");
    }
    if input.timed_out {
        return settle("elapsed timeout");
    }
    if input.tool_budget_exhausted || input.usage_budget_exhausted {
        return settle("budget exhausted");
    }
    if input.structured_output_failed {
        return settle("structured output failure");
    }
    if input.acceptance_failed {
        return settle("acceptance failure");
    }
    if let Some(tool) = input.current_tool.filter(|t| !t.is_empty()) {
        // `:113` — `input.currentTool.slice(0, 128)`. JS `slice` counts UTF-16 code units; this counts
        // BYTES and steps back to a char boundary, because a Rust string cannot be split mid-char.
        // The cap exists to bound an operator-facing message, so the boundary handling is the only
        // thing that differs and it differs only for a >128-byte non-ASCII tool name.
        let mut end = tool.len().min(128);
        while end > 0 && !tool.is_char_boundary(end) {
            end -= 1;
        }
        let truncated = tool.get(..end).unwrap_or(tool);
        return settle(&format!("active tool '{truncated}' remains in flight"));
    }
    if has_unresolved_tool_call(input.messages) {
        return settle("unresolved tool call remains in transcript");
    }
    if !input.after_compaction_settlement {
        return settle("compaction settlement not verified");
    }
    if !abort_candidate {
        return settle("terminal assistant abort evidence not verified");
    }
    // `:117` — the scan limit is the terminal index only when that terminal message was itself the
    // empty zero-usage one; otherwise the whole transcript counts as prior progress.
    let progress_limit = if empty_zero_usage_terminal {
        terminal_index
    } else {
        input.messages.len()
    };
    if !has_useful_progress(input.messages, progress_limit) {
        return settle("no useful prior progress");
    }
    AbortRecoveryPlan::Resume
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A terminal assistant message that carries every piece of abort evidence: empty content, zero
    /// output tokens, `stopReason: "aborted"`.
    fn aborted_terminal() -> serde_json::Value {
        json!({
            "role": "assistant",
            "content": [],
            "usage": { "output": 0 },
            "stopReason": "aborted",
        })
    }

    /// A non-error tool result — `hasUsefulProgress`'s second witness (`:47`).
    fn tool_result(id: &str) -> serde_json::Value {
        json!({ "role": "toolResult", "toolCallId": id, "content": [{ "type": "text", "text": "ok" }] })
    }

    /// The transcript that earns a resume: prior useful work, then an aborted empty terminal turn.
    fn resumable_messages() -> Vec<serde_json::Value> {
        vec![tool_result("t1"), aborted_terminal()]
    }

    /// An input with every settle witness clear and the compaction marker set — the shape that
    /// reaches the bottom of the ladder.
    fn resumable<'a>(messages: &'a [serde_json::Value]) -> AbortRecoveryInput<'a> {
        AbortRecoveryInput {
            messages,
            session_available: true,
            after_compaction_settlement: true,
            ..AbortRecoveryInput::default()
        }
    }

    fn settle_reason(plan: &AbortRecoveryPlan) -> Option<&str> {
        match plan {
            AbortRecoveryPlan::Settle { reason, .. } => Some(reason.as_str()),
            AbortRecoveryPlan::Resume => None,
        }
    }

    fn diagnostic(plan: &AbortRecoveryPlan) -> Option<&str> {
        match plan {
            AbortRecoveryPlan::Settle { diagnostic, .. } => diagnostic.as_deref(),
            AbortRecoveryPlan::Resume => None,
        }
    }

    /// The bottom of the ladder: every witness clear, compaction verified, abort evidence present and
    /// prior progress found (`abort-recovery.ts:119`).
    #[test]
    fn a_compaction_abort_after_useful_progress_resumes() {
        let messages = resumable_messages();
        assert_eq!(
            plan_abort_recovery(&resumable(&messages)),
            AbortRecoveryPlan::Resume
        );
    }

    /// SUBA-118 test (c) — every settle rung, in upstream's order, each reached by setting exactly one
    /// witness on the otherwise-resumable input. A rung tested out of order would pass for the wrong
    /// reason, so each case asserts the SPECIFIC reason, not merely that it settled.
    #[test]
    fn each_settle_rung_reports_its_own_reason_in_upstream_order() {
        let messages = resumable_messages();

        let with = |f: &dyn Fn(&mut AbortRecoveryInput<'_>)| {
            let mut input = resumable(&messages);
            f(&mut input);
            plan_abort_recovery(&input)
        };

        // `:104`
        assert_eq!(
            settle_reason(&with(&|i| i.already_resumed = true)),
            Some("resume already attempted")
        );
        // `:105`
        assert_eq!(
            settle_reason(&with(&|i| i.session_available = false)),
            Some("retained session unavailable")
        );
        // `:106`
        assert_eq!(
            settle_reason(&with(&|i| i.stopped = true)),
            Some("explicit stop or interrupt")
        );
        assert_eq!(
            settle_reason(&with(&|i| i.interrupted = true)),
            Some("explicit stop or interrupt")
        );
        // `:110`
        assert_eq!(
            settle_reason(&with(&|i| i.timed_out = true)),
            Some("elapsed timeout")
        );
        // `:111` — both budget flags fold to ONE reason.
        assert_eq!(
            settle_reason(&with(&|i| i.tool_budget_exhausted = true)),
            Some("budget exhausted")
        );
        assert_eq!(
            settle_reason(&with(&|i| i.usage_budget_exhausted = true)),
            Some("budget exhausted")
        );
        // `:112`
        assert_eq!(
            settle_reason(&with(&|i| i.structured_output_failed = true)),
            Some("structured output failure")
        );
        // `:113`
        assert_eq!(
            settle_reason(&with(&|i| i.acceptance_failed = true)),
            Some("acceptance failure")
        );
        // `:114` — the in-flight tool's name is interpolated.
        assert_eq!(
            settle_reason(&with(&|i| i.current_tool = Some("bash"))),
            Some("active tool 'bash' remains in flight")
        );
    }

    /// `:105` beats `:109`: an explicit stop settles as a STOP even when a signal is also present, and
    /// even when the abort looks compaction-induced. Upstream states this in its own comment at `:108`
    /// (*"explicit stop/interrupt still wins"*).
    #[test]
    fn an_explicit_stop_outranks_the_process_signal_rung() {
        let messages = resumable_messages();
        let mut input = resumable(&messages);
        input.stopped = true;
        input.process_signal = Some("SIGTERM");
        assert_eq!(
            settle_reason(&plan_abort_recovery(&input)),
            Some("explicit stop or interrupt")
        );
    }

    /// SUBA-118 test (c), the load-bearing exemption (`abort-recovery.ts:109`). A process signal
    /// normally settles the attempt — but NOT when the abort looks compaction-induced, because that
    /// signal is an external runner's terminal drain reaping a child the compaction abort left open.
    /// Without the exemption a cleanup signal would veto every compaction resume.
    #[test]
    fn a_process_signal_is_exempted_only_when_the_abort_looks_compaction_induced() {
        let messages = resumable_messages();

        // Signal + full compaction evidence ⇒ the signal is non-authoritative, so the ladder runs on
        // and resumes.
        let mut with_compaction = resumable(&messages);
        with_compaction.process_signal = Some("SIGTERM");
        assert_eq!(
            plan_abort_recovery(&with_compaction),
            AbortRecoveryPlan::Resume
        );

        // The SAME signal with the compaction marker absent ⇒ the signal settles it. (The reason is
        // the signal's, not `compaction settlement not verified`, which proves the rung ORDER: the
        // signal rung sits above the compaction-verification rung.)
        let mut without_compaction = resumable(&messages);
        without_compaction.process_signal = Some("SIGTERM");
        without_compaction.after_compaction_settlement = false;
        assert_eq!(
            settle_reason(&plan_abort_recovery(&without_compaction)),
            Some("process terminated by signal")
        );

        // Compaction marker set but the ABORT evidence missing ⇒ not a compaction abort CANDIDATE, so
        // the signal is authoritative again.
        let plain = vec![
            tool_result("t1"),
            json!({ "role": "assistant", "content": [], "usage": { "output": 0 }, "stopReason": "stop" }),
        ];
        let mut no_abort = resumable(&plain);
        no_abort.process_signal = Some("SIGTERM");
        assert_eq!(
            settle_reason(&plan_abort_recovery(&no_abort)),
            Some("process terminated by signal")
        );
    }

    /// `:116` — the compaction-verification rung, and the exact diagnostic sentence it does NOT carry
    /// (the abort is a candidate only WITH the marker, so a settle for the marker's absence is never
    /// annotated).
    #[test]
    fn an_unverified_compaction_settles_without_a_diagnostic() {
        let messages = resumable_messages();
        let mut input = resumable(&messages);
        input.after_compaction_settlement = false;
        let plan = plan_abort_recovery(&input);
        assert_eq!(
            settle_reason(&plan),
            Some("compaction settlement not verified")
        );
        assert_eq!(
            diagnostic(&plan),
            None,
            "`compactionAbortCandidate` requires the marker, so this settle carries no diagnostic"
        );
    }

    /// `:94-98` — the diagnostic template, verbatim. It appears on a compaction-looking abort's settle
    /// for ANY reason, which is what makes a near-miss explicable to the operator.
    #[test]
    fn a_compaction_looking_abort_annotates_every_settle_reason() {
        let messages = resumable_messages();
        let mut input = resumable(&messages);
        input.timed_out = true;
        assert_eq!(
            diagnostic(&plan_abort_recovery(&input)),
            Some("Compaction-induced child abort could not be resumed safely: elapsed timeout.")
        );
    }

    /// `:117` — an unresolved `toolCall` (no `toolResult` naming its id) is not a state to resume from.
    /// The rung sits ABOVE the compaction-verification rung, which this also pins.
    #[test]
    fn an_unresolved_tool_call_settles() {
        let messages = vec![
            tool_result("t1"),
            json!({
                "role": "assistant",
                "content": [{ "type": "toolCall", "id": "t9", "name": "bash" }],
            }),
            aborted_terminal(),
        ];
        assert_eq!(
            settle_reason(&plan_abort_recovery(&resumable(&messages))),
            Some("unresolved tool call remains in transcript")
        );

        // Resolved by a matching `toolResult`, the same transcript resumes.
        let resolved = vec![
            json!({
                "role": "assistant",
                "content": [{ "type": "toolCall", "id": "t9", "name": "bash" }],
            }),
            tool_result("t9"),
            aborted_terminal(),
        ];
        assert_eq!(
            plan_abort_recovery(&resumable(&resolved)),
            AbortRecoveryPlan::Resume
        );
    }

    /// `:118` — the abort-evidence rung. Each of the three witnesses is individually necessary.
    #[test]
    fn missing_abort_evidence_settles() {
        let cases = [
            // Non-empty content: the child produced something, so this is not an empty abort.
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi" }], "usage": { "output": 0 }, "stopReason": "aborted" }),
            // Non-zero output tokens.
            json!({ "role": "assistant", "content": [], "usage": { "output": 7 }, "stopReason": "aborted" }),
            // A `stopReason` that is neither `aborted` nor a provider-abort `error`.
            json!({ "role": "assistant", "content": [], "usage": { "output": 0 }, "stopReason": "stop" }),
            // `:39` — an ABSENT `usage` is not zero-output.
            json!({ "role": "assistant", "content": [], "stopReason": "aborted" }),
        ];
        for terminal in cases {
            let messages = vec![tool_result("t1"), terminal.clone()];
            assert_eq!(
                settle_reason(&plan_abort_recovery(&resumable(&messages))),
                Some("terminal assistant abort evidence not verified"),
                "{terminal}"
            );
        }
    }

    /// `:92` — a `stopReason: "error"` counts as an abort when its own `errorMessage` reads as a
    /// provider abort, and does not otherwise.
    #[test]
    fn an_error_stop_reason_counts_as_an_abort_only_for_a_provider_abort_message() {
        let abort = vec![
            tool_result("t1"),
            json!({ "role": "assistant", "content": [], "usage": { "output": 0 }, "stopReason": "error", "errorMessage": "provider connection reset" }),
        ];
        assert_eq!(
            plan_abort_recovery(&resumable(&abort)),
            AbortRecoveryPlan::Resume
        );

        let other = vec![
            tool_result("t1"),
            json!({ "role": "assistant", "content": [], "usage": { "output": 0 }, "stopReason": "error", "errorMessage": "the file was not found" }),
        ];
        assert_eq!(
            settle_reason(&plan_abort_recovery(&resumable(&other))),
            Some("terminal assistant abort evidence not verified")
        );
    }

    /// SUBA-118 test (c), the `TOOL_FAILURE_PREFIX` suppression (`abort-recovery.ts:11,14`).
    /// `bash failed (exit 1): connection reset` matches `PROVIDER_ABORT_PATTERN` on its TAIL, so
    /// without the prefix test an ordinary non-zero tool exit would read as a provider abort and
    /// license a resume.
    #[test]
    fn a_tool_failure_message_does_not_read_as_a_provider_abort() {
        for tool_failure in [
            "bash failed (exit 1): connection reset",
            "bash failed with exit code 2 connection was aborted",
            "  edit_file failed (exit 127): stream terminated",
            "mcp:server/tool failed (exit 1): socket closed",
        ] {
            assert!(
                !is_provider_abort_error(tool_failure),
                "must not read as a provider abort: {tool_failure}"
            );
        }
        // …while the genuine provider aborts still do.
        for abort in [
            "provider connection reset",
            "the request was aborted",
            "stream terminated unexpectedly",
            "transport error",
            "Operation aborted",
        ] {
            assert!(
                is_provider_abort_error(abort),
                "must read as an abort: {abort}"
            );
        }
        // A tool name that does not match the prefix grammar is not suppressed: the prefix is anchored
        // and requires the exact `<name> failed (exit N):` / `with exit code N` shape.
        assert!(is_provider_abort_error(
            "something went wrong: connection reset"
        ));
    }

    /// `:119-120` — the useful-progress rung, and the `progressLimit` subtlety: the terminal empty
    /// message is EXCLUDED from the scan (it is the abort itself, not progress), so a transcript whose
    /// only message is that terminal abort has no prior progress.
    #[test]
    fn an_abort_with_no_prior_progress_settles() {
        let only_terminal = vec![aborted_terminal()];
        assert_eq!(
            settle_reason(&plan_abort_recovery(&resumable(&only_terminal))),
            Some("no useful prior progress")
        );

        // An `isError: true` tool result is not progress either (`:47`'s `isError !== true`).
        let errored = vec![
            json!({ "role": "toolResult", "toolCallId": "t1", "isError": true }),
            aborted_terminal(),
        ];
        assert_eq!(
            settle_reason(&plan_abort_recovery(&resumable(&errored))),
            Some("no useful prior progress")
        );

        // An empty-content assistant message is not progress (`:46`).
        let empty_assistant = vec![
            json!({ "role": "assistant", "content": [] }),
            aborted_terminal(),
        ];
        assert_eq!(
            settle_reason(&plan_abort_recovery(&resumable(&empty_assistant))),
            Some("no useful prior progress")
        );
    }

    /// `:37-40` — `outputTokens` is the accepted alias for `output`.
    #[test]
    fn output_tokens_is_accepted_as_an_alias_for_output() {
        let messages = vec![
            tool_result("t1"),
            json!({ "role": "assistant", "content": [], "usage": { "outputTokens": 0 }, "stopReason": "aborted" }),
        ];
        assert_eq!(
            plan_abort_recovery(&resumable(&messages)),
            AbortRecoveryPlan::Resume
        );
    }

    /// `:19` — `record()` rejects `null` and arrays, so a malformed transcript entry is skipped rather
    /// than mistaken for a message.
    #[test]
    fn malformed_transcript_entries_are_skipped_not_misread() {
        let messages = vec![
            serde_json::Value::Null,
            json!([1, 2, 3]),
            json!("a string"),
            tool_result("t1"),
            aborted_terminal(),
        ];
        assert_eq!(
            plan_abort_recovery(&resumable(&messages)),
            AbortRecoveryPlan::Resume
        );
    }

    /// `:113` — the 128-char cap on the interpolated tool name.
    #[test]
    fn a_long_tool_name_is_truncated_in_the_settle_reason() {
        let messages = resumable_messages();
        let long = "t".repeat(200);
        let mut input = resumable(&messages);
        input.current_tool = Some(&long);
        let reason = settle_reason(&plan_abort_recovery(&input))
            .expect("settles")
            .to_string();
        assert_eq!(
            reason,
            format!("active tool '{}' remains in flight", "t".repeat(128))
        );
    }
}
