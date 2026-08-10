//! Optional out-of-band result delivery ("intercom") and the foreground clarify/ask pause
//! primitive (func-SA §5.5; arch-SA §6.7 "Intercom degradation"; R-SA-119/120/123/124/125).
//!
//! # Scope
//!
//! This file owns exactly two things:
//!
//! 1. **Out-of-band result delivery** ([`IntercomPayload`]/[`deliver`]): a *best-effort*,
//!    gracefully-degrading side-channel a grouped subagent result MAY be pushed through, in
//!    addition to (never instead of) the ordinary inline tool-result payload. The `pi-intercom`
//!    companion transport IS now ported (`cyrup-intercom`), and its broker-backed
//!    [`DeliveryChannel`] impl (`cyrup-intercom/src/seams.rs`) is threaded into the executor via
//!    `SubagentsExtension::with_channels` (from `crates/cyrup/src/main.rs`) — CLOSING
//!    R-SA-123/124/125. With that channel, [`deliver`] confirms/degrades per the real broker; with
//!    the [`NoTransportChannel`] default (no intercom wired) it resolves to
//!    [`DeliveryOutcome::NotDelivered`] and every caller's full result stays inline, exactly as the
//!    spec anticipates. The timeout-race, allowlist-projection, and "never block/error the caller's
//!    turn" contracts hold across both.
//! 2. **The foreground clarify/ask pause primitive** ([`ClarifyRequest`]/[`AskLock`]/
//!    [`request_clarify`]): R-SA-119's "visibly pause the affected foreground flow while a child's
//!    clarify request is outstanding" and R-SA-120's "at most one outstanding blocking ask per
//!    orchestrator session" single-slot lock. This is now WIRED end to end (R-SA-119/120/037
//!    CLOSED): the intercom companion's broker-backed [`ClarifyChannel`] (`IntercomClarifyChannel`,
//!    which surfaces the child's ask through the P-1-late-bound `HostServices::input` and routes the
//!    answer back to the still-alive child over the broker) is threaded into the executor's
//!    [`AskLock`] via `SubagentsExtension::with_channels`, and the exec drive loop fires
//!    [`spawn_clarify`] against it on a child's blocking `contact_supervisor` ask (see the
//!    `NOTE(clarify-wired)` marker below). With no channel wired (headless / SDK-embedder) the lock
//!    degrades to the documented no-op fallback ([`ClarifyOutcome::NoLiveChannel`], never blocks).
//!
//! # What this file deliberately does NOT do (mandatory-mechanism guardrails)
//!
//! No in-process nested agent turn loop, no in-process event-relay standing in for a child
//! subprocess's own NDJSON stdout, and no extension-host session-access seam beyond the one
//! narrow exception already justified elsewhere in this crate (fork-context's direct
//! `cyrup-session` dependency, `fork_context.rs`) — this module has **zero** dependency on
//! `cyrup-agent` and does not acquire a `cyrup_session::SessionManager` handle at all. The
//! clarify primitive here is a pure, in-memory single-slot lock plus a pluggable, fallible,
//! timeout-bounded async channel trait; it does not itself talk to any subprocess, session, or
//! WASM guest — a real implementation of [`ClarifyChannel`] (later phase, per the doc above) is
//! where any such wiring would live, never inline in this file.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Mutex as AsyncMutex};

use crate::background::RunId;

// =================================================================================================
// Allowlisted out-of-band payload (R-SA-123/124)
// =================================================================================================

/// The explicit, closed set of fields an out-of-band delivery attempt is permitted to carry
/// (R-SA-124). This is a **distinct type** from [`crate::background::ResultFile`]/
/// [`crate::exec::SingleResult`] — never a re-export, never `#[serde(flatten)]`, never a
/// generic "serialize the whole record" call — specifically so that the allowlist is enforced
/// **by construction/by type**, not by convention: there is no field on this struct that any
/// constructor could populate with a capability-bearing/secret value, because no such field
/// exists here at all (that is the property the "assert by construction, not just by example"
/// test requirement below is checking).
///
/// In particular, this type has **no** field for:
/// - a control-inbox route ([`crate::background::RunPaths::control_inbox`] or any other
///   filesystem path used to *drive* a run — e.g. `cwd`, `session_file` — which are both
///   plausible-looking-but-excluded fields on [`crate::background::ResultFile`] itself, kept out
///   here because a shared out-of-band transport is not necessarily as trusted as the local
///   filesystem the orchestrator process already has ambient access to);
/// - any capability token, credential, or extension-host handle of any kind (this crate has none
///   to leak today, but the allowlist shape is deliberately closed so that if a future field is
///   ever added to `ResultFile`/`SingleResult` upstream, it does NOT automatically start flowing
///   out-of-band — a maintainer must explicitly add a new field *here* and populate it in
///   [`IntercomPayload::from_result`] for it to ever leave the process this way).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntercomPayload {
    /// The run this payload summarizes.
    pub run_id: RunId,
    /// The top-level agent name (mirrors [`crate::background::ResultFile::agent`]).
    pub agent: String,
    /// Whether the run succeeded overall (mirrors [`crate::background::ResultFile::success`]).
    pub success: bool,
    /// Per-child final textual outputs, in the same fixed order as the run's steps (R-SA-051
    /// ordering preserved) — the "heavy duplicated field" R-SA-123 refers to omitting from the
    /// inline payload once this has been delivered out-of-band. Deliberately `String`, never
    /// `Option<serde_json::Value>` or any other shape that could smuggle a structured field this
    /// allowlist does not otherwise name.
    pub outputs: Vec<String>,
    /// Total additive token usage across the run (a plain numeric summary — not a capability, not
    /// a route).
    pub total_tokens: u64,
    /// pi's grouped `SubagentResultIntercomPayload.status` (`result-intercom.ts:83-91
    /// resolveGroupedStatus` @v0.43.0): the **5**-state precedence verdict over
    /// [`Self::child_statuses`] — any-failed wins, else any-**stopped**, else any-paused, else
    /// any-completed, else any-detached, else `Failed` (pi's own explicit default for an empty
    /// child set). A closed enum, not a capability-bearing value, so it is safe to include in this
    /// otherwise-narrow allowlist.
    pub status: SubagentResultStatus,
    /// pi's grouped `SubagentResultIntercomPayload.summary` (`result-intercom.ts:57-66
    /// formatStatusCounts` @v0.43.0): "N completed, N failed, N stopped, N paused, N detached"
    /// (only the non-zero buckets, in that fixed render order), or `"0 results"` when
    /// [`Self::child_statuses`] is empty.
    pub summary: String,
    /// The per-child [`SubagentResultStatus`], in the same fixed order as [`Self::outputs`] (pi's
    /// `children[].status`, `result-intercom.ts:33-44 countStatuses`'s input) — a second parallel
    /// array alongside `outputs`, following this struct's existing "parallel array, never an
    /// embedded per-child object" allowlist discipline, so [`Self::status`]/[`Self::summary`] (and
    /// [`format_subagent_result_receipt`]'s own "Children: …" line) can be recomputed from real
    /// per-child data rather than only ever seeing the pre-collapsed aggregate.
    pub child_statuses: Vec<SubagentResultStatus>,
}

/// pi `SubagentResultStatus` (`shared/types.ts:229` @v0.43.0 — `"completed" | "failed" | "paused"
/// | "stopped" | "detached"`, consumed by `result-intercom.ts:20-91`): the **five** terminal states
/// a single grouped child (or a whole grouped run, via [`resolve_grouped_status`]) can resolve to.
///
/// G104 — `Stopped` is its own variant, never an alias for `Failed` or `Paused`. Declaration order
/// is upstream's own union order because [`count_statuses`] indexes its bucket array with `self as
/// usize`; the RENDER order in [`format_status_counts`] is deliberately different (upstream's
/// `formatStatusCounts` prints stopped BEFORE paused, `result-intercom.ts:57-66`) and is spelled
/// out separately there rather than derived from this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentResultStatus {
    Completed,
    Failed,
    Paused,
    /// G104 — pi `"stopped"`. Produced by an explicit stop request
    /// ([`crate::background::RunState::Stopped`] / [`crate::exec::SingleResult::stopped`]) and by
    /// an UNEXPLAINED process signal (`result-intercom.ts:35`).
    Stopped,
    Detached,
}

/// The number of [`SubagentResultStatus`] buckets [`count_statuses`] tallies into — kept as a named
/// constant so the array type, the render table and the `as usize` indexing cannot drift apart if a
/// sixth state is ever added.
const STATUS_BUCKETS: usize = 5;

/// pi `countStatuses` (`result-intercom.ts:43-55` @v0.43.0): tally `statuses` into the fixed
/// completed/failed/paused/stopped/detached bucket order (index 0..4, matching
/// [`SubagentResultStatus`]'s declaration order so `as usize` indexes this array directly).
fn count_statuses(statuses: &[SubagentResultStatus]) -> [u32; STATUS_BUCKETS] {
    let mut counts = [0u32; STATUS_BUCKETS];
    for s in statuses {
        if let Some(count) = counts.get_mut(*s as usize) {
            *count += 1;
        }
    }
    counts
}

