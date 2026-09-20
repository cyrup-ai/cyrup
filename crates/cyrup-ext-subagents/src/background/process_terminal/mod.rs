//! The process-terminal proof — pi `src/runs/background/process-terminal.ts` (310 lines)
//! @v0.68.0.
//!
//! # The question this artifact answers
//!
//! `status.json` can say a run is `failed`. It cannot say whether the RUNNER PROCESS that owned
//! the run is gone. Those are different facts, and every consumer that has to decide "may I hand
//! this run's resources to someone else?" needs the second one: the active-async capacity pool
//! releases a slot on it ([`crate::background::active_async_capacity`]), the active-run index
//! drops a marker on it, and an agent that delegated work reads it to tell a crash from a slow
//! start. Before this module, cyrup answered that question with `kill(pid, 0)` alone, which a
//! RECYCLED pid answers wrongly and forever.
//!
//! # Three artifacts, written by two processes, in one fixed order
//!
//! 1. **The candidate** (`<run_dir>/process-terminal-candidate.json`, 0600) — what the runner
//!    declares it will have to prove: its run id, its
//!    [`RunnerProcessInstanceId`], one writer-exit ledger per step, the session file it held and
//!    the revival-lease token it took. Written PRIVATE
//!    ([`write_private_atomic_json`](crate::background::atomic::write_private_atomic_json))
//!    because it can carry a session path (pi `:114-116`).
//! 2. **The proof** (`<run_dir>/process-terminal.json`) — the answer. Written twice: once at
//!    launch as `state: "pending"` by [`initialize_process_terminal`], and once at the runner's
//!    close by [`finalize_process_terminal`].
//! 3. **The overlay** — the same proof copied onto `status.processTerminal` and each
//!    `status.steps[i].processTerminal`, so a status reader needs no second file
//!    (pi `overlayStatus`, `:224-241`).
//!
//! # The ladder, in upstream's order (`:243-310`) — every rung is a distinguishable answer
//!
//! | rung | condition | result |
//! |---|---|---|
//! | `:248-252` | an `observed` proof for this run+instance, or ANY `unknown` proof, already on disk | returned verbatim, no second event |
//! | `:258` | no candidate | `unknown` / [`ProcessTerminalReason::RunnerCandidateMissing`] |
//! | `:259` | the candidate names another run or another runner instance | [`ProcessTerminalReason::RunnerInstanceMismatch`] |
//! | `:273-274` | the canonical session lease still CLAIMS the file — `owned` by an owner that is not [`demonstrably_stale`](crate::background::session_lease::demonstrably_stale), or `unreadable` | [`ProcessTerminalReason::CanonicalSessionLeaseActive`] / [`ProcessTerminalReason::CanonicalSessionUnavailable`] |
//! | `:275-276` | a revival lease token is recorded but its release was never acknowledged | [`ProcessTerminalReason::CanonicalSessionReleaseUnverified`] |
//! | `:277` | declared and recorded writers disagree, or BOTH are empty | [`ProcessTerminalReason::WriterCloseUnverified`] |
//! | `:279` | some writer's process TREE was not observed torn down | [`ProcessTerminalReason::ProcessTreeUnverified`] |
//! | `:284` | otherwise | `observed`, `instances: [runner, ...writers]` |
//!
//! The `:277` rung is also the **crash discriminator**, and it is load-bearing:
//! [`initialize_process_terminal`] writes `writers: {}` and NO `expectedWriters`, so a runner that
//! died before it reached its own candidate write leaves a candidate whose two maps are both
//! empty — `writer-close-unverified`. A runner that finished writes one entry per step, so the
//! maps are non-empty and the ladder can reach `observed`.
//!
//! **Do not "simplify" that guard into a bare `all_writers.is_empty()`.** It reads natural and it
//! matches a skim of upstream's `allWriters.length === 0`, and it is wrong for exactly the runs
//! whose writer LEDGER is legitimately empty while their DECLARATION is not: a step that launched
//! no child (`expectedWriters["0"] = 0`, `writers["0"] = []` — upstream's own candidate shape
//! verbatim, and what cyrup writes for a step whose agent binary does not exist or whose run was
//! skipped). Every such run would close `writer-close-unverified`: `debug.run` would say so for a
//! clean close, the capacity slot would fall through to the pid ladder, and the active-run marker
//! would be held for 24 hours. `finalize_reaches_observed_over_the_upstream_empty_writer_shape`
//! (`tests.rs`) is the only thing in the suite that notices — the mutation was GREEN across all
//! 4,295 tests before it existed.
//!
//! # Durability comes before the event (`:299-309`)
//!
//! [`finalize_process_terminal`] writes the sidecar FIRST. Only if that write succeeded does it
//! release the active-run index, overlay the status and append
//! `subagent.run.process_terminal` to `events.jsonl`. A non-durable write emits NO event and
//! returns an `unknown` proof, so a consumer never sees a lifecycle event announcing a proof that
//! no file can confirm.
//!
//! # `[CYRUP-DELTA]` — cyrup's writers are REAL, where upstream's are structurally empty
//!
//! pi `subagent-runner.ts:5168-5190` writes `writers[i] = []` and `expectedWriters[i] = 0` for
//! every step, unconditionally, and its own comment at `:5169` states why: *"Children run inside
//! this process, so no step has writer processes to prove terminal."* That premise is FALSE for
//! cyrup. Every step of a cyrup run spawns a real OS child in its own process group
//! ([`crate::spawn::SpawnedChild`], `spawn/mod.rs:795` `command.process_group(0)`), with a real
//! exit status, a real POSIX signal name ([`crate::spawn::signal::signal_name_of`],
//! `spawn/signal.rs:162-170`) and a real process-GROUP teardown that can be verified
//! (`spawn/signal.rs:476-514`). So cyrup emits one
//! [`ProcessInstanceExit::PiWriter`] per launched child, carrying a
//! [`ProcessTreeTerminal`] that says whether that child's whole group was observed gone — which
//! makes the `:279` `process-tree-unverified` rung and the `:277` inconsistency rung LIVE
//! verdicts about real processes rather than arms that upstream's own empty shape can never
//! reach. Reproducing upstream's emptiness here would be a hollow proof: it would report
//! `observed` while having verified nothing at all about the children that actually wrote the
//! session.

