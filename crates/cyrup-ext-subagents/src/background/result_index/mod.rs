//! Session-partitioned result index — the on-disk layout that makes concurrent cyrup instances
//! in one directory correct.
//!
//! Ports pi `runs/background/result-files.ts` (519 LOC).
//!
//! # The problem this solves
//!
//! `<results_dir>` is `<temp_root>/results/<cwd_key>` (`background/artifact_roots.rs:281-284`) —
//! keyed by **cwd**, never by session. Every cyrup instance running in a directory resolves the
//! identical path, so a flat `read_dir` of it returns every instance's results, and a consumer
//! that deletes what it reads destroys other instances' work.
//!
//! Filtering after enumeration is not the fix and is not what upstream does: pi has no
//! `readdirSync(resultsDir)` in its watcher at all. Results are **partitioned on disk** and each
//! instance enumerates only its own partition.
//!
//! # Layout
//!
//! ```text
//! <results_dir>/
//!   <runId>.json                                        LEGACY payload — read, never written
//!   result-pending/<enc(sessionId)>/<enc(runId)>.json    staged, written FIRST
//!   result-owned/<enc(sessionId)>/<enc(runId)>.json      promoted payload — the write target
//!   result-index/
//!     sessions/<enc(sessionId)>/<enc(runId)>.json        the session partition
//!     runs/<enc(runId)>.json                             lookup by run id
//!     observers/mission/<enc(runId)>.json                cross-session mission reconciliation
//!     tool-calls/<enc(toolCallId)>/<enc(runId)>.json     lookup by tool call
//! ```
//!
//! **[CYRUP-DELTA] the promoted payload is session-partitioned**, where upstream publishes to the
//! results ROOT (`result-files.ts:52-54`). The root is keyed by cwd and shared by every cyrup
//! instance in a directory, including ones running builds that predate this index and enumerate it
//! by `readdir`; nineteen completed runs' results were consumed that way on one machine in a day.
//! Promoting into `result-owned/<enc(sessionId)>/` makes that theft impossible rather than
//! unlikely. Payloads at the legacy root are still resolved and consumed by their owner, so runs
//! written by an older build stay deliverable; [`resolve_payload`] is the ladder that reaches all
//! three locations, and it is the ONLY way a reader should address a payload.
//!
//! `<enc(...)>` is [`crate::identity::IndexSegment`] — a session id is routinely a full `.jsonl`
//! path, so it is URI-encoded and, when that is over-long or non-portable, hashed.
//!
//! # Write protocol
//!
//! [`write_async_result_file`] performs pi's `writeAsyncResultFile` (`:178-183`) in order:
//!
//! 1. **stage** the payload session-private under `result-pending/`,
//! 2. **index** it (session, then the isolated run/tool-call/observer accelerators),
//! 3. **promote** it with an atomic `rename` to the public path.
//!
//! The invariant that falls out: *a payload is never publicly visible before its index exists.* A
//! reader can therefore never see a result it cannot attribute, and the enumerator can never miss
//! a result that is visible. A payload with no [`crate::identity::SessionId`] is refused outright
//! (pi throws at `:166`) — it could not be indexed, so it could only ever become garbage.
//!
//! # Read protocol
//!
//! [`enumerate`]'s candidate sources correspond 1:1 with pi's `indexedResultCandidates`
//! (`result-watcher.ts:634-644`): the current session's partition plus any claimed predecessors,
//! tracked run ids, the mission observer band, and explicitly observed ids. All index- or
//! state-driven; none is a directory scan of the results root.
//!
//! Reads fan out over [`crate::identity::IndexSegment`] aliases while writes use a single key —
//! that asymmetry is what makes an encoding change backward-compatible.
//!
//! # The one deliberate cross-session path
//!
//! `observers/mission/` is read by every instance regardless of ownership. Mission reconciliation
//! has to see a completion whichever instance launched it, and serving that from a narrow,
//! explicitly-populated index is what makes it possible **without** widening the ownership
//! predicate to every result in the directory. It is written only for runs that actually carry a
//! mission binding ([`write::write_result_index_for_data`]).
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs          facade + layout diagram; no logic
//! entry.rs        the index record — ResultIndexEntry, IndexVersion
//! paths.rs        the on-disk layout — every path builder, IndexSegment-only, I/O-free
//! errno.rs        error policy — absent / access-denied / ignorable
//! exists.rs       payload existence probing under that policy
//! write.rs        the write protocol — stage → index → promote, ResultWrite
//! promote.rs      publishing a staged payload — the rename race, PromotionState
//! locate.rs       finding a payload — ResultPayloadLocation, PayloadState
//! enumerate.rs    listing candidates — by session, tool call, mission band
//! remove.rs       clearing one run's index entries
//! retention.rs    expiring orphaned entries — the 24 h sweep
//! ```

mod entry;
mod enumerate;
// `pub(crate)`, not private: WORKFLOW_3's `workflows::receipt` module (outside this subtree)
// needs `errno::is_absent` through the same door WORKFLOW_4's `wait_completions` will reuse for
// `is_access_denied` — one declaration, made once, here.
pub(crate) mod errno;
mod exists;
mod locate;
mod paths;
mod promote;
mod remove;
mod retention;
mod write;

pub use entry::{IndexVersion, ResultIndexEntry};
pub use enumerate::{
    ResultCandidate, mission_observer_result_candidate_files, result_candidate_files_for_session,
    result_candidate_files_for_tool_call, result_candidates_for_session, result_files_for_session,
};
pub use exists::indexed_result_exists;
pub use locate::{
    ConsumablePayload, PayloadResolution, PayloadState, ResultPayloadLocation,
    fallback_result_payload_path_for_session_run, owned_payload_path, resolve_payload,
    result_payload_path_for_indexed_run, result_payload_path_for_mission_observer_run,
    result_payload_path_for_session_run,
};
pub use promote::PromotionState;
pub use remove::{remove_mission_observer_index, remove_result_index};
pub use retention::{DEFAULT_MAX_AGE_MS, cleanup_result_indexes};
pub use write::{ResultWrite, write_async_result_file, write_pending_async_result_file};