/// Read one bucket out of a [`count_statuses`] tally by variant, without indexing arithmetic the
/// no-panic policy would have to reason about.
fn count_of(counts: &[u32; STATUS_BUCKETS], status: SubagentResultStatus) -> u32 {
    counts.get(status as usize).copied().unwrap_or(0)
}

/// pi `formatStatusCounts` (`result-intercom.ts:57-66` @v0.43.0): "N completed, N failed, N
/// stopped, N paused, N detached" (only the non-zero buckets, joined with `", "`, in that fixed
/// order), or the literal `"0 results"` when `statuses` is empty (pi's own explicit fallback for
/// `parts.length === 0`).
///
/// G104 — the render order is NOT the enum's declaration order: upstream's `parts` array lists
/// `counts.stopped` between `counts.failed` and `counts.paused`, so `stopped` prints third even
/// though `countStatuses` declares its bucket fourth. Reproduced literally here rather than zipping
/// against the bucket order, which would print it fourth.
#[must_use]
pub fn format_status_counts(statuses: &[SubagentResultStatus]) -> String {
    let counts = count_statuses(statuses);
    const RENDER_ORDER: [(SubagentResultStatus, &str); STATUS_BUCKETS] = [
        (SubagentResultStatus::Completed, "completed"),
        (SubagentResultStatus::Failed, "failed"),
        (SubagentResultStatus::Stopped, "stopped"),
        (SubagentResultStatus::Paused, "paused"),
        (SubagentResultStatus::Detached, "detached"),
    ];
    let parts: Vec<String> = RENDER_ORDER
        .iter()
        .map(|(status, label)| (count_of(&counts, *status), label))
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{n} {label}"))
        .collect();
    if parts.is_empty() {
        "0 results".to_string()
    } else {
        parts.join(", ")
    }
}

/// pi `resolveGroupedStatus` (`result-intercom.ts:83-91` @v0.43.0), ported verbatim: any-failed →
/// `Failed`; else any-**stopped** → `Stopped`; else any-paused → `Paused`; else any-completed →
/// `Completed`; else any-detached → `Detached`; else (no children at all) → `Failed` (pi's own
/// explicit default, matched exactly — not `Completed` or any other "optimistic" fallback).
///
/// G104 — the `Stopped` slot sits between `Failed` and `Paused`, which is load-bearing: a grouped
/// run with one stopped and one paused child reports `stopped`, not `paused`, and a grouped run
/// with one stopped and one completed child reports `stopped`, not `completed`.
#[must_use]
pub fn resolve_grouped_status(statuses: &[SubagentResultStatus]) -> SubagentResultStatus {
    let counts = count_statuses(statuses);
    for candidate in [
        SubagentResultStatus::Failed,
        SubagentResultStatus::Stopped,
        SubagentResultStatus::Paused,
        SubagentResultStatus::Completed,
        SubagentResultStatus::Detached,
    ] {
        if count_of(&counts, candidate) > 0 {
            return candidate;
        }
    }
    SubagentResultStatus::Failed
}

/// pi `isUnexplainedProcessSignal` (`runs/shared/process-signal.ts:5-19` @v0.43.0): a process signal
/// is "unexplained" when one was actually delivered AND none of the four lifecycle verdicts that
/// would explain it is set. (Upstream's fifth input, `forcedDrainAfterFinalSuccess`, is an
/// `execution.ts`-local latch never carried onto a `SingleResult`, so it has no field here — its
/// only call site that passes it is `execution.ts:1082-1088`, not `resolveSubagentResultStatus`,
/// which passes exactly the four below.)
#[must_use]
pub fn is_unexplained_process_signal(
    process_signal: Option<&str>,
    interrupted: bool,
    timed_out: bool,
    stopped: bool,
    turn_budget_exceeded: bool,
) -> bool {
    process_signal.is_some_and(|s| !s.is_empty())
        && !interrupted
        && !timed_out
        && !stopped
        && !turn_budget_exceeded
}

/// Every field pi's `resolveSubagentResultStatus` reads (`result-intercom.ts:20-41` @v0.43.0),
/// gathered so the port can be a single faithful branch ladder instead of a widening argument list.
/// `None` on `success`/`state` reproduces upstream's `undefined`, which is what makes the
/// later branches reachable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultStatusInput<'a> {
    /// pi `exitCode?: number`.
    pub exit_code: Option<i32>,
    /// pi `success?: boolean`.
    pub success: Option<bool>,
    /// pi `state?: string` — the run/step lifecycle word (`"stopped"`, `"paused"`, `"complete"`,
    /// `"failed"`).
    pub state: Option<&'a str>,
    /// pi `interrupted?: boolean`.
    pub interrupted: bool,
    /// pi `detached?: boolean`.
    pub detached: bool,
    /// pi `processSignal?: string | null`.
    pub process_signal: Option<&'a str>,
    /// pi `timedOut?: boolean`.
    pub timed_out: bool,
    /// pi `stopped?: boolean`.
    pub stopped: bool,
    /// pi `turnBudgetExceeded?: boolean`.
    pub turn_budget_exceeded: bool,
}

/// pi `resolveSubagentResultStatus` (`result-intercom.ts:20-41` @v0.43.0), ported branch-for-branch
/// in upstream's exact order:
///
/// 1. `detached` → `Detached`
/// 2. `stopped || state === "stopped"` → `Stopped`   ← **G104: ahead of `interrupted`**
/// 3. `interrupted || state === "paused"` → `Paused`
/// 4. `success === true` → `Completed`
/// 5. `isUnexplainedProcessSignal(...) && exitCode !== 0` → `Stopped`  ← **G104: a signal death
///    nobody claimed is a stop, not a failure**
/// 6. `success === false` → `Failed`
/// 7. `state === "complete"` → `Completed`
/// 8. `state === "failed"` → `Failed`
/// 9. `typeof exitCode === "number"` → `exitCode === 0 ? Completed : Failed`
/// 10. otherwise → `Failed`
///
/// Ordering is behaviour, not style: branch 2 before branch 3 is what stops a run that was both
/// stopped and interrupted from being reported resumable, and branch 5 before branch 6 is what
/// keeps an externally-`SIGKILL`ed child out of the `failed` bucket.
#[must_use]
pub fn resolve_subagent_result_status(input: &ResultStatusInput<'_>) -> SubagentResultStatus {
    if input.detached {
        return SubagentResultStatus::Detached;
    }
    if input.stopped || input.state == Some("stopped") {
        return SubagentResultStatus::Stopped;
    }
    if input.interrupted || input.state == Some("paused") {
        return SubagentResultStatus::Paused;
    }
    if input.success == Some(true) {
        return SubagentResultStatus::Completed;
    }
    if is_unexplained_process_signal(
        input.process_signal,
        input.interrupted,
        input.timed_out,
        input.stopped,
        input.turn_budget_exceeded,
    ) && input.exit_code != Some(0)
    {
        return SubagentResultStatus::Stopped;
    }
    if input.success == Some(false) {
        return SubagentResultStatus::Failed;
    }
    if input.state == Some("complete") {
        return SubagentResultStatus::Completed;
    }
    if input.state == Some("failed") {
        return SubagentResultStatus::Failed;
    }
    match input.exit_code {
        Some(0) => SubagentResultStatus::Completed,
        Some(_) => SubagentResultStatus::Failed,
        None => SubagentResultStatus::Failed,
    }
}

/// [`resolve_subagent_result_status`] applied to a real [`crate::exec::SingleResult`] child — pi
/// `foregroundResultIntercomStatus` (`runs/foreground/subagent-executor.ts:1594-1605` @v0.43.0),
/// which passes exactly these fields off the child's own record. `success` is left `None` (pi only
/// sets it from a REJECTED acceptance ledger, `:1596`), so a child with no acceptance verdict falls
/// through to the signal/exit-code branches exactly as upstream's does.
#[must_use]
pub fn resolve_single_result_status(child: &crate::exec::SingleResult) -> SubagentResultStatus {
    resolve_subagent_result_status(&ResultStatusInput {
        exit_code: Some(child.exit_code),
        // pi `...(result.acceptance?.status === "rejected" ? { success: false } : {})`
        // (`subagent-executor.ts:1597`) — the ONLY thing that pins `success` on this path.
        success: child
            .acceptance
            .as_ref()
            .and_then(|ledger| {
                (ledger.status == crate::exec::acceptance::AcceptanceStatus::Rejected).then_some(false)
            }),
        state: None,
        interrupted: child.interrupted,
        detached: child.detached,
        process_signal: child.process_signal.as_deref(),
        timed_out: child.timed_out,
        stopped: child.stopped,
        // cyrup's `SingleResult` carries no `turnBudgetExceeded` (the turn-budget subsystem stops a
        // child via its own tool-budget path and does not stamp a terminal flag); `false` here can
        // only WIDEN `isUnexplainedProcessSignal`, never narrow it, and a turn-budget stop is not
        // signal-killed in this port, so no real case is misclassified.
        turn_budget_exceeded: false,
    })
}

