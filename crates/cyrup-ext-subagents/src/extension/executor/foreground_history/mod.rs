//! Foreground-run history (WORKFLOW_7 SUBTASK2): the settled-run record this process remembers
//! in memory, the on-disk round-trip that survives a restart, and the STRICT session-scoped
//! restore that seeds `FleetState::foreground_runs` (§3.3) at session start.
//!
//! Port of [`runs/foreground/foreground-history.ts`
//! (`@57278d82`)](../../../../../../workspace/pi-subagents/src/runs/foreground/foreground-history.ts)
//! (persist/restore) plus the settle-path slice of `subagent-executor.ts`'s `rememberForegroundRun`
//! this crate's ONE foreground settle path (`foreground.rs::run_foreground_impl`) actually needs
//! (record/remember).
//!
//! # File layout — one file, one concern (same split as `background::terminal_run_index`)
//!
//! ```text
//! mod.rs      facade + this doc; no logic
//! record.rs   the on-disk/in-memory shape (`ForegroundHistoryRun`/`Child`, `HistoryVersion`, the
//!             byte-bounded UTF-8 tail) + the in-memory "remember" producer and its eviction sweep
//! persist.rs  the on-disk round-trip: the envelope, `read_index`, `sort_and_bound`, and the
//!             merge-by-id writer
//! restore.rs  the STRICT session-scoped reader that seeds the in-memory map at session start
//! ```
//!
//! Declared alongside `foreground_control`/`workflow_controllers` in `executor/mod.rs`'s module
//! block (same private-module facade `background/run_history.rs` and `background/runner_main/`
//! use): every item another `executor::*` submodule needs is re-exported here, so consumer paths
//! read `crate::extension::executor::foreground_history::X`.
//!
//! # The in-memory map is a SUPERSET of the persisted file
//!
//! [`SubagentExecutor::remember_foreground_run`](crate::extension::executor::SubagentExecutor::remember_foreground_run)
//! (`record.rs`) writes **every** settled run into the in-memory `foreground_runs` map, whatever
//! its children's statuses — including `detached`.
//! The persist writers in `persist.rs`
//! ([`persist::persist_foreground_run_history_from`] and its roots-resolving sibling
//! [`persist::persist_foreground_run_history_in`]) then drop any run that is not fully **restorable** ([`record::RESTORABLE`] —
//! `completed | failed | paused | stopped`) before it ever reaches disk. So: settle → in-memory map
//! (all statuses) → persist filters to the restorable subset → file. A `detached` run is visible
//! to the fleet for the life of the process and gone after a restart. That is upstream's
//! behaviour, and it is deliberate.
//!
//! # Two persist entry points, one rule
//!
//! Every persist writer is `&self`-free and takes the foreground-runs MAP:
//! [`persist::persist_foreground_run_history_from`] (a resolved `results_dir` and a bound) and
//! [`persist::persist_foreground_run_history_in`] (a [`crate::paths::Roots`] and a `cwd`, which it
//! resolves through the same `default_results_dir_in` arithmetic every other reader uses). The one
//! `&self` wrapper left, `SubagentExecutor::persist_foreground_run_history_for`, reads `roots` off
//! the config snapshot and delegates to the second.
//!
//! They take the map because `foreground.rs`'s detached continuation does: that task owns a child
//! which outlived its tool call and captures only the map's `Arc` — never the executor, so a
//! detached child cannot pin a shutdown alive (R-VLS11b-02). One rule, therefore, for both the
//! ordinary settle and the detached one: the same merge-by-id, the same [`record::RESTORABLE`]
//! filter, the same bound. There is no second writer that could forget the never-persist rule.

// `pub(crate)` for exactly one reason, and it is the only module here that is: `foreground.rs`'s
// DETACHED CONTINUATION (R-VLS11b-02) calls
// [`persist::persist_foreground_run_history_in`] directly. That task is a SIBLING of this facade,
// not a descendant, so a private `mod persist;` is invisible to it — and it cannot go through
// [`SubagentExecutor`]'s wrapper because it holds the foreground-runs `Arc` and deliberately not
// the executor. A `pub(crate) use` re-export at this facade would be an unused import until that
// call site lands; the module is the honest seam.
pub(crate) mod persist;
mod record;
mod restore;

// Only `ForegroundHistoryRun` is re-exported here: it is `executor/mod.rs`'s `foreground_runs`
// field type. Everything else (`MAX_REMEMBERED_FOREGROUND_RUNS`, `HistoryVersion`, `RESTORABLE`,
// `bounded_tail`, `read_index`/`sort_and_bound`, the restore functions) is consumed only by
// sibling submodules within this facade via `super::record`/`super::persist`, which their
// shared-private-module visibility already permits — re-exporting them here too would be an
// unused `pub(crate) use`.
pub(crate) use record::ForegroundHistoryRun;
