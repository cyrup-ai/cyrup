//! Hop-2 detached-runner main loop (func-SA §5.4 R-SA-073..077/098..103; arch-SA §6.5).
//!
//! This is the single riskiest file in the crate: it is the integration point every other
//! background-subsystem module (`spawn_detached.rs`, `atomic.rs`, `control.rs`, `reconcile.rs`)
//! and the Phase 3 spawn boundary (`spawn/mod.rs`, `spawn/chain_graph.rs`, `spawn/parallel.rs`)
//! all feed into, and it is the ONE place the R-SA-077 "status.json before ResultFile, on EVERY
//! exit path" invariant must hold without exception. `crates/cyrup/src/subagent_runner_cmd.rs`
//! (a sibling crate, outside this one) is the sole caller: it selects the internal
//! `__subagent-runner --config <path>` subcommand and calls [`run`] directly — no separate
//! loader/interpreter hop, since `cyrup` is already one compiled binary.
//!
//! # The main-loop shape (arch-SA §6.5, restated exactly)
//!
//! ```text
//! read+delete config file
//!   -> resolve fork_context is already done (eager, by the orchestrator, R-SA-137) — this loop
//!      only reads the already-resolved session-file path per step, never re-derives it
//!   -> write initial RunStatus{state:Running,pid:self} via atomic.rs        (R-SA-075)
//!   -> spawn a control-inbox watcher task (uses control.rs)                 (R-SA-082)
//!   -> loop {
//!        check interrupted                                                 (R-SA-084)
//!        consume pending append requests via control.rs (re-scan disk)     (R-SA-095/096)
//!        if step cursor exhausted, break
//!        run the next step via the Phase-3 spawn boundary                  (R-SA-045..069)
//!        write status via atomic.rs
//!        advance cursor
//!      }
//!   -> compute terminal state
//!   -> write status.json THEN ResultFile, in that exact order,             (R-SA-077)
//!      on every single exit path (happy path, early return, error branch)
//!   -> exit
//! ```
//!
//! # R-SA-077's ordering invariant is enforced by construction, not by convention
//!
//! Every code path that can end this function's execution — the happy path (steps exhausted),
//! an interrupt (steps paused mid-flight), and an unrecoverable internal error (e.g. the runner
//! config fails to parse) — funnels through exactly one function, [`finish_run`], which performs
//! the `status.json`-write-THEN-`ResultFile`-write sequence unconditionally and returns `()`
//! (never a `Result` a caller could short-circuit past). [`run`] itself has no `return` statement
//! that bypasses `finish_run`: every `?`/early-return branch inside the loop body is caught by an
//! inner `Result`-returning helper ([`run_inner`]) whose own `Err` is turned into a terminal
//! `Failed` status by [`run`]'s own tail, which then always calls `finish_run`. This mirrors this
//! crate's established "no silent bypass of a load-bearing ordering invariant" convention (compare
//! `exec/mod.rs`'s own R-SA-033 post-hoc-correction-must-run-after-completion-guard ordering,
//! enforced the same way: one funnel function, no early return around it).
//!
//! # Delete-then-act idempotency (R-SA-073's config file, mirroring control.rs's own R-SA-083)
//!
//! Reading the one-shot `runner-config.json` handoff file follows the identical delete-then-act
//! discipline `control.rs::consume_interrupt_request` already established for interrupt requests
//! (R-SA-083): the file's *content* is read first (needed to actually build the run), then the
//! file is deleted — and a SECOND call to [`read_and_delete_config`] against an already-consumed
//! config path (the file no longer exists) returns a typed "already consumed" outcome rather than
//! panicking or erroring loudly, so a hypothetical double-invocation of the runner subcommand
//! against the same config path (a supervisor retry, a test harness bug) degrades gracefully
//! instead of crashing. This is NOT the same ordering as `control.rs`'s interrupt consumption
//! (which reads-then-deletes so a lost race against a concurrent consumer still returns the
//! content) — here there is only ever one reader (the one runner process invoked with this exact
//! `--config` path), so a plain "read, then delete, tolerate NotFound on delete" sequence is
//! sufficient and matches R-SA-073's literal text ("the runner MUST delete this config file
//! immediately after reading it").
//!
//! # ResultsDir filesystem-watch completion notification (R-SA-098..103)
//!
//! This module owns only the *runner-side* half of R-SA-098's contract: [`run`] writes the
//! terminal [`super::ResultFile`] into `ResultsDir` as its very last file-writing act (R-SA-077),
//! which is what makes the orchestrator-side watch observable at all. The ORCHESTRATOR-side watch
//! primitive itself (installing a `notify` watcher over the whole `ResultsDir`, deduping by a
//! seen-set with a bounded TTL, R-SA-099, classifying terminal outcomes, R-SA-100, and bounding
//! retry-in-place on processing failure, R-SA-102) runs in the **orchestrator** process — never
//! the detached runner process this file's main loop (`run`) itself executes in — and lives in
//! the sibling module [`crate::background::watch`], per arch-SA §2.2's module layout. See that
//! module's own docs for the full R-SA-098..103 contract.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::SubagentError;
use crate::exec::{self, AgentConfig, RunOptions, SingleResult};
use crate::exec::fallback::ModelOverride;
use crate::exec::output::OutputCap;
use crate::fork_context::{ContextMode, ForkContext};
use crate::spawn::chain_graph::{
    ChainRunContext, OutputRegistry, RunnerStep, SingleStepExecutor, SingleStepSpec, StepResult,
    walk_chain,
};
use crate::spawn::depth::DepthEnvelope;
use crate::spawn::parallel::GlobalConcurrencyLimit;

use super::atomic::write_atomic_json;
use super::control::{self, ChainAppendRequest};
use super::{
    ParallelGroupStatus, ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, StepState,
    StepStatus,
};
use crate::jsonl::BoundedJsonlWriter;

// =================================================================================================
// RunnerConfig — the one-shot handoff file (func-SA §4.5, arch-SA §4.3, R-SA-073)
// =================================================================================================

/// The one-shot `runner-config.json` handoff file's shape (arch-SA §4.3), read exactly once by
/// [`run`] and deleted immediately afterward (R-SA-073). Every field the orchestrator resolves
/// EAGERLY before spawning hop 2 — including every step's fork-context session-file path
/// (R-SA-137, resolved by [`crate::exec::plan_batch`] and baked into each
/// [`SingleStepSpec::session_file`]/[`SingleStepSpec::context`] before this file is ever written)
/// — lives here; the runner process itself never re-derives fork-context, never re-discovers
/// agents, and never re-resolves depth beyond what its own inherited environment
/// ([`crate::spawn::depth::resolve_effective_depth`]) already gives it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerConfig {
    /// This run's identity — MUST match the run id encoded in the `--config` path's own parent
    /// `RunDir` (the caller is responsible for that consistency; this module does not itself
    /// cross-check the two, since the config file's `run_id` is the sole authoritative source
    /// once read).
    pub run_id: RunId,
    /// Which shape of run this is (func-SA §4.5).
    pub mode: RunMode,
    /// The already fully-resolved step list — a flat [`RunnerStep`] sequence for a `Chain` run
    /// (walked via [`walk_chain`]), or, for a `Single`/`Parallel` top-level run, a list whose
    /// single [`RunnerStep`] is either one [`RunnerStep::SingleStep`] or one
    /// [`RunnerStep::ParallelGroup`] respectively — [`run_inner`] does not itself branch on `mode`
    /// for step-execution purposes (the difference is purely how `steps` was constructed by the
    /// orchestrator), it only consults `mode` when constructing the initial/terminal
    /// [`RunStatus`]/[`ResultFile`] records.
    pub steps: Vec<RunnerStep>,
    /// The working directory every step without its own `cwd` override runs in.
    pub cwd: PathBuf,
    /// The top-level persisted session-transcript path, if this run's context is `Fork` at the
    /// top level (threaded into the terminal [`ResultFile::session_file`], R-SA-085's resume
    /// target).
    pub session_file: Option<PathBuf>,
    /// Run-wide global concurrency ceiling (R-SA-050) — resolved once by the orchestrator from
    /// [`crate::registration::SubagentExtensionConfig::global_concurrency_limit`] and handed
    /// through verbatim rather than re-read from config inside the runner process.
    pub global_concurrency_limit: usize,
    /// Base directory for `worktree: true` group isolation (R-SA-060..064), if any group in
    /// `steps` needs one. `None` is fine for a run with no worktree-isolated group.
    pub worktree_base_dir: Option<PathBuf>,
    /// The depth ceiling this run's own children may inherit (R-SA-054/056) — mirrors the
    /// process's own inherited `CYRUP_SUBAGENT_MAX_DEPTH`, carried here so a runner invoked with
    /// no such env var (e.g. a test harness that only sets `--config`) still gets a sane,
    /// explicit ceiling rather than silently falling back to an unbounded one.
    pub max_subagent_depth: u32,
}

// =================================================================================================
// read_and_delete_config — R-SA-073, delete-then-act idempotency
// =================================================================================================