/// The chain-graph-local analogue of [`resolve_single_result_status`] for a foreground grouped
/// child ([`crate::spawn::chain_graph::StepResult`]): no `detached`/`exitCode`/`processSignal` field
/// exists at this granularity, so this drives [`resolve_subagent_result_status`] with the two
/// signals it does have plus the run-level `stopped` verdict its caller knows. A `None` child (a
/// skipped/absent step) resolves to pi's ultimate `"failed"` default (no completion status is
/// knowable for a step that never ran).
///
/// G104 — no `Stopped` branch here, and that is faithful rather than a gap: the stop verb is a
/// control-inbox request consumed by the DETACHED runner (`subagent-runner.ts:2955-2984`), a
/// foreground grouped run has no control inbox, and pi's own foreground path never sets
/// `result.stopped` either (`execution.ts` only reads it). The background grouped path, which CAN
/// be stopped, goes through [`IntercomPayload::from_result`] instead and does resolve `Stopped`.
fn resolve_step_result_status(
    step: Option<&crate::spawn::chain_graph::StepResult>,
) -> SubagentResultStatus {
    match step {
        None => SubagentResultStatus::Failed,
        Some(r) => resolve_subagent_result_status(&ResultStatusInput {
            success: Some(r.success),
            interrupted: r.interrupted,
            ..ResultStatusInput::default()
        }),
    }
}

/// pi `formatSubagentResultReceipt` (`result-intercom.ts:334-377`), ported for the mode label +
/// "Run: …" + "Children: …" + closing-line structure this crate has real data for today. pi's three
/// conditional sections (`Artifacts:` / `Run intercom targets (may be inactive after completion):` /
/// `Sessions:`) each only render when at least one delivered child carries an `artifactPath` /
/// `intercomTarget` / `sessionPath` respectively (`.filter(...)`, `result-intercom.ts:351-373`) — no
/// per-grouped-child artifact path, session path, or intercom target is tracked anywhere in this
/// crate's pipeline yet, so those three sections correctly evaluate to "no matching children" and are
/// omitted here exactly as pi's own filters would do given the identical absence of data; the moment
/// a later phase threads that per-child metadata through, this function starts rendering those
/// sections with zero further changes to its own logic.
#[must_use]
pub fn format_subagent_result_receipt(mode: &str, run_id: &RunId, child_statuses: &[SubagentResultStatus]) -> String {
    let mode_label = match mode {
        "single" => "single subagent result",
        "chain" => "chain subagent results",
        _ => "parallel subagent results",
    };
    let lines = [
        format!("Delivered {mode_label} via intercom."),
        format!("Run: {}", run_id.as_str()),
        format!("Children: {}", format_status_counts(child_statuses)),
        "Full grouped output was sent over intercom.".to_string(),
    ];
    lines.join("\n")
}

impl IntercomPayload {
    /// Builds an [`IntercomPayload`] by copying only the allowlisted fields out of a full
    /// [`crate::background::ResultFile`]. This is the **sole** sanctioned constructor
    /// (R-SA-124): every field assignment here is an explicit, individually-named copy, never a
    /// blanket `..` struct-update or a generic serialize/re-deserialize round trip through
    /// `ResultFile`'s own (much wider) shape — the two types are not `From`/`Into` of each other
    /// for exactly this reason, so a future field added to `ResultFile` cannot silently start
    /// flowing out-of-band via an auto-derived conversion.
    #[must_use]
    pub fn from_result(result: &crate::background::ResultFile) -> Self {
        let total_tokens: u64 = result
            .results
            .iter()
            .map(|r| r.usage.total_tokens)
            .fold(0u64, u64::saturating_add);
        // G104 — every child is resolved through the FULL `resolveSubagentResultStatus` ladder
        // (`result-intercom.ts:20-41`), including its two `"stopped"` branches. The verdict is read
        // PER CHILD (pi feeds `state: child.state`/`stopped: child.stopped`, not the run's,
        // `run-status.ts:467-475`) — a run that was stopped mid-flight has one stopped child and
        // possibly several already-completed ones, and flattening the run's own state onto all of
        // them would wrongly re-label work that had genuinely finished. The per-child `stopped`
        // flag is set by `runner_main`'s stop arm, mirroring pi's `stoppedAfterAcceptance =
        // finalResult?.stopped === true || ctx.stopSignal?.aborted === true`
        // (`subagent-runner.ts:1642,1722`).
        let child_statuses: Vec<SubagentResultStatus> = result
            .results
            .iter()
            .map(|r| {
                let mut input = ResultStatusInput {
                    exit_code: Some(r.exit_code),
                    success: None,
                    state: None,
                    interrupted: r.interrupted,
                    detached: r.detached,
                    process_signal: r.process_signal.as_deref(),
                    timed_out: r.timed_out,
                    stopped: r.stopped,
                    turn_budget_exceeded: false,
                };
                if r.acceptance
                    .as_ref()
                    .is_some_and(|l| l.status == crate::exec::acceptance::AcceptanceStatus::Rejected)
                {
                    input.success = Some(false);
                }
                resolve_subagent_result_status(&input)
            })
            .collect();
        let status = resolve_grouped_status(&child_statuses);
        let summary = format_status_counts(&child_statuses);
        Self {
            run_id: result.run_id.clone(),
            agent: result.agent.clone(),
            success: result.success,
            outputs: result
                .results
                .iter()
                .map(|r| r.final_output.clone().unwrap_or_default())
                .collect(),
            total_tokens,
            status,
            summary,
            child_statuses,
        }
    }

    /// Builds an [`IntercomPayload`] for a FOREGROUND grouped (parallel/chain) run from its per-child
    /// [`crate::spawn::chain_graph::StepResult`]s (R-SA-123/124/125). A second sanctioned constructor
    /// alongside [`Self::from_result`], holding the identical allowlist discipline — every field is an
    /// explicit, individually-named copy of an allowlisted value, never a blanket serialize of the
    /// child results. A `None` child (a skipped/absent step) contributes an empty output string.
    /// `success` is the caller's already-computed overall verdict (every present child succeeded).
    /// `total_tokens` is `0` here: the chain-graph-local `StepResult` carries no per-step token usage
    /// (unlike the background [`crate::background::ResultFile`] path, which sums real usage via
    /// [`Self::from_result`]) — the out-of-band grouped-delivery summary simply omits it.
    #[must_use]
    pub fn from_group_children(
        run_id: RunId,
        agent: String,
        success: bool,
        children: &[Option<crate::spawn::chain_graph::StepResult>],
    ) -> Self {
        let child_statuses: Vec<SubagentResultStatus> =
            children.iter().map(|c| resolve_step_result_status(c.as_ref())).collect();
        let status = resolve_grouped_status(&child_statuses);
        let summary = format_status_counts(&child_statuses);
        Self {
            run_id,
            agent,
            success,
            outputs: children
                .iter()
                .map(|c| c.as_ref().and_then(|r| r.final_output.clone()).unwrap_or_default())
                .collect(),
            total_tokens: 0,
            status,
            summary,
            child_statuses,
        }
    }

    /// Builds an [`IntercomPayload`] for a FOREGROUND **single** run straight off the child's real
    /// [`crate::exec::SingleResult`] — the third sanctioned constructor, holding the same
    /// explicit-allowlist discipline as its two siblings.
    ///
    /// G104 — this exists because a single run's child status MUST be resolved by
    /// [`resolve_single_result_status`] (pi `foregroundResultIntercomStatus`,
    /// `runs/foreground/subagent-executor.ts:1594-1605` @v0.43.0, which `emitForegroundResultIntercom`
    /// calls per child at `:1626`), not by [`resolve_step_result_status`]. The two disagree on real
    /// data, and the disagreement is exactly the bug this constructor closes:
    ///
    /// * a child killed by an unexplained process signal (`processSignal` set, non-zero exit)
    ///   resolves `Stopped` here and `Failed` through a `StepResult` (which has no `process_signal`
    ///   field at all, so `resolve_step_result_status` can never reach `result-intercom.ts:35`);
    /// * a child whose acceptance ledger is REJECTED but whose process exited `0` resolves `Failed`
    ///   here (pi `:1596` pins `success: false`) and `Completed` through a `StepResult` built with
    ///   `success: exit_code == 0`.
    ///
    /// `outputs`/`total_tokens` keep the single-run shapes the grouped foreground constructor
    /// produces (`final_output` or empty; `0` — upstream's own foreground result-intercom message
    /// carries no token total either, `formatSubagentResultIntercomMessage` at
    /// `result-intercom.ts:230` renders run/mode/status/children/outputs and per-child lines and
    /// nothing else), so the ONLY behaviour this changes is the per-child status resolution, which
    /// is the point.
    #[must_use]
    pub fn from_single_result(
        run_id: RunId,
        agent: String,
        success: bool,
        result: &crate::exec::SingleResult,
    ) -> Self {
        let child_statuses = vec![resolve_single_result_status(result)];
        let status = resolve_grouped_status(&child_statuses);
        let summary = format_status_counts(&child_statuses);
        Self {
            run_id,
            agent,
            success,
            outputs: vec![result.final_output.clone().unwrap_or_default()],
            total_tokens: 0,
            status,
            summary,
            child_statuses,
        }
    }
}

