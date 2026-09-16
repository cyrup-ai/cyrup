//! SCOPE_9 — the per-SESSION active-async capacity pool.
//!
//! Ports pi `runs/background/active-async-capacity.ts` (516 LOC @`v0.66.0`; the design is
//! byte-identical at `v0.68.0` apart from the one additive guard folded into
//! [`inspect::workflow_release_verdict`]).
//!
//! # The problem this solves
//!
//! cyrup's existing concurrency knob, `SubagentExtensionConfig::global_concurrency_limit`, is
//! PROCESS-wide: it bounds concurrent child processes inside one runner. Nothing bounds how many
//! top-level async runs one orchestrator SESSION may have in flight, and because every cyrup
//! instance in a working directory shares one async root
//! ([`crate::background::run_artifact_roots`]), one instance's fan-out starves every other
//! instance sharing the directory. This module gives each session its own durable pool of slots,
//! so the cap partitions instead of competing.
//!
//! # Layout
//!
//! ```text
//! <run scratch>/session-active-async-capacity/     ← the ONE session-keyed root in this crate
//!   <enc(sessionId)>/                              one pool per session
//!     slot-0/
//!       owner.json                                 ActiveAsyncCapacityOwner, 0600
//!       capacity.claim                             the O_EXCL lockfile, while held
//!     slot-1/…
//! ```
//!
//! The root deliberately does NOT carry a [`cwd_key`](crate::background::run_artifact_roots)
//! level — see [`crate::background::active_async_capacity_root_in`], which states the two
//! consequences (two instances in one cwd get two pools; one session across two cwds gets one).
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs      facade + the three findings below; no logic
//! config.rs   the thresholds, the two resolvers, `validate_capacity_config`, `CapacityOptions`
//! key.rs      the record AND its address: owner, snapshot, pool/slot paths, parse/read/list
//! claim.rs    the O_EXCL claim, slot create/remove, the handle, `acquire`, `transfer`
//! inspect.rs  the four release verdicts and `inspect_active_async_capacity_owner`
//! sweep.rs    `reconcile_active_async_capacity` + the snapshot readers
//! ```
//!
//! # §D1 — there is no clamping, anywhere
//!
//! `MIN_`/`MAX_ABANDONED_SLOT_RELEASE_AFTER_MS` are **validation bounds**, not clamps:
//! `resolveAbandonedSlotReleaseAfterMs` returns an in-range value verbatim and substitutes the
//! default only for garbage, while `validateCapacityConfig` (`extension/config.ts:86-98`) THROWS
//! on an out-of-range value. A clamping port would silently accept `1` and run a five-minute
//! reclamation policy where upstream refuses to start. See
//! [`config::resolve_abandoned_slot_release`].
//!
//! # §D2 — nothing "releases on terminal"; release is reconciliation
//!
//! There is no `release()` on the handle. A slot is removed only by
//! [`reconcile_active_async_capacity`], which every [`acquire`] runs first, and only against a
//! verdict. A terminal run's slot is therefore freed by the NEXT spawn attempt in that session, or
//! by an explicit reconcile/snapshot read. See [`sweep`]'s own module doc.
//!
//! # §D3 — the release verdict could not be ported as written
//!
//! Upstream releases a runner's slot on one positive proof: a `processTerminal` artifact whose
//! `state === "observed"` matches the owner's `runnerProcessInstanceId`. **cyrup has neither
//! input** — `runs/background/process-terminal.ts` has no port, and nothing in this crate mints a
//! `runnerProcessInstanceId`. Ported verbatim, a run that finishes SUCCESSFULLY would have no
//! proof and is not `failed`, so its verdict would be `retained` forever: after `limit` successful
//! background runs the session could never spawn again, which is strictly worse than having no cap
//! at all.
//!
//! The substitute is cyrup's own start-proof, the runner **pid** — real, already recorded, and
//! exactly the value [`crate::background::reconcile::check_pid_liveness`] consumes. The full rung
//! table, and the two upstream rungs that are unrepresentable and dropped, are on
//! [`inspect::runner_release_verdict`].
//!
//! # No `allow(dead_code)` lives in this module, and that is checkable
//!
//! [`key::ActiveAsyncCapacityKind::Workflow`] has no cyrup acquire site (cyrup's workflow router
//! refuses the async shape), but it is reachable through `serde` from a slot written by another
//! build, and [`inspect::workflow_release_verdict`] runs against it on every reconcile — so
//! nothing here needs to be suppressed to compile clean.

pub mod claim;
pub mod config;
pub mod inspect;
pub mod key;
pub mod sweep;

pub use claim::{AcquireInput, ActiveAsyncCapacityHandle, TransferInput, acquire, transfer};
pub use config::{
    CapacityOptions, DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS, MAX_ABANDONED_SLOT_RELEASE_AFTER_MS,
    MIN_ABANDONED_SLOT_RELEASE_AFTER_MS, resolve_abandoned_slot_release,
    resolve_max_active_async_runs_per_session, validate_capacity_config,
};
pub use inspect::{
    ActiveAsyncCapacityInspection, ActiveAsyncCapacityReleaseEvidence,
    ActiveAsyncCapacityReleaseVerdict, CapacityRelation, abandoned_runner_release_verdict,
    inspect_active_async_capacity_owner, owner_release_verdict, runner_release_verdict,
    workflow_release_verdict,
};
pub use key::{
    ActiveAsyncCapacityKind, ActiveAsyncCapacityOwner, ActiveAsyncCapacitySnapshot,
    CapacityOwnerVersion, parse_owner, read_owner, session_pool_dir, slot_dir,
};
pub use sweep::{
    get_active_async_capacity_snapshot, reconcile_active_async_capacity, snapshot_for,
};

#[cfg(test)]
mod tests;