/// The observable outcome of one [`read_and_delete_config`] call — distinguishes "this call
/// actually read and consumed a fresh config" from "the config file was already gone" so a
/// double-invocation of the runner subcommand against the same `--config` path degrades to a
/// typed, non-panicking outcome rather than crashing (this file's own delete-then-act idempotency
/// obligation, mirroring `control.rs::consume_interrupt_request`'s R-SA-083 contract at the
/// config-handoff layer instead of the interrupt-request layer).
#[derive(Debug)]
pub enum ConfigConsumeOutcome {
    /// The config file existed, parsed successfully, and has now been deleted.
    Consumed(RunnerConfig),
    /// The config file did not exist at all when this call ran — either it was already consumed
    /// by a prior call (double-invocation) or it was never written. Either way, this is NOT
    /// treated as a hard error by [`read_and_delete_config`] itself; the caller ([`run`]) decides
    /// what a missing config means for its own control flow (in practice: nothing useful can be
    /// done without step data, so [`run`] surfaces this as a [`SubagentError`] via
    /// [`RunnerConfig`]'s own absence — but the TYPE here stays a plain enum, not a panic, so a
    /// test can assert on this outcome directly without unwinding).
    AlreadyConsumed,
}

/// Read `config_path` as [`RunnerConfig`] JSON, then delete it (R-SA-073: "the runner MUST delete
/// this config file immediately after reading it").
///
/// Read-then-delete (not delete-then-read): the config's CONTENT is what this call exists to
/// obtain, and — unlike `control.rs`'s interrupt-request consumption, where the file's mere
/// *existence* is the entire piece of state being raced over by potentially many concurrent
/// consumers — there is exactly one legitimate reader of a given `runner-config.json` (the one
/// runner process invoked with that exact `--config` path), so there is no concurrent-consumer
/// race to protect against here. What this function DOES guard against is a **double-invocation**
/// of the SAME runner process's own startup path (e.g. a test harness or a supervisor retry
/// re-running `run()` against a config path whose file this process — or an earlier crashed
/// attempt — already consumed): the delete step tolerates the file already being absent
/// (`ErrorKind::NotFound`) as a non-error, silently-absorbed outcome, exactly mirroring
/// `consume_interrupt_request`'s own "duplicate consumption... MUST be silently absorbed, not
/// re-processed" idempotency property, restated here for the config handoff.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the file exists but cannot be read, or exists but fails to
/// parse as valid [`RunnerConfig`] JSON (a malformed config is a genuine anomaly this function
/// surfaces rather than silently treating as "already consumed" — those are two different failure
/// modes and must not be conflated). Never returns an error merely because the file was already
/// absent — that is [`ConfigConsumeOutcome::AlreadyConsumed`], not an `Err`.
pub async fn read_and_delete_config(
    config_path: &Path,
) -> Result<ConfigConsumeOutcome, SubagentError> {
    let bytes = match tokio::fs::read(config_path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigConsumeOutcome::AlreadyConsumed);
        }
        Err(err) => return Err(SubagentError::Spawn(err)),
    };

    let config: RunnerConfig = serde_json::from_slice(&bytes).map_err(|err| {
        SubagentError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    })?;

    // Delete immediately after a successful read (R-SA-073). A NotFound here (lost a race against
    // some other process's cleanup, or the file vanished between our read and this delete) is
    // tolerated exactly like `consume_interrupt_request`'s own delete step — we already have the
    // content in hand, so a delete failure of this specific kind changes nothing about what this
    // call returns.
    match tokio::fs::remove_file(config_path).await {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(SubagentError::Spawn(err)),
    }

    Ok(ConfigConsumeOutcome::Consumed(config))
}

// =================================================================================================
// run — the hop-2 main loop entry point
// =================================================================================================

/// Run the hop-2 detached-runner main loop against the one-shot config at `config_path`
/// (R-SA-073..077).
///
/// `run_paths` locates every well-known file this run writes to
/// ([`super::RunPaths::for_run`], resolved by the caller — `crates/cyrup/src/
/// subagent_runner_cmd.rs` — from the config path's own parent directory structure, per
/// func-SA §4.5's fixed `<AsyncRoot>/<run_id>/` layout, and passed in explicitly here rather than
/// re-derived, since this module has no opinion on where `AsyncRoot`/`ResultsDir` themselves live
/// — that is `registration::SubagentExtensionConfig`'s concern).
///
/// This function's own top-level control flow is exactly the shape documented in this module's
/// header comment: read+delete config -> write initial `Running` status -> spawn the control-inbox
/// watcher -> the step loop -> compute terminal state -> [`finish_run`] (status THEN result, on
/// every exit path) -> return. Every fallible step inside is funneled through [`run_inner`] so a
/// SINGLE tail call to [`finish_run`] is the only place either file gets written to its terminal
/// form, regardless of which branch produced that terminal state (steps-exhausted happy path,
/// mid-loop interrupt, or an internal error surfaced by `run_inner` itself).
///
/// # Errors
///
/// This function itself is effectively infallible from the CALLER's point of view — every
/// internal failure is captured into a terminal `Failed` [`RunStatus`]/[`ResultFile`] pair rather
/// than propagated as a `Result::Err`, since there is no one left to hand an `Err` to once this
/// process is the detached runner (R-SA-078: "the orchestrator MUST NOT assume a live IPC channel
/// to the runner" — there is no return channel for a `Result` to travel back through). The
/// `Result` return type exists purely so `crates/cyrup/src/subagent_runner_cmd.rs` can log a
/// diagnostic and choose its own process exit code; it carries NO information this function's own
/// on-disk writes have not already durably recorded.
pub async fn run(config_path: &Path, run_paths: &RunPaths) -> Result<(), SubagentError> {
    let outcome = read_and_delete_config(config_path).await;

    let config = match outcome {
        Ok(ConfigConsumeOutcome::Consumed(config)) => config,
        Ok(ConfigConsumeOutcome::AlreadyConsumed) => {
            // R-SA-073's delete-then-act idempotency, restated at the top level: a double
            // invocation against an already-consumed config has nothing to build a run from.
            // There is no prior in-flight run THIS process instance is aware of (a genuinely
            // resumed/steered run goes through `control::resume`, never a second `run()` call
            // against the same one-shot file) — surface a terminal Failed record so a caller
            // polling this run id sees a definitive, non-hanging outcome rather than silence.
            let run_id = run_id_from_paths(run_paths);
            let status = RunStatus::queued(run_id, RunMode::Single, Some(std::process::id()));
            finish_run(
                run_paths,
                status,
                RunState::Failed,
                Vec::new(),
                PathBuf::new(),
                None,
                "runner-config.json was already consumed (double-invocation of the runner \
                 subcommand against the same --config path); nothing to run"
                    .to_string(),
            )
            .await;
            return Ok(());
        }
        Err(err) => {
            // No config at all to build even a run-id-bearing status from in the ordinary case —
            // but `run_paths` itself still encodes a run id (its own directory name), so a
            // terminal Failed record can still be synthesized and written, giving any orchestrator
            // watching this run id a definitive answer instead of an indefinitely "Queued" ghost.
            let run_id = run_id_from_paths(run_paths);
            let status = RunStatus::queued(run_id, RunMode::Single, Some(std::process::id()));
            finish_run(
                run_paths,
                status,
                RunState::Failed,
                Vec::new(),
                PathBuf::new(),
                None,
                format!("failed to read runner-config.json: {err}"),
            )
            .await;
            return Ok(());
        }
    };

    // R-SA-075: initial status.json (state=Running, pid=own pid), written BEFORE any step work.
    let mut status = RunStatus::queued(config.run_id.clone(), config.mode, Some(std::process::id()));
    status.chain_step_count = Some(config.steps.len());
    status.steps = config
        .steps
        .iter()
        .map(pending_step_status_for)
        .collect();
    // Queued -> Running is always legal (RunState::can_transition_to).
    if status.advance_state(RunState::Running).is_err() {
        // Unreachable in practice (a freshly `queued` status can always advance to Running), but
        // this crate never unwraps a Result — if the transition guard were ever tightened in a
        // way that made this fail, degrade to a terminal Failed record rather than panicking.
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            "internal error: Queued -> Running transition was rejected".to_string(),
        )
        .await;
        return Ok(());
    }
    if let Err(err) = write_atomic_json(&run_paths.status, &status).await {
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            format!("failed to write initial status.json: {err}"),
        )
        .await;
        return Ok(());
    }

    // Install a SIGUSR2 handler BEFORE anything else that could race an interrupt delivery
    // (R-SA-081's wake-up signal, sent by `control::deliver_wakeup_signal`): on both Linux and
    // macOS, SIGUSR2's default disposition is process TERMINATION. Without an installed handler,
    // the very act of a caller trying to softly interrupt this run would instead kill the runner
    // outright — the opposite of R-SA-084's "interrupt is soft, not fatal" guarantee, and the
    // interrupt would never even reach `run_inner`'s own cooperative `interrupted` check. The
    // signal's payload itself is not consulted for anything: `control::watch_control_inbox`'s
    // filesystem-notification mechanism (installed by `spawn_control_watcher` immediately below)
    // is the actual, authoritative "an interrupt/append request landed" signal per DI-SA-9
    // (file-based control, never live IPC) — SIGUSR2 exists purely to nudge that watcher/poll
    // loop awake sooner than its next scheduled tick, so this handle's only job is to keep
    // existing for this function's whole lifetime (held via `_sigusr2_guard`) so the OS routes
    // the signal to a registered handler instead of applying its default terminate action; a
    // received signal is otherwise fully drained/ignored.
    #[cfg(unix)]
    let _sigusr2_guard = install_ignored_sigusr2_handler();

    // The control-inbox directory (`<run_dir>/control/`) MUST exist before
    // `spawn_control_watcher` installs its `notify::PollWatcher` below: that watcher targets the
    // DIRECTORY, not the (not-yet-existing, created-on-first-interrupt) file itself, since
    // watching a not-yet-existing file path is unreliable across platforms (see
    // `control::watch_control_inbox`'s own doc). Watching a directory that does not exist YET
    // fails to install at all on every platform this crate ships to — and `spawn_control_watcher`
    // degrades that failure to a silent no-op (by design, so a watcher failure never strands the
    // run), which would silently make EVERY interrupt delivered after this point unobservable:
    // `run_inner`'s own per-iteration re-check only re-scans pending chain-append requests
    // (R-SA-096), it has no independent interrupt-file poll fallback of its own — the `interrupted`
    // flag is set SOLELY by this watcher task. Creating the directory here, unconditionally,
    // before the watcher is installed, closes that gap.
    //
    // This MUST route through `finish_run` on failure, matching every other pre-loop fallible step
    // immediately above (never a bare `?`, found bypassing `finish_run` entirely in second-pass
    // adversarial review): a bare `?` here would return `Err` straight out of `run` itself, leaving
    // `status.json` permanently stuck at the `Running` record already written above and NO
    // `ResultFile` ever written — directly contradicting this function's own documented "effectively
    // infallible from the caller's point of view" contract (every internal failure captured into a
    // terminal on-disk record, never propagated) and silently violating R-SA-077's ordering
    // invariant by skipping BOTH writes rather than merely reordering them.
    if let Err(err) = tokio::fs::create_dir_all(
        run_paths
            .control_inbox
            .parent()
            .unwrap_or(&run_paths.run_dir),
    )
    .await
    {
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            format!("failed to create control-inbox directory: {err}"),
        )
        .await;
        return Ok(());
    }

    // R-SA-082: control-inbox watcher, installed with the mandatory synchronous startup check
    // performed FIRST (catches a request written in the race window before the watcher attaches),
    // then a background task forwarding every watch notification into `interrupted`.
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if control::check_control_inbox_now(run_paths)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let _watcher_task = spawn_control_watcher(run_paths.clone(), Arc::clone(&interrupted));

    // R-SA-136/146: open the size-capped `events.jsonl` writer for this run, via the SAME shared
    // `BoundedJsonlWriter` primitive `spawn::SpawnedChild`'s per-attempt child-output tee uses
    // (`jsonl.rs`'s own module doc names this exact call site as one of its two intended writers).
    // A failure to open it (e.g. an unwritable run directory) degrades to `None` — `append_event`
    // then silently no-ops on every call — rather than failing this run over a best-effort
    // diagnostic log, mirroring every other non-`status.json`/`ResultFile` write in this function.
    let mut events = BoundedJsonlWriter::create(&run_paths.events).await.ok();
    append_event(&mut events, "run.started", None).await;

    // The step loop itself, all failure modes funneled to a single Result the tail below always
    // routes through `finish_run`.
    let loop_outcome = run_inner(&config, run_paths, &mut status, &interrupted, &mut events).await;

    let (terminal_state, results, final_error) = match loop_outcome {
        Ok(LoopOutcome::Completed { results }) => {
            let all_ok = results.iter().all(|r| r.exit_code == 0);
            append_event(&mut events, "run.completed", None).await;
            (
                if all_ok { RunState::Complete } else { RunState::Failed },
                results,
                None,
            )
        }
        Ok(LoopOutcome::Interrupted { results }) => {
            append_event(&mut events, "run.paused", None).await;
            (RunState::Paused, results, None)
        }
        Err(err) => {
            append_event(
                &mut events,
                "run.failed",
                Some(serde_json::json!({ "error": err.to_string() })),
            )
            .await;
            (RunState::Failed, Vec::new(), Some(err.to_string()))
        }
    };

    finish_run(
        run_paths,
        status,
        terminal_state,
        results,
        config.cwd.clone(),
        config.session_file.clone(),
        final_error.unwrap_or_default(),
    )
    .await;

    Ok(())
}