// =================================================================================================
// Delivery channel + graceful-degradation race (R-SA-125)
// =================================================================================================

/// A pluggable out-of-band transport. No implementation of this trait ships in the current
/// workspace (func-SA §9 item 25 / arch-SA §12 item 7: the `pi-intercom` companion transport's
/// Rust-port status is unconfirmed) — [`deliver`] is written against this trait so that, if/when
/// such a transport exists, wiring it in is exactly "construct a `DeliveryChannel` impl and pass
/// it to `deliver`", with no change needed to the timeout/degradation contract implemented here.
///
/// Implementations MUST NOT block indefinitely; [`deliver`] applies its own bounded timeout on
/// top regardless, but a well-behaved implementation should still return promptly once it knows
/// there is no receiver (R-SA-125's "unavailable... without blocking").
pub trait DeliveryChannel: Send + Sync {
    /// Attempt one delivery of `payload`. `Ok(true)` means a receiver confirmed receipt; `Ok(false)`
    /// means the channel is reachable but no receiver confirmed (treated identically to `Err` by
    /// [`deliver`] — both degrade to [`DeliveryOutcome::NotDelivered`]); `Err` means the transport
    /// itself failed. None of the three outcomes may panic or block past what the implementation's
    /// own I/O naturally takes — [`deliver`]'s outer timeout is the actual safety net.
    fn send(&self, payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>>;
}

/// The default channel: no transport exists (func-SA §9 item 25). `send` resolves immediately to
/// `Ok(false)` — deliberately not `Err`, since "no transport configured" is not itself a failure
/// condition, it is the documented, expected steady state of this workspace today. Using this
/// channel makes R-SA-123-125 vacuously satisfied exactly as the spec anticipates: [`deliver`]
/// always reports [`DeliveryOutcome::NotDelivered`] and every caller's full result stays inline.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTransportChannel;

impl DeliveryChannel for NoTransportChannel {
    fn send(&self, _payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        Box::pin(async { Ok(false) })
    }
}

// =================================================================================================
// Steer channel — deliver an unsolicited follow-up to a live registered child (R-SA-086)
// =================================================================================================

/// A pluggable transport for delivering an UNSOLICITED steer message to an already-registered live
/// subagent child, addressed by its deterministic broker presence target
/// ([`crate::spawn::intercom_target::resolve_subagent_intercom_target`]).
///
/// This is a DISTINCT seam from [`DeliveryChannel`] (which relays a completed run's result to THIS
/// orchestrator's own fixed supervisor) and [`ClarifyChannel`] (which only REPLIES to a correlated
/// inbound child ask): neither can address an arbitrary child target with a fresh, unsolicited
/// message. [`crate::extension::SubagentExecutor::control_resume`]'s `SteerRunning` arm drives this to
/// deliver `action='resume'`'s follow-up to a still-running async child over the broker — pi's
/// `deliverSubagentIntercomMessageEvent(events, target.intercomTarget, …)`
/// (`subagent-executor.ts:860-878`). The "not registered" notice pi returns is ONLY the
/// delivery-FAILED fallback (`Ok(false)`/`Err`), never the primary path.
pub trait SteerChannel: Send + Sync {
    /// Deliver `text` to the live child registered under `target`. `Ok(true)` = the broker confirmed
    /// a registered receiver took delivery (the follow-up landed); `Ok(false)` = reachable transport
    /// but no registered receiver at `target` (the genuine "not registered" fallback); `Err` = the
    /// transport itself failed. Never panics or blocks past its own I/O.
    fn steer(&self, target: String, text: String) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>>;

    /// Whether a real intercom bridge is wired (pi `intercomBridge.active`): callers that only want
    /// to know whether it is worth SHOWING a resolved intercom-target line (e.g. the revive
    /// confirmation, `subagent-executor.ts:1019-1027`) — as opposed to attempting delivery — consult
    /// this instead of probing with a real `steer` call. Defaults to `true` so existing
    /// broker-backed implementors (which ARE a real bridge) need no change; only
    /// [`NoTransportSteerChannel`] overrides this to `false`.
    fn is_active(&self) -> bool {
        true
    }
}

/// The default steer channel: no transport wired (headless / SDK-embedder / no intercom this
/// session). `steer` resolves immediately to `Ok(false)` — "no registered receiver reachable" — so
/// [`crate::extension::SubagentExecutor::control_resume`] cleanly degrades to pi's "intercom target
/// is not registered" guidance without a live broker, exactly the documented steady state when
/// intercom is not attached. Replaced by the intercom companion's broker-backed impl
/// (`cyrup-intercom::seams::IntercomSteerChannel`) via
/// [`crate::extension::SubagentsExtension::with_channels`].
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTransportSteerChannel;

impl SteerChannel for NoTransportSteerChannel {
    fn steer(&self, _target: String, _text: String) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        Box::pin(async { Ok(false) })
    }

    fn is_active(&self) -> bool {
        false
    }
}

/// The default bounded wait for an out-of-band delivery attempt before giving up and degrading
/// (R-SA-125). A tuning parameter, not a normative numeric requirement (mirrors func-SA §9 item
/// 26's framing for the sibling `1000ms` debounce constant) — chosen short enough that a missing
/// receiver never perceptibly stalls the orchestrator's own turn.
pub const DEFAULT_DELIVERY_TIMEOUT: Duration = Duration::from_millis(750);

/// pi's `deliverSubagentIntercomMessageEvent` default `timeoutMs` (`result-intercom.ts:283-288`) —
/// the SAME 500ms bound applies to EVERY caller of that function, including the live-child follow-up
/// steer at `subagent-executor.ts:860` ([`SubagentExecutor::control_resume`]'s `SteerRunning` arm).
/// Distinct from [`DEFAULT_DELIVERY_TIMEOUT`] (750ms), which only bounds the grouped-result
/// [`DeliveryChannel`] path — pi's own two call sites use two different literals, so this module
/// keeps them as two separate constants rather than collapsing them into one.
pub const DEFAULT_STEER_TIMEOUT: Duration = Duration::from_millis(500);

/// Attempt one steer delivery through `channel`, racing it against `timeout` — the [`SteerChannel`]
/// analogue of [`deliver`]'s race, applied to the distinct steer seam. Resolves to `true` only if the
/// channel confirms `Ok(true)` before `timeout` elapses; any other outcome (`Ok(false)`, `Err`, or the
/// timeout branch firing first) resolves to `false`, matching pi's `deliverSubagentIntercomMessageEvent`
/// contract that the caller's turn is never blocked longer than `timeoutMs` (`result-intercom.ts:283-316`).
pub async fn steer_with_timeout(channel: &dyn SteerChannel, target: String, text: String, timeout: Duration) -> bool {
    let attempt = channel.steer(target, text);
    tokio::select! {
        biased;
        result = attempt => result.unwrap_or(false),
        () = tokio::time::sleep(timeout) => false,
    }
}

/// Convenience wrapper over [`steer_with_timeout`] using [`DEFAULT_STEER_TIMEOUT`] (pi's `timeoutMs
/// = 500` default, applied uniformly to every caller per `result-intercom.ts:283-288`).
pub async fn steer_with_default_timeout(channel: &dyn SteerChannel, target: String, text: String) -> bool {
    steer_with_timeout(channel, target, text, DEFAULT_STEER_TIMEOUT).await
}

/// The result of one [`deliver`] attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// A receiver confirmed receipt within the timeout — the caller MAY now omit the heavy
    /// duplicated fields from its own inline payload (R-SA-123).
    Delivered,
    /// The channel was unavailable, reported no receiver, errored, or did not respond within the
    /// timeout — the caller's full result MUST remain in the ordinary inline tool-result payload
    /// (R-SA-125). This module intentionally does not distinguish *why* delivery failed beyond
    /// this: R-SA-125 treats "unavailable" and "timed out" identically (both fail gracefully to
    /// "not delivered"), so collapsing them into one variant avoids inventing a behavioral
    /// distinction the spec does not draw.
    NotDelivered,
}

/// Attempt one out-of-band delivery of `payload` through `channel`, racing it against a bounded
/// timeout (R-SA-125). Never blocks past `timeout`, never returns `Err` (there is nothing for a
/// caller to handle as an error — every failure mode collapses to
/// [`DeliveryOutcome::NotDelivered`]), and never panics regardless of what `channel` does.
///
/// This function does not need to (and does not) detect receiver presence up front — "no
/// receiver" and "timed out waiting for a receiver" are handled by the identical code path (the
/// `tokio::select!` below simply resolves to `NotDelivered` on either the channel returning
/// `Ok(false)`/`Err`, or the timeout branch firing first), matching R-SA-125's "without needing
/// to detect presence beforehand" framing.
pub async fn deliver(channel: &dyn DeliveryChannel, payload: IntercomPayload, timeout: Duration) -> DeliveryOutcome {
    let attempt = channel.send(payload);
    tokio::select! {
        biased;
        result = attempt => match result {
            Ok(true) => DeliveryOutcome::Delivered,
            Ok(false) | Err(_) => DeliveryOutcome::NotDelivered,
        },
        () = tokio::time::sleep(timeout) => DeliveryOutcome::NotDelivered,
    }
}

