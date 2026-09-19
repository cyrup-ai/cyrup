//! `ResultsDir` filesystem-watch completion notification (func-SA §5.4 R-SA-098..103; arch-SA
//! §6.5's `background/watch.rs`).
//!
//! # Every instance in a directory shares this directory
//!
//! `ResultsDir` is `<temp_root>/results/<cwd_key>` (`background/artifact_roots.rs:281-284`) —
//! keyed by **cwd**, never by session. Running several cyrup instances in one project is ordinary,
//! and they all resolve the identical path. Partitioning results by session on disk
//! ([`crate::background::result_index`]) and gating consumption on two identities
//! ([`crate::background::delivery`]) is therefore what makes this module CORRECT, not an
//! optimisation layered on top of it.
//!
//! This module is the ORCHESTRATOR-side half of the background/async job system's completion
//! signal: it never runs inside the detached runner process (`background/runner_main.rs::run`
//! owns that side — writing the terminal [`crate::background::ResultFile`] into `ResultsDir` as its very last
//! file-writing act, R-SA-077, which is what makes anything in THIS module observable at all).
//! Everything here is pure orchestrator-process, file-watching, dedup, and classification logic —
//! zero process-spawning, zero in-process handle to the runner (R-SA-098's own explicit "never by
//! the orchestrator holding any live handle to the runner process").
//!
//! # R-SA-098: native watch + poll fallback, never a live-handle assumption
//!
//! [`ResultsWatcher::install`] uses `notify::PollWatcher` configured with a fixed poll interval
//! ([`RESULTS_DIR_POLL_INTERVAL`]). `PollWatcher` is deliberately the single implementation for
//! BOTH the "OS-level filesystem-change notification" and the "fixed-interval poll fallback" halves
//! of R-SA-098's requirement — it already falls back to pure polling on platforms/conditions where
//! a native backend (inotify/FSEvents/ReadDirectoryChangesW) is unavailable or hits a
//! resource-exhaustion-class error (`EMFILE`/`ENOSPC`), so there is no second, separately
//! maintained poll-only code path to keep in sync with a native-only one. This mirrors
//! `background::control::watch_control_inbox`'s identical choice for the control-inbox watch
//! (R-SA-082) and `cyrup_resources::theme::ThemeWatcher`'s established workspace convention.
//!
//! # R-SA-099: parse, verify-session, dedup, notify, delete-last
//!
//! [`ResultsWatcher::scan_candidates`] performs the parse+session-filter+dedup+notify portion of R-SA-099's
//! fixed processing order in one pass:
//!
//! 1. **Parse** — a result file that fails to deserialize as [`crate::background::ResultFile`] is silently skipped
//!    (never deleted), matching this crate's "malformed on-disk state degrades gracefully"
//!    convention.
//! 2. **Verify ownership** — decided by
//!    [`crate::background::delivery::Attribution::classify`] from the result's own
//!    [`crate::identity::SessionId`] and [`crate::identity::CompletionOwnerId`], not by a
//!    caller-supplied `RunId` predicate.
//!
//!    An earlier revision of this doc claimed `watch.rs` "has no business knowing what a session
//!    is", citing an architecture rule about keeping `cyrup-session` scoped to `fork_context.rs`.
//!    That reasoning was wrong twice over: [`crate::identity::SessionId`] is a leaf newtype over a
//!    string and creates **no** `cyrup-session` dependency, and the `RunId` predicate it justified
//!    is what left the production drain calling an accept-everything scan — so every instance
//!    consumed and deleted every other instance's results. Ownership is exactly this module's
//!    business.
//!
//!    A result belonging to another live session is **observed but never delivered and never
//!    deleted** ([`crate::background::delivery::Attribution::Foreign`]), so the
//!    instance that owns it still gets it. That was always the documented intent here; only the
//!    wiring was missing.
//! 3. **Dedup** — an in-memory seen-set keyed by the R-SA-099-specified composite (run id / agent /
//!    timestamp) with a bounded TTL ([`DEDUP_TTL`], target ~10 minutes) guards against re-notifying
//!    for the same result twice within one orchestrator process's lifetime.
//! 4. **Notify** — the caller receives every not-yet-seen, owned result as a
//!    [`CompletionNotification`] and is responsible for actually delivering it (re-entering the
//!    normal turn/prompt path, R-SA-101 — see that section below for why this module stops short
//!    of performing the delivery itself).
//!
//! Deletion is **explicitly the caller's own separate act**, via [`ResultsWatcher::consume`],
//! called only AFTER the caller's own downstream delivery has succeeded (R-SA-099: "Deletion MUST
//! happen last (after notification), accepting that a crash between notification and deletion
//! causes at most one duplicate re-notification on restart, never a lost one").
//!
//! # R-SA-100: OR'd terminal-state classification
//!
//! [`classify_outcome`] classifies a [`crate::background::ResultFile`] using two independently-populated signals —
//! the explicit `state` field and the `success` flag — OR'd together rather than relying on either
//! alone, since `state == Complete && !success` (every step individually failed acceptance, but the
//! run itself finished without a run-ending crash) is a real, legitimate combination this crate's
//! own `runner_main::finish_run` can produce. A `Paused` state is NEVER reclassified as `Failed`
//! regardless of `success` (R-SA-100's explicit carve-out).
//!
//! # R-SA-101: re-enters the normal turn/prompt path (deferred to a later phase/file)
//!
//! R-SA-101 requires that consuming a [`CompletionNotification`] for chat/UI delivery "re-enter the
//! orchestrator's normal turn/prompt-handling path (not merely display inert text) so that the
//! parent LLM/agent sees and can act on the background result on its own initiative." This module
//! deliberately stops at producing the plain, inert [`CompletionNotification`] payload — actually
//! injecting that payload into a live session's turn loop requires a handle to session/agent-turn
//! machinery this crate does not hold here (per this crate's own "ZERO dependency on `cyrup-agent`,
//! and `cyrup-session` usage scoped only to `fork_context.rs`/lineage-append" boundary, arch-SA
//! §2.1/§12 item 10). **That hand-off wiring belongs to a later phase's `background/tracker.rs`
//! (R-SA-093's shared poller, which already owns per-session tracked-run bookkeeping) and/or
//! `registration/`'s extension-facade code, which hold the actual `HostCtx`/session-event-sink
//! reference** — this module supplies [`CompletionNotification`] as the exact, complete payload
//! that hand-off consumes, and nothing about its shape needs to change when that later phase is
//! implemented.
//!
//! # R-SA-102: bounded retry-in-place on transient processing failure
//!
//! A caller may fail to actually DELIVER a [`CompletionNotification`] after `scan_candidates`
//! returns it (e.g. the later-phase turn-re-entry hand-off above hits a transient error). R-SA-102
//! says such a failure SHOULD leave the result file in place for retry on the next cycle rather than
//! losing the notification — but SHOULD NOT retry indefinitely without any bound. [`ResultsWatcher`]
//! supports this directly: [`ResultsWatcher::record_processing_failure`] lets the caller tell this
//! watcher "I saw this one but could not process it", which (a) un-marks it from the seen-set so the
//! NEXT `scan_candidates` call re-surfaces it (retry-in-place — the file itself was never
//! touched, so nothing above needs to re-read it from disk) and (b) increments a per-key attempt
//! counter capped at [`MAX_PROCESSING_ATTEMPTS`]; once a key exceeds the bound,
//! [`ResultsWatcher::scan_candidates`] stops re-surfacing it as a normal [`CompletionNotification`]
//! and instead reports it once via [`CompletionNotification::exhausted`] so the caller can log/alert
//! rather than spinning forever on a permanently-broken result file.
//!
//! # R-SA-103: mode-agnostic mechanics
//!
//! Nothing in this module reads or branches on `interactive`/`print`/`json`/`rpc` mode — the
//! on-disk mechanics (watch, parse, dedup, classify, retry-bound) are identical regardless of which
//! mode's caller drives them; R-SA-103 only changes what SURFACES the resulting
//! [`CompletionNotification`]s (a persistent TUI widget vs. a tagged RPC event vs. nothing at all,
//! for `print`/`json`'s "a subsequent separate invocation observes results" case), which is
//! entirely the calling layer's concern, not this module's.