/// R-SA-136/146: append one JSON-shaped line to this run's `events.jsonl` via the shared
/// [`BoundedJsonlWriter`] primitive, if a writer was successfully opened for this run (`events` is
/// `None` only when [`BoundedJsonlWriter::create`] itself failed at startup — see [`run`]'s own
/// construction site — in which case this is a silent no-op, matching this crate's established
/// "a `.jsonl` artifact's own failure never fails the run" convention, restated here at the
/// writer-availability level rather than only the per-line byte-cap level
/// [`BoundedJsonlWriter::write_line`] already enforces internally).
///
/// `kind` is a short, stable event-type tag (`"run.started"`, `"step.started"`, `"step.completed"`,
/// `"run.paused"`, `"run.completed"`) mirroring the shape [`super::tracker`]'s own tailing-consumer
/// doc comment and test fixtures already assume for this file (one JSON object per line, a `kind`
/// field identifying the event). `detail` is folded into the same JSON object as additional fields
/// when present, so a consumer never has to parse a nested string-encoded sub-document.
async fn append_event(
    events: &mut Option<BoundedJsonlWriter>,
    kind: &str,
    detail: Option<serde_json::Value>,
) {
    let Some(writer) = events.as_mut() else {
        return;
    };
    let mut object = serde_json::Map::new();
    object.insert("kind".to_string(), serde_json::Value::String(kind.to_string()));
    object.insert(
        "ts".to_string(),
        serde_json::Value::from(super::now_epoch_millis_pub()),
    );
    if let Some(serde_json::Value::Object(fields)) = detail {
        for (key, value) in fields {
            object.insert(key, value);
        }
    }
    let line = serde_json::Value::Object(object).to_string();
    // A write failure here (genuine I/O error while still under the byte cap — the cap itself is
    // always a silent no-op, never an `Err`) is likewise never allowed to fail the run: this event
    // log is a best-effort diagnostic/tailing aid (R-SA-093), not part of R-SA-077's authoritative
    // status.json/ResultFile durability contract.
    let _ = writer.write_line(&line).await;
}