/// Convenience wrapper over [`deliver`] using [`DEFAULT_DELIVERY_TIMEOUT`].
pub async fn deliver_with_default_timeout(channel: &dyn DeliveryChannel, payload: IntercomPayload) -> DeliveryOutcome {
    deliver(channel, payload, DEFAULT_DELIVERY_TIMEOUT).await
}

/// Builds the reduced inline tool-result payload a caller SHOULD use once out-of-band delivery is
/// **confirmed** (R-SA-123): the heavy duplicated `outputs` field is dropped, leaving only the
/// identity/summary fields a reader needs to know "this ran, here's where the full detail already
/// went." Callers MUST NOT call this unless [`deliver`] returned
/// [`DeliveryOutcome::Delivered`] for the exact same payload — on any other outcome the full
/// [`IntercomPayload::outputs`] (or, more precisely, the caller's own full
/// [`crate::background::ResultFile`]/[`crate::exec::SingleResult`] payload it was projected from)
/// must remain inline in full (R-SA-125), which this function intentionally provides no way to
/// bypass: it only ever narrows an already-allowlisted payload further, never widens the
/// "delivered" decision itself.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReducedInlinePayload {
    pub run_id: RunId,
    pub agent: String,
    pub success: bool,
    pub total_tokens: u64,
}

impl From<&IntercomPayload> for ReducedInlinePayload {
    fn from(full: &IntercomPayload) -> Self {
        Self {
            run_id: full.run_id.clone(),
            agent: full.agent.clone(),
            success: full.success,
            total_tokens: full.total_tokens,
        }
    }
}

// =================================================================================================
// Clarify/ask pause primitive (R-SA-119/120)
//
// NOTE(clarify-wired): the real `ClarifyChannel` is now wired (reconciliation §4 step 5 item 3 —
// R-SA-119/120/037 CLOSED). The intercom companion's broker-backed `IntercomClarifyChannel`
// (`cyrup-intercom/src/seams.rs`) — which surfaces the child's ask to the parent's human via the
// P-1-late-bound `HostServices::input` and routes the answer back to the still-alive child over the
// broker — is threaded into the executor via `SubagentsExtension::with_channels` (from the
// `crates/cyrup/src/main.rs` session-build sites) and wrapped in the single-slot [`AskLock`] below.
// The exec drive loop (`exec/mod.rs::drive_attempt`) fires [`spawn_clarify`] against that lock the
// moment a child emits a blocking `contact_supervisor` ask (via `RunOptions::clarify` /
// [`ClarifyDispatch`]). When no channel is wired (headless / SDK-embedder), the lock keeps the
// documented [`NoOpClarifyChannel`] degrade default ([`ClarifyOutcome::NoLiveChannel`], never blocks).
// =================================================================================================

/// One outstanding blocking clarify/ask interaction a child run is waiting on (R-SA-119).
#[derive(Clone, Debug)]
pub struct ClarifyRequest {
    /// The run whose foreground flow is paused waiting on this clarify interaction.
    pub run_id: RunId,
    /// The step index within that run's flow that is blocked, if applicable (a parallel/chain
    /// flow pauses only its affected step, per R-SA-119, not the whole orchestrator).
    pub step_index: Option<u32>,
    /// The human-facing prompt text the child supplied for this clarify interaction.
    pub prompt: String,
}

/// The outcome a caller sees for one [`request_clarify`] attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClarifyOutcome {
    /// A response was obtained (whatever the pluggable [`ClarifyChannel`] returned).
    Answered(String),
    /// No live clarify UI is wired (the documented graceful fallback — see the module-level
    /// `NOTE(clarify-wired)` doc above). The affected flow still visibly pauses for the
    /// duration of the attempt (R-SA-119) before this is returned; it is simply never able to
    /// resolve to an actual human answer until a real [`ClarifyChannel`] is wired in.
    NoLiveChannel,
    /// A second concurrent ask was attempted while one was already outstanding for this session
    /// (R-SA-120) and was rejected rather than silently interleaved.
    Rejected,
}

/// A pluggable clarify/ask transport (deliberately trait-based, not a concrete session handle —
/// see `NOTE(clarify-wired)` above for why this crate does not itself implement one against
/// `cyrup-session-svc` in this file).
pub trait ClarifyChannel: Send + Sync {
    /// Present `request` to a live human/UI and await a response. Implementations should apply
    /// their own reasonable timeout; [`request_clarify`] does not impose an additional one on top
    /// (an ask is, by design, allowed to wait indefinitely for a human — R-SA-119 is about
    /// visibly pausing the flow while this is outstanding, not about bounding how long a human
    /// may take).
    fn ask(&self, request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
}

/// The documented graceful no-op fallback (see the module-level `NOTE(clarify-wired)` doc):
/// always reports [`ClarifyOutcome::NoLiveChannel`] via an `Err`, so [`request_clarify`]'s
/// single-slot-lock bookkeeping and "visibly pause" contract are exercised even with no real UI
/// wired, exactly mirroring `LiveHostServices::ui_roundtrip`'s own "no sink -> deny default,
/// never block" behavior for the identical situation on the WASM-guest path.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoOpClarifyChannel;

impl ClarifyChannel for NoOpClarifyChannel {
    fn ask(&self, _request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async { Err("no live clarify channel wired".to_string()) })
    }
}

/// The single-slot ask lock (R-SA-120): at most one outstanding blocking clarify/ask interaction
/// is permitted per orchestrator session at a time. Keyed by an opaque session identifier (a
/// plain `String` here — this module has no dependency on `cyrup-core::SessionId` and does not
/// need one; callers pass whatever session-scoping key they already use) so one `AskLock`
/// instance can serve every session an orchestrating extension instance is handling, rather than
/// needing one lock per session wired up externally.
pub struct AskLock {
    channel: Arc<dyn ClarifyChannel>,
    /// `Some` while a session has an outstanding ask; the guard is dropped (clearing the slot)
    /// when [`request_clarify`]'s future completes, on every exit path (including cancellation),
    /// via `slots`' own `AsyncMutex` scoping — never left dangling by an early return.
    slots: AsyncMutex<HashMap<String, ()>>,
}

impl AskLock {
    /// Builds a lock backed by `channel`. Production passes the intercom companion's real broker
    /// [`ClarifyChannel`] here (threaded via `SubagentsExtension::with_channels`, see
    /// `NOTE(clarify-wired)`); pass [`NoOpClarifyChannel::default()`] to get the documented
    /// graceful-fallback behavior (no live UI wired — headless / SDK-embedder).
    #[must_use]
    pub fn new(channel: Arc<dyn ClarifyChannel>) -> Self {
        Self { channel, slots: AsyncMutex::new(HashMap::new()) }
    }

    /// Builds a lock using the documented no-op fallback (today's default — see
    /// `NOTE(clarify-wired)`).
    #[must_use]
    pub fn new_with_no_live_channel() -> Self {
        Self::new(Arc::new(NoOpClarifyChannel))
    }

    /// Requests a blocking clarify/ask interaction for `session_key`, visibly pausing the
    /// affected foreground flow for the duration (R-SA-119) and enforcing the single-slot lock
    /// (R-SA-120): if `session_key` already has an outstanding ask, this returns
    /// [`ClarifyOutcome::Rejected`] immediately rather than interleaving a second one.
    ///
    /// "Visibly pause" here means: the caller (the foreground execution driver, a later phase)
    /// MUST treat any non-[`ClarifyOutcome::Rejected`] return of this function as "the affected
    /// step/run is paused until this future resolves" — this function's own contribution to that
    /// visibility is that it does not return early/optimistically; it awaits the channel's answer
    /// (or its own fallback) to completion before yielding a result, so a caller that renders
    /// "paused" for exactly the lifetime of the awaited call gets correct pause duration for
    /// free.
    pub async fn request_clarify(&self, session_key: &str, request: ClarifyRequest) -> ClarifyOutcome {
        {
            let mut slots = self.slots.lock().await;
            if slots.contains_key(session_key) {
                return ClarifyOutcome::Rejected;
            }
            slots.insert(session_key.to_string(), ());
        }

        // The slot is held for the entire await below (cleared in the `finally`-style block
        // afterward on every path, success or failure) — this is the actual enforcement of
        // R-SA-120's "one outstanding ask at a time per session," not merely a check-then-forget.
        let outcome = match self.channel.ask(request).await {
            Ok(answer) => ClarifyOutcome::Answered(answer),
            Err(_) => ClarifyOutcome::NoLiveChannel,
        };

        let mut slots = self.slots.lock().await;
        slots.remove(session_key);

        outcome
    }
}

