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
//! [`SubagentExecutor::persist_foreground_run_history`](crate::extension::executor::SubagentExecutor::persist_foreground_run_history)
//! (`persist.rs`) then drops any run that is not fully **restorable** ([`record::RESTORABLE`] —
//! `completed | failed | paused | stopped`) before it ever reaches disk. So: settle → in-memory map
//! (all statuses) → persist filters to the restorable subset → file. A `detached` run is visible
//! to the fleet for the life of the process and gone after a restart. That is upstream's
//! behaviour, and it is deliberate.

mod persist;
mod record;
mod restore;

// Only `ForegroundHistoryRun` crosses this facade's own boundary (`executor/mod.rs`'s
// `foreground_runs` field type); every other item (`MAX_REMEMBERED_FOREGROUND_RUNS`,
// `HistoryVersion`, `RESTORABLE`, `bounded_tail`, the persist/restore functions themselves) is
// consumed only by sibling submodules within this facade via `super::record`/`super::persist`,
// which their shared-private-module visibility already permits — re-exporting them here too
// would be an unused `pub(crate) use`.
pub(crate) use record::ForegroundHistoryRun;