/// Best-effort recovery of a [`RunId`] from `run_paths`' own `run_dir` path (its final component
/// is always the run id, per [`RunPaths::for_run`]'s construction) — used only on the
/// no-config-available error paths above, where no [`RunnerConfig::run_id`] exists to read.
fn run_id_from_paths(run_paths: &RunPaths) -> RunId {
    run_paths
        .run_dir
        .file_name()
        .map(|name| RunId::from_token(name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| RunId::from_token("unknown-run"))
}

/// A freshly declared, `Pending` [`StepStatus`] for one [`RunnerStep`] — the agent name shown is
/// the step's own agent for a [`RunnerStep::SingleStep`], or a synthesized `"<n> parallel
/// tasks>"`-shaped label for a group step (whose own per-child detail lives in
/// `RunStatus::parallel_groups`, not this top-level `steps` list's single entry for the group).
fn pending_step_status_for(step: &RunnerStep) -> StepStatus {
    match step {
        RunnerStep::SingleStep(spec) => StepStatus::pending(spec.agent.clone()),
        RunnerStep::ParallelGroup(group) => {
            StepStatus::pending(format!("<parallel:{} tasks>", group.steps.len()))
        }
        RunnerStep::DynamicGroup(dynamic) => {
            StepStatus::pending(format!("<dynamic:{}>", dynamic.collect))
        }
    }
}

// =================================================================================================
// run_inner — the step loop itself
// =================================================================================================

/// The step loop's own outcome, BEFORE `run`'s tail maps it into a terminal [`RunState`] — kept
/// distinct from a bare `Vec<SingleResult>` so the interrupted-vs-completed distinction (R-SA-084:
/// `Paused`, never `Failed`) survives without `run_inner` itself needing to know how its caller
/// will map either variant onto [`RunState`].
enum LoopOutcome {
    /// The step cursor was exhausted without an interrupt — every step in `results` ran to its
    /// own completion (success or failure; `run`'s tail decides `Complete` vs. `Failed` overall
    /// from `results`' own exit codes).
    Completed { results: Vec<SingleResult> },
    /// An interrupt was observed and consumed before the step cursor was exhausted — `results`
    /// holds every step that DID complete before the interrupt landed; steps that never got to
    /// run are left `Pending` in `status.steps` (R-SA-084: "mark every currently-running step
    /// Paused... before signaling its own actively-spawned child subprocess(es)" — this phase has
    /// no live child to signal mid-step since interrupts are only checked BETWEEN steps, see this
    /// function's own doc note on that scope boundary).
    Interrupted { results: Vec<SingleResult> },
}

/// Drive the step-execution loop itself (R-SA-076 write-ordering per iteration, R-SA-084 interrupt
/// check, R-SA-095/096 append-request consumption, dispatch via the Phase-3 spawn boundary).
///
/// # Interrupt-check granularity (a deliberate, documented scope boundary)
///
/// This phase checks `interrupted` strictly BETWEEN steps — at the top of every loop iteration,
/// before dispatching the next step — never WITHIN a single step's own child-process lifetime.
/// R-SA-084's "signaling its own actively-spawned child subprocess(es) with SIGINT" (the
/// mid-step-interrupt case, where a step's own live child must itself be torn down) requires
/// threading the SAME `interrupted`-derived [`cyrup_core::CancelToken`] into
/// [`exec::RunOptions::interrupt`] for the step currently in flight — a wiring this function DOES
/// perform (see [`run_single_step`]'s construction of `RunOptions`), so a step's own child IS
/// interruptible mid-flight via the normal `exec::run_sync` -> `drive_attempt` -> `SpawnedChild::
/// terminate` path (R-SA-036/059/084's shared signal-escalation mechanism); what this loop itself
/// additionally re-checks between steps is purely the "should I even START the next step" gate,
/// which is this function's own, distinct responsibility from a step's internal interruptibility.
///
/// # Errors
///
/// Returns `Err` only for a genuine I/O failure writing `status.json` mid-loop (R-SA-076) — a
/// single step's own failure (nonzero exit, timeout, etc.) is NOT an `Err` here; it is recorded as
/// a `SingleResult` with a nonzero `exit_code` and the loop continues to the next step exactly as
/// R-SA-052's chain-walk semantics dictate (a chain does not abort on one step's failure unless
/// the group itself is `fail_fast`, which `walk_chain`/`run_bounded` already enforce internally).
async fn run_inner(
    config: &RunnerConfig,
    run_paths: &RunPaths,
    status: &mut RunStatus,
    interrupted: &Arc<std::sync::atomic::AtomicBool>,
    events: &mut Option<BoundedJsonlWriter>,
) -> Result<LoopOutcome, SubagentError> {
    let mut steps = config.steps.clone();
    let mut cursor = 0usize;
    let mut results: Vec<SingleResult> = Vec::new();
    let mut registry = OutputRegistry::new();

    let depth = crate::spawn::depth::resolve_effective_depth(config.max_subagent_depth);

    // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST in this loop's own setup — before
    // any step's discovery-free-but-still-real worktree setup (`chain_graph::assign_worktree_cwds`
    // -> `spawn::worktree::setup_worktree_group`, which shells out to real `git` subprocesses) or
    // any child OS process is spawned for ANY step in this run's chain. This hop-2 runner process
    // is itself already one recursion hop deep (its own `depth.current_depth` reflects however
    // many ancestors spawned it, propagated via `CYRUP_SUBAGENT_DEPTH`/`_MAX_DEPTH`, R-SA-054) —
    // if that inherited envelope is already at its ceiling, this run must reject EVERY one of its
    // configured steps up front rather than dispatching the first one and only then discovering
    // `ExecSingleStepExecutor::run_single` -> `exec::run_sync`'s own independent re-check rejects
    // it (which would still be correct per R-SA-055's letter for that one step, since no spawn
    // would have happened yet, but would incorrectly leave every LATER step in `steps` looking
    // like it was simply never reached rather than explicitly blocked, and would run any
    // `worktree: true` group's real `git worktree add` setup for nothing before the per-child
    // dispatch inside `run_bounded` ever reached `run_sync`'s own guard). Failing the whole run
    // here, before the loop even starts, keeps the rejection uniform across every step shape
    // (`SingleStep`/`ParallelGroup`/`DynamicGroup`) and guarantees zero worktrees and zero child
    // processes are ever created for a run whose own depth is already exhausted.
    if crate::spawn::depth::is_blocked(&depth) {
        return Err(SubagentError::DepthExceeded {
            current: depth.current_depth,
            max: depth.max_depth,
        });
    }

    let global_limit = GlobalConcurrencyLimit::new(config.global_concurrency_limit.max(1));
    let cancel_root = cyrup_core::CancelToken::new();
    let executor: Arc<dyn SingleStepExecutor> = Arc::new(ExecSingleStepExecutor {
        depth,
        interrupted: Arc::clone(interrupted),
    });
    let ctx = ChainRunContext {
        cwd: config.cwd.clone(),
        deadline_at: None, // R-SA-036: background runs have no built-in wall-clock timeout.
        cancel: cancel_root.clone(),
        global_limit,
        worktree_base_dir: config.worktree_base_dir.clone(),
    };

    loop {
        // R-SA-084: check interrupted FIRST, before consuming appends or dispatching further
        // work — an interrupt that lands must stop new-step dispatch as soon as this loop next
        // observes it, not after one more (possibly append-extended) step has already started.
        //
        // Race guard (found in second-pass adversarial review): a natural completion and an
        // interrupt delivery can land in the same instant — `interrupt()` reads `status.json` and
        // sees `state: Running` (which stays true right up until `finish_run` writes the terminal
        // record), so it can successfully write a control-inbox request and set `interrupted` in
        // the tiny window AFTER this loop's last step already finished (`cursor` already advanced
        // past the final index) but BEFORE this loop's next top-of-iteration check. Without the
        // `cursor < steps.len()` guard below, that late, moot interrupt would still be consumed
        // and reported as `LoopOutcome::Interrupted`, downgrading a run whose every step actually
        // completed into a non-terminal `Paused` `ResultFile` (`success: false`) with no step left
        // to resume from — a permanently-wrong terminal record, since nothing ever reconciles a
        // `Paused` run back to `Complete` after the fact. Only treat the interrupt as a genuine
        // pause when there is still unstarted/unfinished step work for it to actually pause;
        // otherwise silently absorb it (matching R-SA-083's own "duplicate/stale signal MUST be
        // silently absorbed" idempotency principle, applied here to a signal that is stale relative
        // to the run's own already-finished work rather than stale relative to a prior consumption)
        // and let the loop fall through to its normal `Completed` exit on this same iteration.
        if interrupted.load(std::sync::atomic::Ordering::SeqCst) && cursor < steps.len() {
            if let Some(request) = control::consume_interrupt_request(run_paths).await? {
                mark_remaining_paused(status, cursor, steps.len());
                status.touch();
                write_atomic_json(&run_paths.status, status)
                    .await
                    .map_err(SubagentError::Spawn)?;
                let _ = request; // consumed; contents already reflected via status/event log.
                return Ok(LoopOutcome::Interrupted { results });
            }
            // The watcher observed a notification but a synchronous re-check found nothing
            // pending (already consumed by a race, or a stale wake-up) — R-SA-083's idempotent
            // absorption, restated here: clear the flag and keep going rather than looping forever
            // treating a one-shot notification as sticky.
            interrupted.store(false, std::sync::atomic::Ordering::SeqCst);
        }

        // R-SA-095/096: consume pending append requests EVERY iteration, before checking whether
        // the step cursor is exhausted — re-scans disk (never trusts the in-memory `steps` list as
        // the source of truth for what is pending), per R-SA-096's explicit "MUST re-scan disk,
        // not cache" requirement.
        let pending = control::list_pending_appends(&run_paths.append_dir).await?;
        if !pending.is_empty() {
            for (path, parsed) in pending {
                if let Some(request) = parsed {
                    append_steps(&mut steps, status, &request);
                }
                // Delete-then-act, at-most-once (R-SA-095: "MUST list, read, and DELETE all
                // pending request files... and only then extend its own in-loop step list").
                let _ = tokio::fs::remove_file(&path).await;
            }
            let pending_count = control::count_pending_appends(&run_paths.append_dir).await?;
            status.pending_appends = Some(pending_count);
            status.chain_step_count = Some(steps.len());
            status.touch();
            write_atomic_json(&run_paths.status, status)
                .await
                .map_err(SubagentError::Spawn)?;
        }

        if cursor >= steps.len() {
            return Ok(LoopOutcome::Completed { results });
        }

        let step = steps
            .get(cursor)
            .cloned()
            .ok_or_else(|| SubagentError::Spawn(std::io::Error::other("step cursor out of range")))?;

        mark_step_running(status, cursor);
        status.current_step = Some(cursor);
        status.touch();
        write_atomic_json(&run_paths.status, status)
            .await
            .map_err(SubagentError::Spawn)?;
        append_event(
            events,
            "step.started",
            Some(serde_json::json!({ "index": cursor })),
        )
        .await;

        // Dispatch via the Phase-3 spawn boundary (chain_graph::walk_chain over a ONE-element
        // graph for this single cursor position — reusing the exact same SingleStep/ParallelGroup/
        // DynamicGroup dispatch `walk_chain` already implements, rather than re-implementing group
        // fan-out inline here). `ChainGraph` is a plain `Vec<RunnerStep>` type alias, so the
        // one-element "graph" is just a fresh one-element `Vec`.
        let one_step: Vec<RunnerStep> = vec![step.clone()];
        let (step_results, group_results) =
            walk_chain(&one_step, &mut registry, &executor, &ctx).await?;

        let step_result = step_results.into_iter().next().ok_or_else(|| {
            SubagentError::Spawn(std::io::Error::other(
                "walk_chain produced no result for a single dispatched step",
            ))
        })?;

        record_step_outcome(status, cursor, &step, &step_result, group_results.first());
        append_event(
            events,
            "step.completed",
            Some(serde_json::json!({ "index": cursor, "success": step_result.success })),
        )
        .await;
        results.push(step_result_to_single_result(&step, &step_result));

        status.touch();
        write_atomic_json(&run_paths.status, status)
            .await
            .map_err(SubagentError::Spawn)?;

        cursor += 1;
    }
}

/// Mark every step from `from_index` (inclusive) through `total` as `Paused` with an end
/// timestamp (R-SA-084: "mark every currently-running step Paused... with an end timestamp"),
/// including the step at `from_index` itself (the one that was `Running` — or about to be — at
/// the moment the interrupt was observed) — steps strictly before `from_index` are left however
/// [`record_step_outcome`] already left them (their own genuine terminal/paused state from having
/// actually run), and steps at or after `from_index` that were never even started are likewise
/// moved out of `Pending` into `Paused` rather than left looking like they simply never got a
/// turn, since R-SA-084 does not distinguish "was mid-flight" from "was about to start" for the
/// purpose of this marking.
fn mark_remaining_paused(status: &mut RunStatus, from_index: usize, total: usize) {
    let now = super::now_epoch_millis_pub();
    for index in from_index..total {
        if let Some(step) = status.steps.get_mut(index)
            && !step.status.is_terminal()
        {
            step.status = StepState::Paused;
            step.ended_at.get_or_insert(now);
        }
    }
    if let Some(groups) = &mut status.parallel_groups {
        for group in groups {
            if group.group_step_index >= from_index {
                for child in &mut group.children {
                    if !child.status.is_terminal() {
                        child.status = StepState::Paused;
                        child.ended_at.get_or_insert(now);
                    }
                }
            }
        }
    }
}

fn mark_step_running(status: &mut RunStatus, index: usize) {
    if let Some(step) = status.steps.get_mut(index) {
        step.status = StepState::Running;
        step.started_at.get_or_insert(super::now_epoch_millis_pub());
    }
}

/// Fold one completed step's [`StepResult`] (and, for a group step, its [`GroupStepResult`]'s own
/// per-child detail) back into `status.steps[index]`/`status.parallel_groups`.
fn record_step_outcome(
    status: &mut RunStatus,
    index: usize,
    step: &RunnerStep,
    result: &StepResult,
    group_result: Option<&crate::spawn::chain_graph::GroupStepResult>,
) {
    let now = super::now_epoch_millis_pub();
    if let Some(entry) = status.steps.get_mut(index) {
        entry.status = if result.success { StepState::Complete } else { StepState::Failed };
        entry.ended_at = Some(now);
        entry.error = result.error.clone();
    }

    if let (RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_), Some(group)) =
        (step, group_result)
    {
        let children: Vec<StepStatus> = group
            .children
            .iter()
            .map(|child| {
                let mut s = StepStatus::pending("<group-child>");
                s.started_at = Some(now);
                s.ended_at = Some(now);
                match child {
                    Some(outcome) => {
                        s.status = if outcome.success { StepState::Complete } else { StepState::Failed };
                        s.error = outcome.error.clone();
                    }
                    None => {
                        s.status = StepState::Failed;
                        s.error = Some("skipped (fail-fast or cancellation)".to_string());
                    }
                }
                s
            })
            .collect();
        let entry = ParallelGroupStatus {
            group_step_index: index,
            children,
        };
        status.parallel_groups.get_or_insert_with(Vec::new).push(entry);
    }
}