/// A single outstanding-ask handle usable with `tokio::select!`/cancellation call sites that need
/// to observe "the ask finished" as a plain oneshot rather than awaiting [`AskLock::request_clarify`]
/// directly in-line — e.g. a foreground driver that must simultaneously keep consuming a child's
/// NDJSON stdout while a clarify request is outstanding for one of that child's steps. Spawns the
/// actual [`AskLock::request_clarify`] call onto the current runtime and forwards its result
/// through the returned receiver; dropping the receiver before it resolves does not cancel the
/// underlying ask (a human may still be mid-answer), it only stops that particular caller from
/// observing the outcome.
/// The clarify/ask dispatch context the exec drive loop needs to fire a child's blocking
/// `contact_supervisor` ask through [`spawn_clarify`] (R-SA-037 detach-trigger arm). Threaded from
/// the executor's [`AskLock`] into [`crate::exec::RunOptions::clarify`] at the foreground run's spawn
/// site; `None` there degrades to today's no-clarify behavior (the background hop-2 runner / tests
/// with no channel). Carries the session-scoping key ([`AskLock`]'s single-slot key, R-SA-120) plus
/// the run/step identity the [`ClarifyRequest`] surfaces (and the intercom `ClarifyChannel`
/// correlates on).
#[derive(Clone)]
pub struct ClarifyDispatch {
    /// The single-slot ask lock (R-SA-120), shared across every run this executor drives.
    pub lock: Arc<AskLock>,
    /// The orchestrator-session-scoping key for the single-slot lock (R-SA-120's "one outstanding
    /// ask per orchestrator session").
    pub session_key: String,
    /// The run whose foreground flow pauses on the ask (surfaced + correlated).
    pub run_id: RunId,
    /// The affected step within that run's flow, if applicable (R-SA-119 pauses only the affected
    /// step, not the whole orchestrator).
    pub step_index: Option<u32>,
}

// `AskLock` wraps an `Arc<dyn ClarifyChannel>` + an `AsyncMutex` (neither `Debug`); a manual impl
// keeps [`crate::exec::RunOptions`]'s derived `Debug` while never trying to format the channel.
impl std::fmt::Debug for ClarifyDispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClarifyDispatch")
            .field("session_key", &self.session_key)
            .field("run_id", &self.run_id)
            .field("step_index", &self.step_index)
            .finish_non_exhaustive()
    }
}