mod classify;
mod install;
mod message;
mod observer;
mod results_watcher;
mod sink;

pub use classify::{ClassifiedOutcome, classify_outcome};
pub use install::{
    CompletionWatcherHandle, install_completion_watcher, install_completion_watcher_with_observer,
};
pub use message::{
    CompletionMessage, completion_notice_display, format_completion_message,
    scheduled_completion_triggers_turn,
};
pub use message::{
    format_missing_payload_message, format_undeliverable_message, result_display_summary,
};
pub use observer::{
    BusAnnouncingCompletionObserver, CompletionBus, CompletionEvent, CompletionObserver,
    CompositeCompletionObserver, SUBAGENT_ASYNC_COMPLETE_EVENT,
};
pub use results_watcher::{
    CompletionBand, CompletionNotification, DEDUP_TTL, LossReport, MAX_PROCESSING_ATTEMPTS,
    MISSING_PAYLOAD_GRACE_SCANS, MissingPayloadVerdict, ObservedResult, RESULTS_DIR_POLL_INTERVAL,
    RecoveredStep, ResolvedCandidate, ResultsWatcher, ScanOutcome,
};
pub use sink::{
    CompletionSink, HostServicesCompletionSink, InlineAnswerClaim, InlineAnswerLedger,
    InlineAnsweredSink, LoggingCompletionSink,
};