/// Append a [`ChainAppendRequest`]'s steps to the in-loop `steps` list AND `status.steps`
/// (R-SA-095's "only then extend its own in-loop step list/`status.json`'s `steps`/
/// `chain_step_count`" — both updated together so they never observably diverge).
fn append_steps(steps: &mut Vec<RunnerStep>, status: &mut RunStatus, request: &ChainAppendRequest) {
    for step in &request.steps {
        status.steps.push(pending_step_status_for(step));
        steps.push(step.clone());
    }
}

/// Collapse one [`StepResult`] (this file's narrow, chain-graph-local result shape) into a full
/// [`SingleResult`] (func-SA §4.3's canonical per-run record, the shape [`ResultFile::results`]
/// actually stores) — a group step's aggregate is likewise represented as one [`SingleResult`]
/// entry (per-child detail already folded into `status.parallel_groups` by
/// [`record_step_outcome`]; the terminal [`ResultFile`] carries the same one-entry-per-top-level-
/// step shape `status.steps` does, not a flattened per-child list).
fn step_result_to_single_result(step: &RunnerStep, result: &StepResult) -> SingleResult {
    let agent = match step {
        RunnerStep::SingleStep(spec) => spec.agent.clone(),
        RunnerStep::ParallelGroup(group) => format!("<parallel:{} tasks>", group.steps.len()),
        RunnerStep::DynamicGroup(dynamic) => format!("<dynamic:{}>", dynamic.collect),
    };
    let task = match step {
        RunnerStep::SingleStep(spec) => spec.task.clone(),
        RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_) => String::new(),
    };
    SingleResult {
        agent,
        task,
        exit_code: i32::from(!result.success),
        usage: cyrup_core::Usage::default(),
        model: None,
        attempted_models: Vec::new(),
        model_attempts: Vec::new(),
        final_output: result.final_output.clone(),
        structured_output: result.structured_output.clone(),
        acceptance: None,
        detached: false,
        interrupted: false,
        timed_out: false,
        error: result.error.clone(),
        tool_calls: Vec::new(),
        output_truncated: false,
    }
}

// =================================================================================================
// ExecSingleStepExecutor — the real, subprocess-spawning SingleStepExecutor (func-SA §1.1)
// =================================================================================================

/// The production [`SingleStepExecutor`] this runner's [`walk_chain`] calls dispatch through: runs
/// one [`SingleStepSpec`] to completion via [`exec::run_sync`], which — per func-SA §1.1's
/// mandated mechanism — spawns a genuine OS subprocess re-exec of the `cyrup` binary for every
/// attempt. This struct itself spawns nothing directly; it is a thin adapter translating a
/// [`SingleStepSpec`] (this file's/`chain_graph`'s own data-only step shape) into the
/// [`AgentConfig`]/[`RunOptions`] pair `exec::run_sync` actually consumes.
///
/// `pub(crate)` (rather than private to this module) so `extension.rs`'s FOREGROUND `/chain`,
/// `/parallel`, and `/run-chain` slash-command dispatch (R-SA-130: same executor as every other
/// call site, never a second divergent implementation) can drive the exact same
/// [`SingleStepExecutor`] this hop-2 background runner uses, rather than hand-rolling a second
/// `SingleStepSpec` -> `AgentConfig`/`RunOptions` adapter that could silently drift out of sync
/// with this one.
pub(crate) struct ExecSingleStepExecutor {
    pub(crate) depth: DepthEnvelope,
    pub(crate) interrupted: Arc<std::sync::atomic::AtomicBool>,
}