mod candidate;
mod error;
mod finalize;
mod id;
mod proof;
mod types;

#[cfg(test)]
mod tests;

pub use candidate::{
    InstanceKind, initialize_process_terminal, mark_process_terminal_candidate_lease_release,
    read_process_terminal_candidate, valid_process_instance, write_process_terminal_candidate,
};
pub use finalize::{
    LeaseClaimProbe, finalize_process_terminal, finalize_process_terminal_with, overlay_status,
    resume_disposition,
};
pub use id::RunnerProcessInstanceId;
pub use proof::{
    ProofExpectation, read_process_terminal, sanitize_process_terminal, unknown_proof,
    validate_proof,
};
pub(crate) use proof::{deserialize_overlay, deserialize_step_overlay};
pub use types::{
    CanonicalSessionTerminal, LeaseDisposition, ProcessInstanceExit, ProcessTerminal,
    ProcessTerminalBase, ProcessTerminalCandidate, ProcessTerminalReason, ProcessTerminalState,
    ProcessTerminalVersion, ProcessTreeTerminal, ProcessTreeUnknownReason, ResumeDisposition,
    RunnerCloseObservation, WriterProcessLedger,
};

pub use error::ProcessTerminalError;

/// pi `SUBAGENT_LIFECYCLE_ARTIFACT_VERSION` (`shared/types.ts:629`) — the schema generation every
/// lifecycle event line stamps, and the one the `subagent.run.process_terminal` event carries
/// (`process-terminal.ts:305`).
///
/// A consumer tailing `events.jsonl` across a cyrup upgrade uses it to tell "this build writes a
/// shape I know" from "this build writes a shape I must ignore". Upstream's value at v0.68.0 is
/// `3`; it is NOT this crate's own versioning knob and must only ever move when upstream's does.
pub const SUBAGENT_LIFECYCLE_ARTIFACT_VERSION: u32 = 3;

/// The `events.jsonl` event type [`finalize_process_terminal`] appends on a durable proof write —
/// pi `{ type: "subagent.run.process_terminal", … }` (`process-terminal.ts:305`), spelled once.
pub const PROCESS_TERMINAL_EVENT_TYPE: &str = "subagent.run.process_terminal";