/// Fixtures shared by this module's own submodule tests and by [`crate::background::wait`]'s
/// (`ASYNC_NOTIFY_BUG_REPORT` F2's disk replay is rendered from a REAL published payload, and
/// [`crate::exec::SingleResult`] derives no `Default` — so open-coding a child result at the
/// second call site would be a 35-field literal drifting against this one).
///
/// Matches this crate's own `exec::testsupport` / `extension::testsupport` convention:
/// test-only, `pub(crate)`, helper constructors only.
#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::expect_used)]

    use crate::background::{ResultFile, RunId, RunMode, RunState};
    use crate::exec::SingleResult;
    use crate::identity::{CompletionOwnerId, SessionId};
    use std::path::PathBuf;

    /// The session every watcher test's results belong to.
    pub(crate) fn test_session() -> SessionId {
        SessionId::parse("test-session").expect("non-empty")
    }

    /// The process every watcher test's results were launched by.
    pub(crate) fn test_owner() -> CompletionOwnerId {
        CompletionOwnerId::parse("test-owner").expect("non-empty")
    }

    /// Ownership matching [`test_session`]/[`test_owner`] — an instance that OWNS the fixtures.
    pub(crate) fn owning() -> crate::background::delivery::ResultDeliveryOwnership {
        crate::background::delivery::ResultDeliveryOwnership::new(
            Some(test_session()),
            Some(test_owner()),
        )
    }

    pub(crate) fn temp_results_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("real tempdir");
        let results_dir = dir.path().join("results");
        (dir, results_dir)
    }

    /// Publish a result the way production does: staged session-private, indexed, then promoted.
    ///
    /// Writing the payload straight into `results_dir` — which these tests used to do — no longer
    /// makes it discoverable, and that is the point of the change: an unindexed payload is
    /// invisible to the enumerator.
    /// Where `publish_result` promotes a fixture's payload.
    pub(crate) fn published_path(results_dir: &std::path::Path, result: &ResultFile) -> PathBuf {
        crate::background::result_index::owned_payload_path(
            results_dir,
            result
                .session_id
                .as_ref()
                .expect("fixture carries a session"),
            &result.run_id,
        )
    }

    pub(crate) async fn publish_result(results_dir: &std::path::Path, result: &ResultFile) {
        crate::background::result_index::write_async_result_file(
            &crate::background::result_index::ResultWrite {
                results_dir,
                session_id: result
                    .session_id
                    .as_ref()
                    .expect("fixture carries a session"),
                run_id: &result.run_id,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            result,
        )
        .await
        .expect("publish fixture result");
    }

    pub(crate) fn sample_result(run_id: &str, state: RunState, success: bool) -> ResultFile {
        ResultFile {
            schedule_origin: None,
            id: RunId::from_token(run_id),
            run_id: RunId::from_token(run_id),
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state,
            success,
            cwd: PathBuf::from("/tmp"),
            session_file: None,
            session_id: Some(test_session()),
            completion_owner_id: Some(test_owner()),
            results: Vec::new(),
            workflow_children: None,
            workflow_receipt: None,
        }
    }

    // ---------------------------------------------------------------------------------------
    // Completion notification (C6): format + install + deliver-exactly-once + delete
    // ---------------------------------------------------------------------------------------

    pub(crate) fn child_result(
        agent: &str,
        final_output: Option<&str>,
        exit_code: i32,
    ) -> SingleResult {
        SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            child_run_id: None,
            agent: agent.to_string(),
            task: String::new(),
            exit_code,
            usage: cyrup_core::Usage::default(),
            turns: 0,
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: final_output.map(str::to_string),
            structured_output: None,
            session_file: None,
            output_state: Default::default(),
            structured_output_path: None,
            artifact_paths: None,
            transcript_path: None,
            transcript_error: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            timeout_recovery: None,
            context_overflow: false,
            stopped: false,
            process_signal: None,
            error: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
            runner: None,
            external_process: None,
            // Test fixture: no child was planned, so there is no surface to report.
            tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        }
    }

    pub(crate) fn result_with_children(
        run_id: &str,
        state: RunState,
        success: bool,
        session_file: Option<PathBuf>,
        children: Vec<SingleResult>,
    ) -> ResultFile {
        ResultFile {
            schedule_origin: None,
            id: RunId::from_token(run_id),
            run_id: RunId::from_token(run_id),
            agent: "worker".to_string(),
            mode: RunMode::Single,
            state,
            success,
            cwd: PathBuf::from("/tmp"),
            session_file,
            session_id: Some(test_session()),
            completion_owner_id: Some(test_owner()),
            results: children,
            workflow_children: None,
            workflow_receipt: None,
        }
    }
}