impl ExecSingleStepExecutor {
    /// Construct one for a FOREGROUND (non-detached-runner) caller: no live interrupt signal
    /// source exists at this call site (a foreground `/chain`/`/parallel`/`/run-chain` slash
    /// command has no control-inbox watcher, R-SA-082, of its own — that machinery is exclusively
    /// the hop-2 detached runner's), so `interrupted` starts (and stays) `false` for the lifetime
    /// of this executor; cancellation for a foreground run is instead carried by
    /// [`crate::spawn::chain_graph::ChainRunContext::cancel`], which every dispatched step's own
    /// `RunOptions::cancel` already threads through `exec::run_sync` regardless of this flag.
    #[must_use]
    pub(crate) fn foreground(depth: DepthEnvelope) -> Self {
        Self {
            depth,
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl SingleStepExecutor for ExecSingleStepExecutor {
    async fn run_single(
        &self,
        step: &SingleStepSpec,
        resolved_task: &str,
        ctx: &ChainRunContext,
    ) -> Result<StepResult, SubagentError> {
        let agent = AgentConfig {
            name: step.agent.clone(),
            model: step.model.clone(),
            fallback_models: Vec::new(),
            system_prompt_mode: crate::discovery::types::SystemPromptMode::Replace,
            system_prompt_body: String::new(),
            tools: step.tools.clone(),
            output: None,
            completion_guard: Some(false),
            max_output: OutputCap::default(),
            max_subagent_depth: step.max_depth_override,
            depth: self.depth,
        };

        // `fallback::build_model_candidates` builds its ladder from `model_override` (if any) +
        // `agent.model` + `agent.fallback_models`, filtered against `available_models` — an EMPTY
        // ladder (before the `available_models` filter is even applied) results in an immediate
        // `run_sync` failure ("no candidate model available for this subagent run"), so a step
        // with no explicit `model` override needs a genuine candidate to reach `run_sync` at all,
        // not merely a non-empty `available_models` list on its own.
        //
        // Real per-agent model resolution (an agent-persona's own configured `model`/
        // `fallback_models`, R-SA-038's own agent-definition-driven half) is owned by the
        // discovery subsystem (`discovery::types::AgentDefinition`), which this runner has no
        // dependency on — agent lookup by name is a separate, not-yet-wired concern this file does
        // not own (see this file's module doc: the runner reads an ALREADY-resolved step list from
        // `RunnerConfig`, it never re-discovers agents). Until a later phase threads a resolved
        // `AgentDefinition`'s own model/fallback list through `SingleStepSpec` (or a sibling
        // field), this executor synthesizes a single placeholder candidate — via BOTH
        // `model_override` (so the ladder is non-empty even with `agent.model == None`) and
        // `available_models` (so that candidate survives the availability filter) — for the common
        // "no explicit per-step model override" case. `exec::run_sync`'s own `--model <candidate>`
        // argv construction passes this value straight through to the spawned child, which (being
        // the real `cyrup` binary in production, or the scripted fixture in tests) is what
        // actually resolves what "no override" should mean at that layer.
        const DEFAULT_MODEL_PLACEHOLDER: &str = "default";
        let resolved_model = step
            .model
            .clone()
            .unwrap_or_else(|| cyrup_core::ModelId::from(DEFAULT_MODEL_PLACEHOLDER));
        let model_override = ModelOverride::Explicit(resolved_model.clone());
        let available_models: Vec<cyrup_core::ModelId> = vec![resolved_model];

        let interrupt_token = cyrup_core::CancelToken::new();
        if self.interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            interrupt_token.cancel();
        }

        let fork_context = match &step.session_file {
            Some(path) => ForkContext {
                mode: ContextMode::Fork,
                session_file_path: Some(path.clone()),
            },
            None => ForkContext::fresh(),
        };

        let opts = RunOptions {
            cwd: step.cwd.clone().unwrap_or_else(|| ctx.cwd.clone()),
            deadline_at: ctx.deadline_at,
            output_path: None,
            output_mode: step
                .output_mode
                .unwrap_or(crate::discovery::types::OutputMode::Inline),
            structured_output_schema: step.structured_output_schema.clone(),
            model_override,
            preferred_provider: None,
            available_models,
            cancel: ctx.cancel.clone(),
            interrupt: interrupt_token,
            share: None,
            session_dir: None,
            include_progress: None,
            agent_scope: step.agent_scope,
            acceptance: None,
            fork_context,
        };

        let result = exec::run_sync(&agent, resolved_task, &opts).await;

        if result.exit_code == 0 {
            Ok(StepResult::success(
                result.final_output,
                result.structured_output,
            ))
        } else {
            Ok(StepResult::failure(result.error.unwrap_or_else(|| {
                format!("subagent step '{}' exited with code {}", agent.name, result.exit_code)
            })))
        }
    }
}

// =================================================================================================
// install_ignored_sigusr2_handler — survive R-SA-081's best-effort wake-up signal
// =================================================================================================

/// Install a handler for `SIGUSR2` (R-SA-081's best-effort wake-up signal, sent by
/// [`control::deliver_wakeup_signal`] to nudge this runner's control-inbox watcher awake sooner)
/// that does nothing but drain and discard every received signal, for as long as the returned
/// task handle is kept alive.
///
/// This is REQUIRED, not defensive-programming excess: `SIGUSR2`'s default disposition on every
/// Unix target this crate ships to (Linux, macOS) is process TERMINATION. Without a registered
/// handler, `interrupt()`'s signal send would kill this runner process outright — silently
/// converting every "soft" R-SA-084 interrupt into a hard crash before `run_inner`'s own
/// cooperative `interrupted` flag ever gets a chance to observe anything, which would make a
/// `Paused` outcome unreachable by the very code path that is supposed to produce it.
///
/// The handler itself does nothing with the signal's payload — `control::watch_control_inbox`'s
/// filesystem notification (started by [`spawn_control_watcher`] immediately after this function
/// is called) and `run_inner`'s own per-iteration re-check are the actual authoritative source of
/// "an interrupt/append request landed" (DI-SA-9). This handler's only job is to exist for the
/// life of the run so the OS never falls back to terminating the process on receipt.
///
/// # Errors / fallback
///
/// If installing the signal listener itself fails (e.g. resource exhaustion), this degrades the
/// SAME way `spawn_control_watcher`'s own installation failure already degrades: the run
/// continues without the wake-up-signal fast path, relying purely on the poll-interval side of
/// `control::watch_control_inbox`'s `PollWatcher` and `run_inner`'s own per-iteration re-check —
/// never a hard failure of the run itself. In that one failure case, this function returns `None`
/// and the caller simply holds nothing (no guard needed: no handler was installed, so there is
/// nothing this crate did to make `SIGUSR2`'s default disposition worse than it already was
/// before this function was ever added).
#[cfg(unix)]
fn install_ignored_sigusr2_handler() -> Option<SigUsr2Guard> {
    let mut stream = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2()).ok()?;
    let handle = tokio::spawn(async move {
        loop {
            // `recv()` returning `None` means the underlying signal stream has been torn down
            // (process-wide signal-handling shutdown) — nothing further to drain in that case.
            if stream.recv().await.is_none() {
                return;
            }
        }
    });
    Some(SigUsr2Guard { handle })
}

/// RAII wrapper aborting the SIGUSR2-draining task on drop, mirroring
/// [`ControlWatcherHandle`]'s identical pattern immediately below.
#[cfg(unix)]
struct SigUsr2Guard {
    handle: tokio::task::JoinHandle<()>,
}

#[cfg(unix)]
impl Drop for SigUsr2Guard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// =================================================================================================
// spawn_control_watcher — background task forwarding control-inbox notifications
// =================================================================================================

/// Spawn a background task that installs [`control::watch_control_inbox`] and sets `interrupted`
/// whenever a notification arrives, for the duration of the returned [`ControlWatcherHandle`]
/// (dropping it stops the watch — the underlying `notify::PollWatcher` is dropped inside the
/// spawned task once the task itself is aborted, which happens automatically when
/// [`ControlWatcherHandle`] is dropped, since it wraps a [`tokio::task::JoinHandle`] with
/// `abort_on_drop`-equivalent semantics achieved via an explicit `Drop` impl below rather than
/// relying on any external crate).
///
/// # R-SA-082's two mechanisms, both present
///
/// This satisfies R-SA-082's "MUST watch its control inbox via both a filesystem-notification
/// mechanism and a fixed-interval poll fallback" via [`control::watch_control_inbox`]'s own
/// `notify::PollWatcher`-based implementation (that module's own doc comment explains why
/// `PollWatcher` IS simultaneously both halves: it does not depend on a native OS notification
/// backend being available, so there is no separate native-vs-poll branch to maintain here). The
/// mandatory synchronous startup check (the other half of R-SA-082) is performed by [`run`]
/// itself, BEFORE this function is called — never inside this function — matching
/// `control::check_control_inbox_now`'s own documented "caller MUST invoke this once before
/// installing any asynchronous watch" contract.
fn spawn_control_watcher(
    run_paths: RunPaths,
    interrupted: Arc<std::sync::atomic::AtomicBool>,
) -> ControlWatcherHandle {
    let handle = tokio::spawn(async move {
        let (watcher, mut rx) = match control::watch_control_inbox(&run_paths) {
            Ok(pair) => pair,
            Err(_) => {
                // R-SA-082's watch is best-effort defense in depth on top of `run_inner`'s own
                // per-iteration `control::list_pending_appends`/interrupt re-check — a watcher
                // that fails to install (e.g. EMFILE/ENOSPC-class resource exhaustion) does not
                // strand the run: the step loop still re-checks `interrupted`/pending appends on
                // every iteration regardless of whether this watcher is alive at all. This task
                // simply has nothing further to do.
                return;
            }
        };
        // Keep the watcher alive for the lifetime of this task (dropping it would stop the watch)
        // — held in this local binding rather than discarded.
        let _watcher = watcher;
        while rx.recv().await.is_some() {
            interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    ControlWatcherHandle { handle }
}

/// RAII wrapper aborting the spawned control-inbox watcher task on drop, so a caller ([`run`])
/// never needs to remember to clean it up explicitly — the watcher's only useful lifetime is the
/// duration of [`run`]'s own step loop.
struct ControlWatcherHandle {
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for ControlWatcherHandle {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// =================================================================================================
// finish_run — R-SA-077's write-ordering invariant, the ONE funnel every exit path routes through
// =================================================================================================

/// Compute the terminal [`RunState`], write the final `status.json`, THEN write the terminal
/// [`ResultFile`] — in that EXACT order, unconditionally, on every single call site (R-SA-077).
///
/// This is the sole function in this module that writes a run's TERMINAL records. Every call site
/// in [`run`] funnels through here rather than writing either file directly, which is what makes
/// the ordering invariant structural rather than a convention every future edit must remember to
/// preserve: adding a new early-return branch to [`run`] in the future still cannot skip this
/// ordering unless that branch also skips calling this function entirely (in which case NEITHER
/// file is written, which is a strictly safer failure mode than writing them out of order — a
/// caller polling this run id sees "still Queued/Running" rather than an observably-ahead-of-
/// itself `ResultFile` with no matching terminal `status.json`).
///
/// `error` is folded into every result's own `error` field (if `results` is non-empty) OR, if
/// `results` is empty (the run never got far enough to produce even one step outcome), into a
/// single synthesized placeholder [`SingleResult`] so [`ResultFile::results`] is never silently
/// empty for a run that reached a terminal Failed state — a downstream reader walking `results`
/// should always find at least one entry explaining what happened, mirroring
/// `reconcile.rs::synthesize_step_results`'s identical "never leave results empty" contract for
/// the stale-dead-reconciliation path (this is the runner's OWN, first-hand analogue of that same
/// contract, not a re-derivation of `reconcile.rs`'s logic).
///
/// # Double-invocation idempotency (a `finish_run`-level guard, not just `read_and_delete_config`'s)
///
/// Before writing anything, this function checks whether `run_paths.result` ALREADY exists on
/// disk. If it does, some earlier call — this same process's own [`run`] invocation, or (per this
/// module's documented double-invocation scenario) a wholly separate, later `run()` invocation
/// against the same `--config`/`run_paths` pair after the first has already reached a terminal
/// write — has already produced the authoritative terminal record for this run id, and this call
/// is a no-op: neither `status.json` nor the `ResultFile` is touched. Without this guard, a second
/// `run()` invocation whose `read_and_delete_config` call observes
/// [`ConfigConsumeOutcome::AlreadyConsumed`] (module docs: "degrades gracefully instead of
/// crashing") would still reach `finish_run` and — since the terminal-transition write below is
/// deliberately unconditional/guard-bypassing precisely so it can ALWAYS reach a terminal state —
/// silently overwrite a genuinely-completed run's `Complete`/`success: true` result with a
/// synthesized `Failed`/`success: false` one. R-SA-077 already establishes that `ResultFile`
/// presence is the single authoritative "truly done" signal for every OTHER reader in this
/// subsystem (`reconcile.rs`, `control.rs`); this guard applies that identical principle
/// reflexively to the runner's own terminal-write path, so "no panic on double-invocation" (this
/// module's literal contract) also means "no silent data corruption of an already-final result"
/// (the property that contract exists to protect in the first place).
async fn finish_run(
    run_paths: &RunPaths,
    mut status: RunStatus,
    terminal_state: RunState,
    mut results: Vec<SingleResult>,
    cwd: PathBuf,
    session_file: Option<PathBuf>,
    error: String,
) {
    if matches!(tokio::fs::try_exists(&run_paths.result).await, Ok(true)) {
        tracing::warn!(
            run_id = %status.run_id,
            "finish_run called again after a terminal ResultFile already exists on disk \
             (double-invocation of the runner); leaving the existing authoritative result \
             untouched rather than overwriting it"
        );
        return;
    }

    // Force the terminal transition directly (mirrors `reconcile.rs::synthesize_failure`'s own
    // rationale): `finish_run` must be able to reach ANY of Complete/Failed/Paused regardless of
    // which (possibly already-illegal-from-here) state `status` currently holds, since this is the
    // authoritative "this run is now over" write, not a normal forward-progress transition subject
    // to the ordinary transition guard.
    let now = super::now_epoch_millis_pub();
    status.state = terminal_state;
    status.last_update = now;
    status.ended_at = Some(now);

    if !error.is_empty() && results.is_empty() {
        results.push(SingleResult {
            agent: status
                .steps
                .first()
                .map(|s| s.agent.clone())
                .unwrap_or_else(|| status.run_id.as_str().to_string()),
            task: String::new(),
            exit_code: 1,
            usage: cyrup_core::Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: None,
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: terminal_state == RunState::Paused,
            timed_out: false,
            error: Some(error.clone()),
            tool_calls: Vec::new(),
            output_truncated: false,
        });
    }

    // `success` iff the run reached `Complete` AND every recorded result exited cleanly.
    // `Iterator::all` is vacuously `true` over an empty `results` list (a `Complete` run that
    // produced zero step results — e.g. a `Chain` run whose `steps` list was itself empty — is
    // treated as a success, matching this crate's general "no work attempted, no work failed"
    // convention rather than requiring a nonsensical "at least one result" precondition).
    let success =
        terminal_state == RunState::Complete && results.iter().all(|r| r.exit_code == 0);

    // R-SA-077: status.json THEN ResultFile, in that exact order. Both writes are best-effort at
    // the OUTER level (a failure writing `status.json` here still attempts the `ResultFile` write,
    // rather than leaving the run in an indefinite non-terminal state on disk merely because ONE
    // of the two writes hit a transient I/O error) — but the ORDER between the two calls is never
    // reordered, which is the actual invariant R-SA-077 requires; `write_atomic_json`'s own
    // temp-then-rename guarantee (R-SA-076) means a reader never observes a torn write of either
    // individual file, only ever "old status, no result" or "new status, no result yet" or "new
    // status, new result" — never "new result, old status", since the result write is issued
    // strictly after the status write is issued here.
    let status_write = write_atomic_json(&run_paths.status, &status).await;

    let result_file = ResultFile {
        id: status.run_id.clone(),
        run_id: status.run_id.clone(),
        agent: status
            .steps
            .first()
            .map(|s| s.agent.clone())
            .unwrap_or_else(|| status.run_id.as_str().to_string()),
        mode: status.mode,
        state: terminal_state,
        success,
        cwd,
        session_file,
        results,
    };
    let result_write = write_atomic_json(&run_paths.result, &result_file).await;

    if let Err(err) = status_write {
        tracing::warn!(
            run_id = %status.run_id,
            error = %err,
            "failed to write terminal status.json (R-SA-077); ResultFile write was still \
             attempted per this function's own best-effort-both-writes contract"
        );
    }
    if let Err(err) = result_write {
        tracing::warn!(
            run_id = %status.run_id,
            error = %err,
            "failed to write terminal ResultFile (R-SA-077)"
        );
    }
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
    use crate::background::atomic::write_atomic_json;
    use crate::spawn::chain_graph::SingleStepSpec;

    fn single_step(agent: &str, task: &str) -> SingleStepSpec {
        SingleStepSpec {
            agent: agent.to_string(),
            task: task.to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        }
    }

    // ---------------------------------------------------------------------------------------
    // read_and_delete_config: R-SA-073 delete-then-act idempotency
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn read_and_delete_config_consumes_and_removes_the_file() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        let config = RunnerConfig {
            run_id: RunId::from_token("run00001"),
            mode: RunMode::Single,
            steps: vec![RunnerStep::SingleStep(single_step("worker", "do it"))],
            cwd: dir.path().to_path_buf(),
            session_file: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
        };
        write_atomic_json(&cfg_path, &config).await.expect("write config");

        let outcome = read_and_delete_config(&cfg_path).await.expect("read succeeds");
        match outcome {
            ConfigConsumeOutcome::Consumed(read_back) => assert_eq!(read_back, config),
            ConfigConsumeOutcome::AlreadyConsumed => panic!("expected Consumed"),
        }

        assert!(
            !tokio::fs::try_exists(&cfg_path).await.expect("check exists"),
            "the config file must be deleted immediately after being read (R-SA-073)"
        );
    }

    #[tokio::test]
    async fn read_and_delete_config_double_consume_does_not_panic() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        let config = RunnerConfig {
            run_id: RunId::from_token("run00002"),
            mode: RunMode::Single,
            steps: vec![],
            cwd: dir.path().to_path_buf(),
            session_file: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
        };
        write_atomic_json(&cfg_path, &config).await.expect("write config");

        let first = read_and_delete_config(&cfg_path).await.expect("first read succeeds");
        assert!(matches!(first, ConfigConsumeOutcome::Consumed(_)));

        // The load-bearing idempotency proof this task calls for: a SECOND consume against the
        // now-deleted path must not panic, must not error, and must report AlreadyConsumed.
        let second = read_and_delete_config(&cfg_path).await.expect("second read does not error");
        assert!(
            matches!(second, ConfigConsumeOutcome::AlreadyConsumed),
            "a double-consume of the handoff config must degrade to AlreadyConsumed, never panic \
             or re-process: {second:?}"
        );
    }

    #[tokio::test]
    async fn read_and_delete_config_malformed_json_surfaces_as_error_not_already_consumed() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let cfg_path = dir.path().join("runner-config.json");
        tokio::fs::write(&cfg_path, b"not valid json").await.expect("write garbage");

        let result = read_and_delete_config(&cfg_path).await;
        assert!(
            result.is_err(),
            "a malformed (but PRESENT) config file must surface as a genuine error, distinct \
             from the file simply being absent"
        );
    }

    // ---------------------------------------------------------------------------------------
    // run(): full run through the scripted fixture — status-then-result ordering (happy path,
    // forced-error path, missing-config path) and R-SA-096 disk-re-scan append consumption.
    //
    // These live in `tests/background_runner_main_integration.rs`, NOT here, for the identical
    // reason `spawn_detached.rs`'s own module docs give for `spawn_detached_runner`'s fixture-
    // backed proof: `CARGO_BIN_EXE_cyrup-subagent-fixture` is only defined for ordinary Cargo
    // integration tests (files under `tests/`), never for a library's own `#[cfg(test)]` unit
    // tests compiled into `src/`, so `env!("CARGO_BIN_EXE_cyrup-subagent-fixture")` cannot resolve
    // in this module at all. Separately (and independently sufficient on its own), those tests
    // must mutate `CYRUP_SUBAGENT_BINARY`/`CYRUP_SUBAGENT_FIXTURE_SCRIPT` via `unsafe { std::env::
    // set_var/remove_var }` (Rust 2024 requires `unsafe` for either), which this crate's own
    // `#![forbid(unsafe_code)]` (`src/lib.rs`) blocks even inside a `#[cfg(test)]` module — a
    // `tests/*.rs` file is its own separate compilation unit, not subject to the library crate's
    // `forbid` attribute, exactly like `tests/background_spawn_detached_integration.rs`'s own
    // established precedent for the identical constraint.
    // ---------------------------------------------------------------------------------------

    // ---------------------------------------------------------------------------------------
    // finish_run: double-invocation idempotency — a second terminal write against a run id that
    // ALREADY has an authoritative ResultFile on disk must be a no-op, never an overwrite. This is
    // the `finish_run`-level half of this module's double-invocation contract (the
    // `read_and_delete_config`-level half is proven above); together they cover the full `run()`
    // double-invocation scenario without needing the fixture binary, since `finish_run` is called
    // directly here rather than driving the whole `run_inner` step loop.
    // ---------------------------------------------------------------------------------------

    fn run_paths_in(dir: &std::path::Path, run_id: &RunId) -> RunPaths {
        let async_root = dir.join("async");
        let results_dir = dir.join("results");
        RunPaths::for_run(&async_root, &results_dir, run_id)
    }

    // ---------------------------------------------------------------------------------------
    // Second-pass adversarial-review regression: `run()`'s control-inbox-directory creation step
    // (between the initial status.json write and `run_inner`) must route ANY failure through
    // `finish_run` exactly like every other pre-loop fallible step, never bypass it via a bare
    // `?`. This is provable WITHOUT the fixture binary (never reaches `run_inner`/subprocess
    // dispatch at all): pre-creating a plain FILE at the exact path `run()` needs to
    // `create_dir_all` as a directory forces that call to fail deterministically on every
    // platform (`ENOTDIR`/`AlreadyExists`-as-non-directory), with no timing dependency.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn control_inbox_dir_creation_failure_still_reaches_a_terminal_failed_state_via_finish_run(
    ) {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-badcontrol");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        // `run_paths.control_inbox` is `<run_dir>/control/interrupt.json`, so its parent is
        // `<run_dir>/control`. Pre-create a plain FILE at exactly that path so `run()`'s own
        // `tokio::fs::create_dir_all(.../control)` call is guaranteed to fail.
        tokio::fs::write(run_paths.run_dir.join("control"), b"not a directory")
            .await
            .expect("pre-create a blocking file where the control dir needs to go");

        let config = RunnerConfig {
            run_id: run_id.clone(),
            mode: RunMode::Single,
            steps: vec![RunnerStep::SingleStep(single_step("worker", "do it"))],
            cwd: dir.path().to_path_buf(),
            session_file: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
        };
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &config).await.expect("write config");

        let outcome = run(&cfg_path, &run_paths).await;
        assert!(
            outcome.is_ok(),
            "run() itself never returns Err to its own caller, even when the control-inbox \
             directory cannot be created: {outcome:?}"
        );

        let status_bytes = tokio::fs::read(&run_paths.status).await.expect(
            "status.json must exist and be terminal — a bare `?` bypassing finish_run would \
             leave it permanently stuck at the initial Running record written earlier in run()",
        );
        let status: RunStatus = serde_json::from_slice(&status_bytes).expect("valid JSON");
        assert_eq!(
            status.state,
            RunState::Failed,
            "the control-inbox-directory-creation failure must reach a terminal Failed status \
             via finish_run, not leave the run stuck Running forever: {status:?}"
        );

        let result_bytes = tokio::fs::read(&run_paths.result).await.expect(
            "ResultFile must exist too — finish_run's own status-then-result ordering must still \
             hold on this exit path, not skip both writes entirely",
        );
        let result_file: ResultFile = serde_json::from_slice(&result_bytes).expect("valid JSON");
        assert_eq!(result_file.state, RunState::Failed);
        assert!(!result_file.success);
    }

    #[tokio::test]
    async fn finish_run_second_call_after_terminal_result_exists_does_not_overwrite_it() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-double-invoke");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(111));
        status.advance_state(RunState::Running).expect("Queued -> Running");

        // First call: a genuine successful completion.
        finish_run(
            &run_paths,
            status.clone(),
            RunState::Complete,
            vec![SingleResult {
                agent: "researcher".to_string(),
                task: "do the thing".to_string(),
                exit_code: 0,
                usage: cyrup_core::Usage::default(),
                model: None,
                attempted_models: Vec::new(),
                model_attempts: Vec::new(),
                final_output: Some("done".to_string()),
                structured_output: None,
                acceptance: None,
                detached: false,
                interrupted: false,
                timed_out: false,
                error: None,
                tool_calls: Vec::new(),
                output_truncated: false,
            }],
            dir.path().to_path_buf(),
            None,
            String::new(),
        )
        .await;

        let first_result_bytes = tokio::fs::read(&run_paths.result)
            .await
            .expect("ResultFile exists after the first finish_run call");
        let first_result: ResultFile =
            serde_json::from_slice(&first_result_bytes).expect("valid JSON");
        assert_eq!(first_result.state, RunState::Complete);
        assert!(first_result.success, "first call recorded a genuine success");

        let first_status_bytes = tokio::fs::read(&run_paths.status)
            .await
            .expect("status.json exists after the first finish_run call");

        // Second call: simulates a double-invocation of `run()` against the same config/run_paths
        // (e.g. `read_and_delete_config` observing `AlreadyConsumed` and `run`'s own tail routing
        // that outcome to `finish_run` with a freshly synthesized `Failed` status, exactly as
        // `run`'s `AlreadyConsumed` match arm does). This must NOT clobber the already-terminal,
        // already-successful result with a spurious failure.
        let second_status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(222));
        finish_run(
            &run_paths,
            second_status,
            RunState::Failed,
            Vec::new(),
            PathBuf::new(),
            None,
            "runner-config.json was already consumed".to_string(),
        )
        .await;