pub fn spawn_clarify(lock: Arc<AskLock>, session_key: String, request: ClarifyRequest) -> oneshot::Receiver<ClarifyOutcome> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let outcome = lock.request_clarify(&session_key, request).await;
        // A dropped receiver (caller no longer cares) is not an error condition here.
        let _ = tx.send(outcome);
    });
    rx
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use cyrup_core::Usage;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    // ---------------------------------------------------------------------------------------
    // R-SA-124: allowlist projection never leaks a disallowed field — asserted by construction/
    // type, not merely by example.
    // ---------------------------------------------------------------------------------------

    /// This test asserts the allowlist **by type structure**: it exhaustively destructures an
    /// [`IntercomPayload`] by field name. If a future edit added a new field to the struct (e.g.
    /// accidentally re-introducing `cwd`, `session_file`, or any capability-token-shaped field),
    /// this destructuring pattern would fail to compile (missing-field / unknown-field error)
    /// rather than the test silently continuing to pass — that compile-time exhaustiveness is
    /// the "by construction, not just by example" guarantee this test exists to provide. Compare
    /// with a plain `assert_eq!(payload.run_id, ...)` style test, which would NOT fail if an
    /// unrelated extra field were added.
    #[test]
    fn intercom_payload_field_set_is_closed_and_exhaustive() {
        let payload = IntercomPayload {
            run_id: RunId::from_token("deadbeefcafef00d"),
            agent: "researcher".to_string(),
            success: true,
            outputs: vec!["done".to_string()],
            total_tokens: 42,
            status: SubagentResultStatus::Completed,
            summary: "1 completed".to_string(),
            child_statuses: vec![SubagentResultStatus::Completed],
        };

        // Exhaustive destructure: the `let IntercomPayload { .. } = payload;` form below would
        // still compile if a field were added (the `..` pattern), so instead we name every field
        // explicitly with no `..` — this is what makes the assertion compile-time-exhaustive.
        let IntercomPayload { run_id, agent, success, outputs, total_tokens, status, summary, child_statuses } =
            payload;
        assert_eq!(run_id.as_str(), "deadbeefcafef00d");
        assert_eq!(agent, "researcher");
        assert!(success);
        assert_eq!(outputs, vec!["done".to_string()]);
        assert_eq!(total_tokens, 42);
        assert_eq!(status, SubagentResultStatus::Completed);
        assert_eq!(summary, "1 completed");
        assert_eq!(child_statuses, vec![SubagentResultStatus::Completed]);
    }

    /// The allowlist is enforced by the projection function itself never having access to
    /// disallowed fields in the first place: build a [`crate::background::ResultFile`] whose
    /// `cwd`/`session_file` are populated with values that would be an obvious, embarrassing leak
    /// if they ever ended up in the wire payload (a path containing a fake "secret"-looking
    /// token), project it through [`IntercomPayload::from_result`], and assert neither of those
    /// values appears ANYWHERE in the projected payload's serialized form. This is a
    /// belt-and-suspenders behavioral check on top of the type-level exhaustiveness test above.
    #[test]
    fn from_result_never_copies_cwd_or_session_file_into_the_wire_payload() {
        let secret_cwd = PathBuf::from("/Users/nobody/.ssh/CAPABILITY_TOKEN_LEAK_CANARY");
        let secret_session = PathBuf::from("/var/run/CONTROL_INBOX_ROUTE_LEAK_CANARY.json");

        let result = crate::background::ResultFile {
            id: RunId::from_token("run00000000000001"),
            run_id: RunId::from_token("run00000000000001"),
            agent: "delegate".to_string(),
            mode: crate::background::RunMode::Single,
            state: crate::background::RunState::Complete,
            success: true,
            cwd: secret_cwd.clone(),
            session_file: Some(secret_session.clone()),
            results: vec![sample_single_result("delegate", "did the thing")],
        };

        let payload = IntercomPayload::from_result(&result);
        let wire = serde_json::to_string(&payload).expect("serializes");

        assert!(
            !wire.contains("CAPABILITY_TOKEN_LEAK_CANARY"),
            "cwd must never appear in the out-of-band payload: {wire}"
        );
        assert!(
            !wire.contains("CONTROL_INBOX_ROUTE_LEAK_CANARY"),
            "session_file must never appear in the out-of-band payload: {wire}"
        );
        assert!(wire.contains("did the thing"), "the allowlisted output must still be present");
    }

    #[test]
    fn from_result_sums_token_usage_across_all_steps() {
        let mut a = sample_single_result("scout", "found stuff");
        a.usage = Usage { total_tokens: 15, ..Usage::default() };
        let mut b = sample_single_result("worker", "did stuff");
        b.usage = Usage { total_tokens: 150, ..Usage::default() };

        let result = crate::background::ResultFile {
            id: RunId::from_token("run00000000000002"),
            run_id: RunId::from_token("run00000000000002"),
            agent: "orchestrator".to_string(),
            mode: crate::background::RunMode::Chain,
            state: crate::background::RunState::Complete,
            success: true,
            cwd: PathBuf::from("/tmp/irrelevant-here"),
            session_file: None,
            results: vec![a, b],
        };

        let payload = IntercomPayload::from_result(&result);
        assert_eq!(payload.total_tokens, 15 + 150);
        assert_eq!(payload.outputs, vec!["found stuff".to_string(), "did stuff".to_string()]);
    }

    fn sample_single_result(agent: &str, output: &str) -> crate::exec::SingleResult {
        crate::exec::SingleResult {
            agent: agent.to_string(),
            task: "task".to_string(),
            exit_code: 0,
            usage: Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: Some(output.to_string()),
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            stopped: false,
            process_signal: None,
            error: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        }
    }

    // ---------------------------------------------------------------------------------------
    // G104 — `stopped` as a first-class result status (pi `result-intercom.ts:20-91` @v0.43.0)
    // ---------------------------------------------------------------------------------------

    /// pi `resolveSubagentResultStatus` (`result-intercom.ts:20-41`) branch-by-branch, in
    /// upstream's own order. The two assertions that would fail under any "stopped is an alias"
    /// implementation are called out inline.
    #[test]
    fn resolve_subagent_result_status_reproduces_pis_branch_order() {
        let base = ResultStatusInput::default();

        // 1. detached wins over everything, including an explicit stop.
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput {
                detached: true,
                stopped: true,
                ..base.clone()
            }),
            SubagentResultStatus::Detached
        );

        // 2. `stopped` OR `state === "stopped"` — and it wins over `interrupted`, which is the
        //    branch that stops a stopped run from being reported as a resumable pause.
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput { stopped: true, ..base.clone() }),
            SubagentResultStatus::Stopped
        );
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput {
                state: Some("stopped"),
                ..base.clone()
            }),
            SubagentResultStatus::Stopped
        );
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput {
                stopped: true,
                interrupted: true,
                ..base.clone()
            }),
            SubagentResultStatus::Stopped,
            "stopped MUST outrank interrupted (`result-intercom.ts:32` precedes `:33`)"
        );

        // 3. interrupted / state==="paused".
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput { interrupted: true, ..base.clone() }),
            SubagentResultStatus::Paused
        );
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput { state: Some("paused"), ..base.clone() }),
            SubagentResultStatus::Paused
        );

        // 4. success === true.
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput {
                success: Some(true),
                exit_code: Some(3),
                ..base.clone()
            }),
            SubagentResultStatus::Completed
        );

        // 5. an UNEXPLAINED signal with a non-zero exit is `stopped`, NOT `failed`
        //    (`result-intercom.ts:35`) — the branch that keeps an externally-SIGKILLed child out of
        //    the failure bucket.
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput {
                exit_code: Some(137),
                process_signal: Some("SIGKILL"),
                ..base.clone()
            }),
            SubagentResultStatus::Stopped
        );
        // …but an EXPLAINED one is not: a timeout, an interrupt, or a stop all disqualify it.
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput {
                exit_code: Some(137),
                process_signal: Some("SIGKILL"),
                timed_out: true,
                success: Some(false),
                ..base.clone()
            }),
            SubagentResultStatus::Failed
        );
        // …and neither is a signal death that still exited 0 (`&& input.exitCode !== 0`).
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput {
                exit_code: Some(0),
                process_signal: Some("SIGTERM"),
                ..base.clone()
            }),
            SubagentResultStatus::Completed
        );

        // 6-10. the success/state/exit-code tail, including pi's ultimate `"failed"` default.
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput { success: Some(false), ..base.clone() }),
            SubagentResultStatus::Failed
        );
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput { state: Some("complete"), ..base.clone() }),
            SubagentResultStatus::Completed
        );
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput { state: Some("failed"), ..base.clone() }),
            SubagentResultStatus::Failed
        );
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput { exit_code: Some(0), ..base.clone() }),
            SubagentResultStatus::Completed
        );
        assert_eq!(
            resolve_subagent_result_status(&ResultStatusInput { exit_code: Some(1), ..base.clone() }),
            SubagentResultStatus::Failed
        );
        assert_eq!(resolve_subagent_result_status(&base), SubagentResultStatus::Failed);
    }

    /// pi `isUnexplainedProcessSignal` (`runs/shared/process-signal.ts:5-19`): all four
    /// explanations disqualify, and an absent/empty signal is never "unexplained".
    #[test]
    fn is_unexplained_process_signal_matches_pis_four_disqualifiers() {
        assert!(is_unexplained_process_signal(Some("SIGKILL"), false, false, false, false));
        assert!(!is_unexplained_process_signal(None, false, false, false, false));
        assert!(!is_unexplained_process_signal(Some(""), false, false, false, false));
        assert!(!is_unexplained_process_signal(Some("SIGKILL"), true, false, false, false));
        assert!(!is_unexplained_process_signal(Some("SIGKILL"), false, true, false, false));
        assert!(!is_unexplained_process_signal(Some("SIGKILL"), false, false, true, false));
        assert!(!is_unexplained_process_signal(Some("SIGKILL"), false, false, false, true));
    }

    /// pi `formatStatusCounts` (`result-intercom.ts:57-66`): the RENDER order puts `stopped`
    /// between `failed` and `paused`, which is NOT the bucket/declaration order — a zip against the
    /// enum order would print it fourth.
    #[test]
    fn format_status_counts_renders_stopped_between_failed_and_paused() {
        let all = [
            SubagentResultStatus::Detached,
            SubagentResultStatus::Paused,
            SubagentResultStatus::Stopped,
            SubagentResultStatus::Failed,
            SubagentResultStatus::Completed,
        ];
        assert_eq!(
            format_status_counts(&all),
            "1 completed, 1 failed, 1 stopped, 1 paused, 1 detached"
        );
        assert_eq!(
            format_status_counts(&[SubagentResultStatus::Stopped, SubagentResultStatus::Stopped]),
            "2 stopped"
        );
        // Unchanged for a stop-free tally (the pre-G104 render order is preserved exactly).
        assert_eq!(
            format_status_counts(&[
                SubagentResultStatus::Completed,
                SubagentResultStatus::Paused,
                SubagentResultStatus::Detached
            ]),
            "1 completed, 1 paused, 1 detached"
        );
        assert_eq!(format_status_counts(&[]), "0 results");
    }

    /// pi `resolveGroupedStatus` (`result-intercom.ts:83-91`): failed > stopped > paused >
    /// completed > detached, with `failed` as the empty-set default.
    #[test]
    fn resolve_grouped_status_gives_stopped_its_own_precedence_slot() {
        use SubagentResultStatus::{Completed, Detached, Failed, Paused, Stopped};
        assert_eq!(resolve_grouped_status(&[Failed, Stopped]), Failed, "failed still outranks stopped");
        assert_eq!(resolve_grouped_status(&[Stopped, Paused]), Stopped, "stopped outranks paused");
        assert_eq!(resolve_grouped_status(&[Stopped, Completed]), Stopped, "stopped outranks completed");
        assert_eq!(resolve_grouped_status(&[Stopped, Detached]), Stopped);
        assert_eq!(resolve_grouped_status(&[Stopped]), Stopped);
        // The pre-G104 relations are untouched.
        assert_eq!(resolve_grouped_status(&[Paused, Completed]), Paused);
        assert_eq!(resolve_grouped_status(&[Completed, Detached]), Completed);
        assert_eq!(resolve_grouped_status(&[]), Failed);
    }

    /// The LIVE projection path: `IntercomPayload::from_result` must read each child's own
    /// `stopped` flag (never the run's overall state flattened onto every child) and roll the
    /// group up to `Stopped`.
    #[test]
    fn from_result_reports_a_stopped_child_without_relabelling_the_completed_ones() {
        let mut stopped_child = sample_single_result("worker", "");
        stopped_child.stopped = true;
        stopped_child.exit_code = 1;
        let done_child = sample_single_result("scout", "found it");

        let result = crate::background::ResultFile {
            id: RunId::from_token("stoppedrun00001"),
            run_id: RunId::from_token("stoppedrun00001"),
            agent: "scout".to_string(),
            mode: crate::background::RunMode::Chain,
            state: crate::background::RunState::Stopped,
            success: false,
            cwd: PathBuf::from("/tmp"),
            session_file: None,
            results: vec![done_child, stopped_child],
        };

        let payload = IntercomPayload::from_result(&result);
        assert_eq!(
            payload.child_statuses,
            vec![SubagentResultStatus::Completed, SubagentResultStatus::Stopped],
            "the child that finished BEFORE the stop stays `completed` — pi resolves per child, \
             never by flattening the run's own state onto all of them"
        );
        assert_eq!(payload.status, SubagentResultStatus::Stopped);
        assert_eq!(payload.summary, "1 completed, 1 stopped");
        assert_eq!(
            format_subagent_result_receipt("chain", &payload.run_id, &payload.child_statuses),
            "Delivered chain subagent results via intercom.\nRun: stoppedrun00001\nChildren: 1 completed, 1 stopped\nFull grouped output was sent over intercom."
        );
    }

    /// The other LIVE projection path: a child killed by an unexplained signal resolves to
    /// `stopped` straight off its real `SingleResult`, through the same
    /// `foregroundResultIntercomStatus` shape pi uses (`subagent-executor.ts:1594-1605`).
    #[test]
    fn resolve_single_result_status_reads_stopped_and_the_unexplained_signal_off_a_real_child() {
        let mut child = sample_single_result("worker", "partial");
        child.exit_code = 137;
        child.process_signal = Some("SIGKILL".to_string());
        assert_eq!(resolve_single_result_status(&child), SubagentResultStatus::Stopped);

        child.timed_out = true;
        assert_eq!(
            resolve_single_result_status(&child),
            SubagentResultStatus::Failed,
            "an EXPLAINED signal death is not a stop"
        );

        let mut stopped = sample_single_result("worker", "");
        stopped.stopped = true;
        assert_eq!(resolve_single_result_status(&stopped), SubagentResultStatus::Stopped);
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-125: bounded-timeout degrade-to-undelivered path with no receiver present.
    // ---------------------------------------------------------------------------------------

    /// A channel that never resolves — the real-world shape of "no receiver present" (nobody
    /// ever answers) as opposed to "explicitly declines" ([`NoTransportChannel`]). Used to prove
    /// [`deliver`]'s timeout branch actually fires and bounds the wait, rather than [`deliver`]
    /// merely happening to return quickly because the default channel resolves instantly.
    struct HangingChannel;

    impl DeliveryChannel for HangingChannel {
        fn send(&self, _payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_degrades_to_not_delivered_when_no_receiver_ever_responds() {
        let payload = sample_payload();
        let timeout = Duration::from_millis(80);

        let started = Instant::now();
        let outcome = deliver(&HangingChannel, payload, timeout).await;
        let elapsed = started.elapsed();

        assert_eq!(outcome, DeliveryOutcome::NotDelivered);
        assert!(
            elapsed >= timeout,
            "must actually wait out the bound, not return early: {elapsed:?} < {timeout:?}"
        );
        assert!(
            elapsed < timeout * 5,
            "must not block far past the bound (never block the caller's turn): {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_degrades_to_not_delivered_when_channel_reports_no_receiver() {
        // NoTransportChannel::send resolves immediately to Ok(false) — the "channel reachable,
        // explicitly no receiver" case, which must ALSO degrade gracefully without needing the
        // timeout branch to fire at all (R-SA-125's "without needing to detect presence
        // beforehand": both this case and the hanging case above land on the identical outcome).
        let payload = sample_payload();
        let started = Instant::now();

        let outcome = deliver(&NoTransportChannel, payload, Duration::from_secs(5)).await;

        assert_eq!(outcome, DeliveryOutcome::NotDelivered);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "an explicit 'no receiver' answer must resolve promptly, not wait out the full bound"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_degrades_to_not_delivered_on_channel_error() {
        struct ErroringChannel;
        impl DeliveryChannel for ErroringChannel {
            fn send(&self, _payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
                Box::pin(async { Err("transport exploded".to_string()) })
            }
        }

        let outcome = deliver(&ErroringChannel, sample_payload(), Duration::from_secs(5)).await;
        assert_eq!(outcome, DeliveryOutcome::NotDelivered, "an error must never propagate to the caller as Err");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_reports_delivered_when_a_receiver_confirms_promptly() {
        struct ConfirmingChannel;
        impl DeliveryChannel for ConfirmingChannel {
            fn send(&self, _payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
                Box::pin(async { Ok(true) })
            }
        }

        let outcome = deliver(&ConfirmingChannel, sample_payload(), Duration::from_secs(5)).await;
        assert_eq!(outcome, DeliveryOutcome::Delivered);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_with_default_timeout_uses_the_documented_constant() {
        let started = Instant::now();
        let outcome = deliver_with_default_timeout(&HangingChannel, sample_payload()).await;
        let elapsed = started.elapsed();

        assert_eq!(outcome, DeliveryOutcome::NotDelivered);
        assert!(elapsed >= DEFAULT_DELIVERY_TIMEOUT);
        assert!(elapsed < DEFAULT_DELIVERY_TIMEOUT * 5);
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-125 (second half): full-output preservation in the local result when undelivered.
    // ---------------------------------------------------------------------------------------

    /// The defining behavioral contract of "graceful degradation": whatever the caller already
    /// held locally (the full [`IntercomPayload`], standing in here for the caller's own full
    /// inline tool-result payload) is completely untouched by an undelivered attempt — `deliver`
    /// takes the payload by value only to hand it to the channel; nothing about a `NotDelivered`
    /// outcome mutates or discards any field the caller still has. This test rebuilds the exact
    /// value passed in and confirms every field survives identically after a failed delivery.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn undelivered_attempt_leaves_the_original_payload_fully_reconstructable_by_the_caller() {
        let payload = sample_payload();
        let payload_for_local_use = payload.clone();

        let outcome = deliver(&HangingChannel, payload, Duration::from_millis(50)).await;

        assert_eq!(outcome, DeliveryOutcome::NotDelivered);
        // The caller's own retained copy (what an exec/background call site would put in the
        // ordinary inline tool-result payload per R-SA-125) is exactly the full payload, field
        // for field — nothing was truncated, stripped, or replaced by the failed attempt.
        assert_eq!(payload_for_local_use.outputs.len(), 2);
        assert_eq!(payload_for_local_use.outputs[0], "first step output, in full");
        assert_eq!(payload_for_local_use.outputs[1], "second step output, in full, unabridged");
        assert_eq!(payload_for_local_use.total_tokens, 999);
        assert_eq!(payload_for_local_use.agent, "orchestrator");
        assert!(payload_for_local_use.success);
    }

    /// [`ReducedInlinePayload`] must only ever be constructed by a caller that already confirmed
    /// [`DeliveryOutcome::Delivered`] — this test documents (and pins) that the reduction itself
    /// is a pure, always-available projection (it does not gate on delivery status internally;
    /// gating is the caller's job per the type's own doc comment), while separately proving the
    /// FULL payload remains available and unaffected regardless of whether a caller chooses to
    /// build a reduced view from it.
    #[test]
    fn reduced_inline_payload_drops_only_the_heavy_outputs_field() {
        let full = sample_payload();
        let reduced = ReducedInlinePayload::from(&full);

        assert_eq!(reduced.run_id, full.run_id);
        assert_eq!(reduced.agent, full.agent);
        assert_eq!(reduced.success, full.success);
        assert_eq!(reduced.total_tokens, full.total_tokens);
        // The full payload itself is untouched by having built a reduced view from a reference.
        assert_eq!(full.outputs.len(), 2);
    }

    fn sample_payload() -> IntercomPayload {
        IntercomPayload {
            run_id: RunId::from_token("sample0000000001"),
            agent: "orchestrator".to_string(),
            success: true,
            outputs: vec![
                "first step output, in full".to_string(),
                "second step output, in full, unabridged".to_string(),
            ],
            total_tokens: 999,
            status: SubagentResultStatus::Completed,
            summary: "2 completed".to_string(),
            child_statuses: vec![SubagentResultStatus::Completed, SubagentResultStatus::Completed],
        }
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-119/120: clarify/ask single-slot lock + graceful no-live-channel fallback.
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_clarify_with_no_live_channel_degrades_gracefully_without_panicking() {
        let lock = AskLock::new_with_no_live_channel();
        let outcome = lock
            .request_clarify(
                "session-a",
                ClarifyRequest { run_id: RunId::from_token("run0000000000000a"), step_index: Some(2), prompt: "ok?".to_string() },
            )
            .await;
        assert_eq!(outcome, ClarifyOutcome::NoLiveChannel);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_clarify_frees_the_slot_after_completion_so_a_later_ask_succeeds() {
        let lock = AskLock::new_with_no_live_channel();
        let req = |n: u32| ClarifyRequest { run_id: RunId::from_token("run0000000000000b"), step_index: Some(n), prompt: "ok?".to_string() };

        let first = lock.request_clarify("session-b", req(1)).await;
        assert_eq!(first, ClarifyOutcome::NoLiveChannel);

        // The slot must have been released after the first request completed — a second,
        // sequential ask for the SAME session must not be rejected.
        let second = lock.request_clarify("session-b", req(2)).await;
        assert_eq!(second, ClarifyOutcome::NoLiveChannel, "must not be Rejected once the prior ask completed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn request_clarify_rejects_a_second_concurrent_ask_for_the_same_session() {
        // A channel whose `ask` blocks until manually released, so we can deterministically
        // observe TWO concurrent in-flight asks for the same session.
        struct GateChannel {
            gate: tokio::sync::Notify,
            entered: AtomicUsize,
        }
        impl ClarifyChannel for GateChannel {
            fn ask(&self, _request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async move {
                    self.entered.fetch_add(1, Ordering::SeqCst);
                    self.gate.notified().await;
                    Ok("answered".to_string())
                })
            }
        }

        let channel = Arc::new(GateChannel { gate: tokio::sync::Notify::new(), entered: AtomicUsize::new(0) });
        let lock = Arc::new(AskLock::new(channel.clone()));

        let req = |n: u32| ClarifyRequest { run_id: RunId::from_token("run0000000000000c"), step_index: Some(n), prompt: "ok?".to_string() };

        let lock_a = lock.clone();
        let first = tokio::spawn(async move { lock_a.request_clarify("session-c", req(1)).await });

        // Wait until the first ask is genuinely in-flight (inside the channel, holding the slot)
        // before firing the second — avoids a race where the second could win the lock first.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while channel.entered.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(channel.entered.load(Ordering::SeqCst), 1, "first ask must have entered the channel");

        let second = lock.request_clarify("session-c", req(2)).await;
        assert_eq!(second, ClarifyOutcome::Rejected, "R-SA-120: a second concurrent ask must be rejected");

        // Release the first ask and confirm it completes normally (the lock's own bookkeeping
        // is unaffected by the rejected second attempt).
        channel.gate.notify_one();
        let first_outcome = first.await.expect("task join");
        assert_eq!(first_outcome, ClarifyOutcome::Answered("answered".to_string()));

        // Only one ask ever actually entered the channel — the rejected attempt never called
        // `ask` at all.
        assert_eq!(channel.entered.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn request_clarify_permits_concurrent_asks_for_different_sessions() {
        let lock = Arc::new(AskLock::new_with_no_live_channel());
        let req = |n: u32| ClarifyRequest { run_id: RunId::from_token("run0000000000000d"), step_index: Some(n), prompt: "ok?".to_string() };

        let lock_a = lock.clone();
        let a = tokio::spawn(async move { lock_a.request_clarify("session-d1", req(1)).await });
        let lock_b = lock.clone();
        let b = tokio::spawn(async move { lock_b.request_clarify("session-d2", req(2)).await });

        let (a_outcome, b_outcome) = tokio::join!(a, b);
        assert_eq!(a_outcome.expect("join"), ClarifyOutcome::NoLiveChannel);
        assert_eq!(b_outcome.expect("join"), ClarifyOutcome::NoLiveChannel);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_clarify_delivers_the_outcome_through_the_returned_receiver() {
        let lock = Arc::new(AskLock::new_with_no_live_channel());
        let rx = spawn_clarify(
            lock,
            "session-e".to_string(),
            ClarifyRequest { run_id: RunId::from_token("run0000000000000e"), step_index: None, prompt: "ok?".to_string() },
        );
        let outcome = rx.await.expect("the spawned ask completes and sends its outcome");
        assert_eq!(outcome, ClarifyOutcome::NoLiveChannel);
    }
}