        let result_bytes_after_second_call = tokio::fs::read(&run_paths.result)
            .await
            .expect("ResultFile still exists after the second finish_run call");
        let result_after_second_call: ResultFile =
            serde_json::from_slice(&result_bytes_after_second_call).expect("valid JSON");
        assert_eq!(
            result_after_second_call, first_result,
            "a second finish_run call against a run id with an already-terminal ResultFile must \
             leave it byte-for-byte untouched, never overwrite a genuine success with a \
             synthesized double-invocation failure"
        );

        let status_bytes_after_second_call = tokio::fs::read(&run_paths.status)
            .await
            .expect("status.json still exists after the second finish_run call");
        assert_eq!(
            status_bytes_after_second_call, first_status_bytes,
            "status.json must likewise be left untouched by a no-op double-invocation finish_run call"
        );
    }

    #[tokio::test]
    async fn finish_run_first_call_writes_normally_when_no_result_file_exists_yet() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-first-call");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let status = RunStatus::queued(run_id, RunMode::Single, Some(1));

        finish_run(
            &run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            dir.path().to_path_buf(),
            None,
            "boom".to_string(),
        )
        .await;

        assert!(
            tokio::fs::try_exists(&run_paths.result).await.expect("check exists"),
            "the double-invocation guard must not block a genuine FIRST terminal write"
        );
        let result: ResultFile = serde_json::from_slice(
            &tokio::fs::read(&run_paths.result).await.expect("read result"),
        )
        .expect("valid JSON");
        assert_eq!(result.state, RunState::Failed);
        assert!(!result.success);
    }
}
