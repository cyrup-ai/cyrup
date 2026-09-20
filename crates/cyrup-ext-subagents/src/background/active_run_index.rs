//! The active-run index — the write-side twin of
//! [`crate::background::terminal_run_index`], and the single function every status writer in this
//! crate calls instead of `update_terminal_run_index` directly.
//!
//! Ports pi `runs/background/active-run-index.ts` (139 LOC @`v0.66.0`, 155 @`v0.68.0`) in full.
//!
//! # The problem this solves
//!
//! `<async_root>` is per-**cwd** and shared by every cyrup instance in the directory. Answering
//! "which runs are in flight right now" from that root means `read_dir`-ing every run ever
//! launched there and reading each run's `status.json` — the same full scan
//! [`crate::background::terminal_run_index`] exists to replace for the terminal half. This index
//! makes the active half a bounded listing of one directory.
//!
//! # Layout
//!
//! ```text
//! <async_root>/
//!   <runId>/…                              the run directories (unchanged)
//!   .active-runs/                          THIS index — a dot-prefixed SIBLING of the runs
//!     <runId>                              one empty marker file per QUEUED-or-RUNNING run
//!     tool-calls/
//!       <enc(toolCallId)>/
//!         <runId>                          optional alias, best-effort
//!   .terminal-runs/…                       the terminal index this module delegates to
//! ```
//!
//! `.active-runs` was ALREADY reserved by
//! [`crate::background::terminal_run_index::is_reserved_async_root_entry`] against a module that
//! did not exist; this is that module. Every async-root scanner therefore already skips it.
//!
//! # The router, and the ordering it must get right
//!
//! [`update_active_run_index`] is a two-armed router over the run's state:
//!
//! * **queued or running** — `mkdir -p` the index dir and touch the marker, plus a best-effort
//!   tool-call alias whose failure is logged and swallowed (*"the authoritative active-run marker
//!   must keep the launch discoverable even when alias indexing fails"*, pi `:89-93`);
//! * **anything else** — touch the marker, write the TERMINAL index, and only then release the
//!   active marker and its aliases.
//!
//! That second ordering is upstream's `terminalIndexBeforeRelease` (added between `v0.66.0` and
//! `v0.68.0`), and it closes a real hole: releasing the active entry first leaves a run in
//! NEITHER index if the process dies between the two writes. cyrup ships only the fixed ordering
//! — upstream keeps the flag because its own call sites predate the fix and it must not change
//! them all at once, whereas cyrup's four production call sites all land after it. Shipping the
//! racy arm as a reachable option would reproduce a hole upstream has just closed, for no caller.
//!
//! # One upstream behaviour has no cyrup analog and is recorded rather than invented
//!
//! * `retryCapacityErrors` (pi `:103`) rethrows a *storage-capacity* error from the terminal-index
//!   write instead of logging it. There is no `isStorageCapacityError` in this crate and no errno
//!   classifier to build one from; this port returns [`std::io::Result`] and lets the caller —
//!   which already logs-and-continues at every site — decide. That is the same outcome for every
//!   error class cyrup can currently distinguish.

use std::path::{Path, PathBuf};

use crate::background::process_terminal::{
    ProcessTerminal, ProcessTerminalState, ProofExpectation, read_process_terminal,
};
use crate::background::{RunId, RunState, RunStatus};
use crate::identity::IndexSegment;

/// pi `ACTIVE_RUN_INDEX_DIR` (`active-run-index.ts:9`). Already reserved at
/// [`crate::background::terminal_run_index::is_reserved_async_root_entry`].
pub const ACTIVE_RUN_INDEX_DIR: &str = ".active-runs";

/// pi `DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS` (`:10`) — 24 hours.
///
/// The threshold the READER (`async-status.ts:573-577`) applies to
/// [`active_run_marker_age_ms`]: a marker older than this belongs to a run whose terminal
/// transition never got to release it. [`read_live_active_run_ids`] is that reader, and both
/// production listings go through it.
pub const DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS: i64 = 24 * 60 * 60 * 1000;

/// pi `TOOL_CALL_INDEX_DIR` (`:12`) — not exported upstream either.
const TOOL_CALL_INDEX_DIR: &str = "tool-calls";

/// `<async_root>/.active-runs` — pi `indexDir` (`:14-16`).
fn index_dir(async_root: &Path) -> PathBuf {
    async_root.join(ACTIVE_RUN_INDEX_DIR)
}

/// `<async_root>/.active-runs/tool-calls/<enc(toolCallId)>` — pi `toolCallIndexDir` (`:18-20`).
///
/// ONE key from [`IndexSegment::encode`], never the alias fan-out, for
/// `terminal_run_index::entry`'s stated reason: a write addresses exactly one location, and reads
/// that need history fan out instead.
fn tool_call_index_dir(async_root: &Path, tool_call_id: &str) -> PathBuf {
    index_dir(async_root)
        .join(TOOL_CALL_INDEX_DIR)
        .join(IndexSegment::encode(tool_call_id).as_str())
}

/// `<async_root>/.active-runs/<runDir>` — pi `markerPath` (`:26-28`).
///
/// `None` when `async_dir` has no parent or a non-UTF-8 leaf; pi's `path.dirname`/`path.basename`
/// cannot fail, so the `Option` is cyrup's and every caller treats it as "nothing to index" —
/// the same shape [`crate::background::terminal_run_index`]'s own `marker_path` has.
fn marker_path(async_dir: &Path) -> Option<PathBuf> {
    let async_root = async_dir.parent()?;
    let name = async_dir.file_name()?.to_str()?;
    Some(index_dir(async_root).join(name))
}

/// `<…>/tool-calls/<enc(id)>/<runDir>` — pi `toolCallIndexPath` (`:22-24`).
fn tool_call_index_path(async_dir: &Path, tool_call_id: &str) -> Option<PathBuf> {
    let async_root = async_dir.parent()?;
    let name = async_dir.file_name()?.to_str()?;
    Some(tool_call_index_dir(async_root, tool_call_id).join(name))
}

/// pi `removeEmptyAncestors` (`:30-40`): `rmdir` upward from `start` while it is strictly below
/// `stop`, stopping at the first directory that is not empty.
///
/// `rmdir` (never `remove_dir_all`) is the whole safety property: a directory that still holds
/// another run's alias fails with `ENOTEMPTY` and the walk stops there.
async fn remove_empty_ancestors(start: &Path, stop: &Path) {
    let mut current = start.to_path_buf();
    while current != stop && current.starts_with(stop) {
        if tokio::fs::remove_dir(&current).await.is_err() {
            return;
        }
        let Some(parent) = current.parent() else {
            return;
        };
        current = parent.to_path_buf();
    }
}

/// pi `isActiveAsyncState` (`:42-44`) — `queued` or `running`.
///
/// The EXACT complement of
/// [`crate::background::terminal_run_index`]'s `is_indexed_state`, which is what makes the two
/// indexes a partition rather than two overlapping views: every state lands in exactly one of
/// them, `Paused` included (it is neither queued nor running, so it takes the TERMINAL arm — see
/// that module's own note on why its "terminal" set is not [`RunState::is_terminal`]).
#[must_use]
pub const fn is_active_async_state(state: RunState) -> bool {
    matches!(state, RunState::Queued | RunState::Running)
}

/// pi `releaseToolCallAliases` (`:46-68`): unlink this run's marker from EVERY tool-call alias
/// directory, pruning directories that become empty.
///
/// Scans all alias directories rather than deriving the one from the status, because the run's
/// `tool_call_id` may have changed (or been absent) since the alias was written — upstream does
/// the same, and an orphaned alias is a phantom active run for every later reader.
///
/// Entirely best-effort: *"Alias cleanup must not affect the authoritative active-run marker."*
async fn release_tool_call_aliases(async_dir: &Path) {
    let (Some(async_root), Some(name)) = (
        async_dir.parent(),
        async_dir.file_name().and_then(|n| n.to_str()),
    ) else {
        return;
    };
    let root = index_dir(async_root).join(TOOL_CALL_INDEX_DIR);
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) => {
            // pi `:55-58` — ENOENT/ENOTDIR are the ordinary "no aliases were ever written" shapes.
            if !matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) {
                tracing::warn!(
                    root = %root.display(),
                    %error,
                    "failed to inspect async active-run tool-call index root"
                );
            }
            return;
        }
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let alias_marker = entry.path().join(name);
        if tokio::fs::remove_file(&alias_marker).await.is_ok() {
            remove_empty_ancestors(&entry.path(), &root).await;
        }
    }
}

/// pi `releaseActiveRunIndex` (`:70-77`): drop this run's authoritative marker and every alias.
///
/// A missing marker is success — the release is idempotent, which is what lets it be called from
/// dismissal, from reconciliation and from the terminal arm of [`update_active_run_index`] without
/// any of them having to know whether another already ran.
///
/// # Errors
///
/// A marker-unlink failure other than "not found" (pi `:73` rethrows the same way).
pub async fn release_active_run_index(async_dir: &Path) -> std::io::Result<()> {
    if let Some(marker) = marker_path(async_dir)
        && let Err(error) = tokio::fs::remove_file(&marker).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error);
    }
    release_tool_call_aliases(async_dir).await;
    Ok(())
}

/// pi `updateActiveRunIndex` (`:79-122` @`v0.68.0`) — THE router every status writer calls.
///
/// Takes a typed `&RunStatus` rather than upstream's loose `(state, toolCallId?)` pair: cyrup
/// already threads a `&RunStatus` to `update_terminal_run_index`, the record carries both values,
/// and the terminal arm needs the whole record anyway. The one upstream caller that has a state
/// but no record — `async-status.ts:514`/`:570`'s "synthesize `failed` for an unreadable run" —
/// is served by [`mark_active_run_failed`].
///
/// # Errors
///
/// A marker write failure (the authoritative marker is NOT best-effort — a launch that cannot be
/// indexed is a launch later readers cannot see), or the terminal-index delegation's own failure.
/// Every production call site treats the whole call as best-effort and logs, exactly as
/// `runner_main/finish.rs` already does.
pub async fn update_active_run_index(async_dir: &Path, status: &RunStatus) -> std::io::Result<()> {
    update_for_state(async_dir, status.state, status.tool_call_id.as_deref()).await
}

/// pi `updateActiveRunIndex(asyncDir, "failed")` (`async-status.ts:514`, `:570`) — the entry point
/// for a caller that has decided a run is failed WITHOUT holding its record.
///
/// # Errors
///
/// [`update_active_run_index`]'s.
pub async fn mark_active_run_failed(async_dir: &Path) -> std::io::Result<()> {
    update_for_state(async_dir, RunState::Failed, None).await
}

/// The router body, shared by the two public entry points.
async fn update_for_state(
    async_dir: &Path,
    state: RunState,
    tool_call_id: Option<&str>,
) -> std::io::Result<()> {
    let Some(marker) = marker_path(async_dir) else {
        return Ok(());
    };

    if is_active_async_state(state) {
        touch_marker(&marker).await?;
        // pi `:85-93` — the alias is optional and its failure is logged, never propagated.
        if let Some(tool_call_id) = tool_call_id.filter(|id| !id.is_empty())
            && let Some(alias) = tool_call_index_path(async_dir, tool_call_id)
            && let Err(error) = touch_marker(&alias).await
        {
            tracing::warn!(
                async_dir = %async_dir.display(),
                %error,
                "failed to write async active-run tool-call index; the authoritative active-run \
                 marker still records the launch"
            );
        }
        return Ok(());
    }

    // THE ORDERING (pi's `terminalIndexBeforeRelease` arm, `:97-112` @`v0.68.0`): marker, then the
    // TERMINAL index, then the release. A process that dies between the last two leaves the run in
    // BOTH indexes — recoverable. The pre-fix ordering left it in NEITHER.
    touch_marker(&marker).await?;

    // pi `:100-101` — re-read, and delegate only if the on-disk state still equals the state we
    // were asked to publish. A status that has moved on since belongs to whoever wrote it, not to
    // this call; indexing the stale state would record a transition that never happened.
    if let Some(on_disk) = read_status(async_dir).await
        && on_disk.state == state
    {
        // Returned rather than logged-and-swallowed: upstream's `retryCapacityErrors` arm exists so
        // a storage-capacity failure can reach the caller, and cyrup has no errno classifier to
        // reproduce the narrowing (see this module's header). Returning gives every caller the
        // same information without inventing one — and, exactly as upstream does at `:106`, the
        // active marker is NOT released on this path, so the run stays discoverable.
        crate::background::terminal_run_index::update_terminal_run_index(async_dir, &on_disk)
            .await?;
    }

    release_active_run_index(async_dir).await
}

/// `mkdir -p` + append-mode create — pi's `fs.writeFileSync(marker, "", { flag: "a" })` (`:83-84`).
///
/// Append mode rather than truncate is upstream's choice and it matters: touching an existing
/// marker must refresh its mtime ([`active_run_marker_age_ms`] reads it) without ever racing a
/// concurrent reader into seeing a zero-length file that was previously non-empty.
async fn touch_marker(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map(drop)
}

/// The run's own `status.json`, or `None` when it is missing or unreadable — pi `readStatus`.
async fn read_status(async_dir: &Path) -> Option<RunStatus> {
    let bytes = tokio::fs::read(crate::background::RunDir::for_existing(async_dir).status())
        .await
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// pi `activeRunMarkerAgeMs` (`:124-131`): how long ago this run's active marker was last touched,
/// or `None` when there is no marker.
///
/// The input to [`read_live_active_run_ids`]'s 24-hour staleness rung
/// ([`DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS`]): a marker that old belongs to a run whose
/// terminal transition never got to release it.
#[must_use]
pub async fn active_run_marker_age_ms(async_dir: &Path, now: i64) -> Option<i64> {
    let marker = marker_path(async_dir)?;
    let modified = tokio::fs::metadata(&marker).await.ok()?.modified().ok()?;
    Some(
        now.saturating_sub(crate::time::epoch_millis(modified))
            .max(0),
    )
}

/// pi `readActiveRunIndex` (`:133-142`): the run-directory names this index currently lists, or
/// `None` when the index does not exist at all.
///
/// The `None`/`Some(vec![])` distinction is upstream's and is load-bearing for the reader: "no
/// index" means fall back to the full scan, while "an empty index" means there are genuinely no
/// active runs.
#[must_use]
pub async fn read_active_run_index(async_root: &Path) -> Option<Vec<String>> {
    read_marker_names(&index_dir(async_root)).await
}

/// pi `listAsyncRuns`'s ACTIVE candidate pass (`async-status.ts:506-516` + `:568-577`
/// @`v0.68.0`) — the run ids this index still claims are in flight, with the index repaired in the
/// same read.
///
/// This is the index-first replacement for a `read_dir` of `<async_root>`, and it is what the two
/// production listings consume: [`crate::background::run_status::list_active_runs`] (and through
/// it `wait`, `status`, `auto_drain` and every control op) and
/// [`crate::tui::fleet::collect_fleet_history`]. `None` — the index directory does not exist —
/// means "this root has no index yet", and every consumer falls back to its full scan; that is the
/// same `None`/`Some(vec![])` contract [`read_active_run_index`] documents, and it is what lets a
/// marker be unlinked at any time without a listing losing a run.
///
/// # The two repairs, both upstream's
///
/// * **A phantom entry** — a marker whose run has no readable `status.json`, because the directory
///   was never a run or was removed underneath the index. pi calls
///   `updateActiveRunIndex(asyncDir, "failed")` for exactly this (`:514`, and again at `:570` for
///   `!status`); [`mark_active_run_failed`] is that call, and it releases the marker so the next
///   read is not paying for it again.
/// * **The staleness rung** — a marker whose run reads back NON-active. pi releases it when the
///   run's process-terminal record was OBSERVED, or when [`active_run_marker_age_ms`] exceeds
///   [`DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS`] (`:573-577`). Both disjuncts are applied here,
///   in upstream's order, and they answer two different situations:
///
///   - The **proof** disjunct is the fast, POSITIVE one. A run whose runner reached its close
///     wrote `process-terminal.json` with `state: "observed"`
///     ([`finalize_process_terminal`](crate::background::process_terminal::finalize_process_terminal)),
///     which is a definite statement that the process that owned the run is gone. Its marker is
///     released on the very next read — not 24 hours later — so a run that ended a second ago
///     stops being reported as live by every listing that goes through this reader.
///   - The **age** disjunct is the fallback for a run that could not make that statement: a
///     runner killed hard enough to skip its own close writes no proof, and its `pending` sidecar
///     never becomes `observed`. Below 24 hours the marker is left alone, because the terminal
///     write may simply not have landed yet, and releasing early is how a run ends up in neither
///     index.
///
///   The proof is read against upstream's own expectation (`:574`): this run's id, and the runner
///   instance the status itself names. A sidecar belonging to another run or another runner
///   degrades to `unknown` at the read and releases nothing.
///
/// Both repairs are best-effort and logged: this is a READ, and a listing that cannot tidy the
/// index must still answer. Repaired or not, a non-active entry is never reported as live.
#[must_use]
pub async fn read_live_active_run_ids(async_root: &Path) -> Option<Vec<RunId>> {
    let names = read_active_run_index(async_root).await?;
    let now = crate::time::now_epoch_millis();
    let mut live = Vec::with_capacity(names.len());
    for name in names {
        let async_dir = async_root.join(&name);
        let Some(status) = read_status(&async_dir).await else {
            if let Err(error) = mark_active_run_failed(&async_dir).await {
                tracing::warn!(
                    async_dir = %async_dir.display(),
                    %error,
                    "failed to release an async active-run marker whose run has no readable status"
                );
            }
            continue;
        };
        if is_active_async_state(status.state) {
            live.push(RunId::from_token(name));
            continue;
        }
        // pi `:574-576` — the proof first, the age threshold second.
        let run_dir = crate::background::RunDir::for_existing(&async_dir);
        let observed = read_process_terminal(
            &run_dir,
            ProofExpectation {
                run_id: Some(&status.run_id),
                runner_process_instance_id: status
                    .process_terminal
                    .as_ref()
                    .map(ProcessTerminal::runner_process_instance_id),
            },
        )
        .await
        .is_some_and(|proof| proof.state() == ProcessTerminalState::Observed);
        if (observed
            || active_run_marker_age_ms(&async_dir, now)
                .await
                .is_some_and(|age| age > DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS))
            && let Err(error) = update_active_run_index(&async_dir, &status).await
        {
            tracing::warn!(
                async_dir = %async_dir.display(),
                %error,
                "failed to release a stale async active-run marker"
            );
        }
    }
    // `read_active_run_index` already sorted the marker names, and the ids are those names, so the
    // listing is deterministic without a second sort.
    Some(live)
}

/// pi `readActiveRunToolCallIndex` (`:144-155`): the run-directory names filed under one tool-call
/// alias. A missing alias is an empty list, never `None` — upstream collapses `ENOENT`/`ENOTDIR`
/// to `[]` here because an absent alias is a definite answer, not a missing index.
#[must_use]
pub async fn read_active_run_tool_call_index(async_root: &Path, tool_call_id: &str) -> Vec<String> {
    read_marker_names(&tool_call_index_dir(async_root, tool_call_id))
        .await
        .unwrap_or_default()
}

/// The shared body of the two readers: FILE entries only (pi's `entry.isFile()`), so
/// `tool-calls/` never appears in the active-run listing.
async fn read_marker_names(dir: &Path) -> Option<Vec<String>> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    let mut names = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_type().await.is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Some(names)
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
    use crate::background::RunMode;
    use crate::background::atomic::write_atomic_json;
    use crate::identity::SessionId;

    fn status(state: RunState, tool_call_id: Option<&str>) -> RunStatus {
        let mut status = RunStatus::queued(RunId::from_token("run1"), RunMode::Single, Some(1));
        status.state = state;
        status.session_id = SessionId::parse_opt(Some("s1"));
        status.tool_call_id = tool_call_id.map(str::to_string);
        status.last_update = 42;
        status
    }

    async fn write_status(async_dir: &Path, status: &RunStatus) {
        tokio::fs::create_dir_all(async_dir).await.expect("mkdir");
        write_atomic_json(
            &crate::background::RunDir::for_existing(async_dir).status(),
            status,
        )
        .await
        .expect("write status");
    }

    fn terminal_markers(async_root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let Ok(sessions) = std::fs::read_dir(async_root.join(".terminal-runs")) else {
            return found;
        };
        for session in sessions.flatten() {
            if let Ok(files) = std::fs::read_dir(session.path()) {
                found.extend(files.flatten().map(|f| f.path()));
            }
        }
        found.sort();
        found
    }

    /// Every [`RunState`] this crate has, so the partition below cannot silently stop covering one.
    const EVERY_STATE: [RunState; 6] = [
        RunState::Queued,
        RunState::Running,
        RunState::Paused,
        RunState::Complete,
        RunState::Failed,
        RunState::Stopped,
    ];

    #[tokio::test]
    async fn the_active_set_is_exactly_the_complement_of_the_terminal_index() {
        assert!(is_active_async_state(RunState::Queued));
        assert!(is_active_async_state(RunState::Running));
        // `Paused` takes the TERMINAL arm in both indexes — SCOPE_8's open question 7, resolved
        // by preserving the existing behaviour rather than gating on `RunState::is_terminal`.
        assert!(!is_active_async_state(RunState::Paused));
        assert!(!is_active_async_state(RunState::Complete));
        assert!(!is_active_async_state(RunState::Failed));
        assert!(!is_active_async_state(RunState::Stopped));

        // The name's actual claim, asserted against BOTH indexes rather than against this one
        // predicate: routing every state through `update_active_run_index` must land the run in
        // exactly one index, never both and never neither. The predicate above could agree with
        // itself while the router disagreed with it — this is the half that catches that.
        for state in EVERY_STATE {
            let tmp = tempfile::tempdir().expect("tempdir");
            let async_dir = tmp.path().join("run1");
            let status = status(state, None);
            write_status(&async_dir, &status).await;
            update_active_run_index(&async_dir, &status)
                .await
                .expect("index");

            let active = read_active_run_index(tmp.path()).await.unwrap_or_default();
            let terminal = terminal_markers(tmp.path());
            assert_eq!(
                active,
                if is_active_async_state(state) {
                    vec!["run1".to_string()]
                } else {
                    Vec::new()
                },
                "active index for {state:?}"
            );
            assert_eq!(
                terminal.len(),
                usize::from(!is_active_async_state(state)),
                "terminal index for {state:?}: {terminal:?}"
            );
        }
    }

    #[tokio::test]
    async fn update_active_run_index_writes_a_marker_for_a_running_run() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        let status = status(RunState::Running, None);
        write_status(&async_dir, &status).await;

        update_active_run_index(&async_dir, &status)
            .await
            .expect("marker write");

        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(vec!["run1".to_string()])
        );
        assert!(tmp.path().join(".active-runs/run1").is_file());
        assert!(terminal_markers(tmp.path()).is_empty(), "still active");
    }

    #[tokio::test]
    async fn a_missing_index_reads_as_none_and_an_empty_one_as_an_empty_list() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_active_run_index(tmp.path()).await, None);
        tokio::fs::create_dir_all(tmp.path().join(".active-runs"))
            .await
            .expect("mkdir");
        assert_eq!(read_active_run_index(tmp.path()).await, Some(Vec::new()));
    }

    #[tokio::test]
    async fn update_active_run_index_releases_and_delegates_to_the_terminal_index_on_terminal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        let running = status(RunState::Running, None);
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("active");

        let mut done = status(RunState::Complete, None);
        done.ended_at = Some(77);
        write_status(&async_dir, &done).await;
        update_active_run_index(&async_dir, &done)
            .await
            .expect("terminal");

        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(Vec::new()),
            "the active marker is gone"
        );
        let markers = terminal_markers(tmp.path());
        assert_eq!(markers.len(), 1, "got {markers:?}");
        assert!(
            markers[0]
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("0000000000000077-")),
            "got {markers:?}"
        );
    }

    #[tokio::test]
    async fn a_tool_call_alias_is_written_and_released_with_its_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        let running = status(RunState::Running, Some("call-7"));
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("active");

        assert_eq!(
            read_active_run_tool_call_index(tmp.path(), "call-7").await,
            vec!["run1".to_string()]
        );

        let done = status(RunState::Complete, Some("call-7"));
        write_status(&async_dir, &done).await;
        update_active_run_index(&async_dir, &done)
            .await
            .expect("terminal");

        assert!(
            read_active_run_tool_call_index(tmp.path(), "call-7")
                .await
                .is_empty()
        );
        // pi `removeEmptyAncestors` (`:30-40`): the now-empty alias directory is pruned, but the
        // `tool-calls` root it hangs off is the walk's STOP and survives.
        assert!(
            !tmp.path()
                .join(".active-runs/tool-calls")
                .join(IndexSegment::encode("call-7").as_str())
                .exists()
        );
        assert!(tmp.path().join(".active-runs/tool-calls").is_dir());
    }

    #[tokio::test]
    async fn an_alias_write_failure_does_not_lose_the_authoritative_marker() {
        // pi `:89-93`. The alias's own encoded directory is occupied by a FILE, so `mkdir -p` for
        // it fails while the authoritative marker is unaffected. The `tool-calls` root itself stays
        // a real directory, so the file-only listing skips it rather than reporting it as a run.
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        tokio::fs::create_dir_all(tmp.path().join(".active-runs/tool-calls"))
            .await
            .expect("mkdir");
        tokio::fs::write(
            tmp.path()
                .join(".active-runs/tool-calls")
                .join(IndexSegment::encode("call-7").as_str()),
            b"not a directory",
        )
        .await
        .expect("write");

        let running = status(RunState::Running, Some("call-7"));
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("the alias failure is swallowed");

        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(vec!["run1".to_string()])
        );
        assert!(
            read_active_run_tool_call_index(tmp.path(), "call-7")
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_terminal_delegation_is_skipped_when_the_on_disk_state_has_moved_on() {
        // pi `:100` — `status?.state === state`.
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        // On disk the run is still RUNNING; we are asked to publish `complete`.
        write_status(&async_dir, &status(RunState::Running, None)).await;

        update_active_run_index(&async_dir, &status(RunState::Complete, None))
            .await
            .expect("no-op delegation");

        assert!(
            terminal_markers(tmp.path()).is_empty(),
            "a transition that never happened must not be indexed"
        );
        // The active marker is still released — upstream releases unconditionally on this arm.
        assert_eq!(read_active_run_index(tmp.path()).await, Some(Vec::new()));
    }

    #[tokio::test]
    async fn mark_active_run_failed_releases_a_marker_without_a_record() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        let running = status(RunState::Running, None);
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("active");

        mark_active_run_failed(&async_dir)
            .await
            .expect("synthesized failure");

        assert_eq!(read_active_run_index(tmp.path()).await, Some(Vec::new()));
        // The on-disk state is still `running`, so there is nothing to delegate — the marker is
        // released all the same, which is the whole point of this entry point.
        assert!(terminal_markers(tmp.path()).is_empty());
    }

    #[tokio::test]
    async fn releasing_an_unindexed_run_creates_nothing_and_release_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        release_active_run_index(&async_dir)
            .await
            .expect("a missing marker is success");
        // A release must never CREATE the index it was asked to remove from: `read_active_run_index`
        // returning `None` is what tells every consumer to fall back to the full scan, and an empty
        // directory here would tell them the root has genuinely no active runs.
        assert_eq!(read_active_run_index(tmp.path()).await, None);
        assert!(!tmp.path().join(ACTIVE_RUN_INDEX_DIR).exists());

        // The real idempotence claim: the FIRST release of an indexed run removes the marker and
        // its alias, and a second release of the same run is still `Ok` and changes nothing.
        let running = status(RunState::Running, Some("call-7"));
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("active");
        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(vec!["run1".to_string()])
        );

        release_active_run_index(&async_dir)
            .await
            .expect("first release");
        assert_eq!(read_active_run_index(tmp.path()).await, Some(Vec::new()));
        assert!(
            read_active_run_tool_call_index(tmp.path(), "call-7")
                .await
                .is_empty()
        );

        release_active_run_index(&async_dir)
            .await
            .expect("second release");
        assert_eq!(read_active_run_index(tmp.path()).await, Some(Vec::new()));
    }

    #[tokio::test]
    async fn the_marker_age_is_none_without_a_marker_and_measures_from_its_own_mtime() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        assert!(active_run_marker_age_ms(&async_dir, 0).await.is_none());

        let running = status(RunState::Running, None);
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("active");

        // Measured against the marker's OWN mtime, so the assertion is exact arithmetic rather
        // than `age >= 0` — which `Math.max(0, …)` guarantees for every input and therefore
        // proves nothing about the subtraction.
        let mtime = tokio::fs::metadata(tmp.path().join(ACTIVE_RUN_INDEX_DIR).join("run1"))
            .await
            .expect("marker metadata")
            .modified()
            .expect("mtime");
        let mtime_ms = crate::time::epoch_millis(mtime);
        assert_eq!(
            active_run_marker_age_ms(&async_dir, mtime_ms + 5_000).await,
            Some(5_000)
        );
        assert_eq!(
            active_run_marker_age_ms(&async_dir, mtime_ms).await,
            Some(0),
            "a clock exactly at the mtime is zero old, not one"
        );
        // A clock behind the marker's mtime clamps to zero rather than going negative (pi's
        // `Math.max(0, …)`).
        assert_eq!(active_run_marker_age_ms(&async_dir, 0).await, Some(0));
    }

    /// Backdate a run's active marker by `ms`, so the 24-hour staleness rung is reachable without
    /// a 24-hour wait and without any wall-clock bound in the assertion.
    fn backdate_marker(async_dir: &Path, ms: u64) {
        let marker = marker_path(async_dir).expect("a marker path");
        let file = std::fs::File::options()
            .write(true)
            .open(&marker)
            .expect("open marker");
        let when = std::time::SystemTime::now() - std::time::Duration::from_millis(ms);
        file.set_times(std::fs::FileTimes::new().set_modified(when))
            .expect("set mtime");
    }

    #[tokio::test]
    async fn the_live_reader_reports_only_active_runs_and_releases_a_phantom_marker() {
        // pi `async-status.ts:508-515` — a marker whose run has no readable status is repaired in
        // the same read that found it, by `updateActiveRunIndex(asyncDir, "failed")`.
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            read_live_active_run_ids(tmp.path()).await,
            None,
            "no index at all means fall back to the full scan, not `no active runs`"
        );

        let live_dir = tmp.path().join("live");
        let mut running = status(RunState::Running, None);
        running.run_id = RunId::from_token("live");
        write_status(&live_dir, &running).await;
        update_active_run_index(&live_dir, &running)
            .await
            .expect("active");

        // A phantom: the index claims a run whose directory carries no readable `status.json`.
        tokio::fs::create_dir_all(tmp.path().join("phantom"))
            .await
            .expect("mkdir");
        tokio::fs::write(tmp.path().join(ACTIVE_RUN_INDEX_DIR).join("phantom"), b"")
            .await
            .expect("phantom marker");
        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(vec!["live".to_string(), "phantom".to_string()])
        );

        assert_eq!(
            read_live_active_run_ids(tmp.path()).await,
            Some(vec![RunId::from_token("live")])
        );
        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(vec!["live".to_string()]),
            "the phantom marker is released by the read that found it"
        );
    }

    #[tokio::test]
    async fn a_terminal_runs_marker_survives_until_it_is_a_day_stale() {
        // pi `async-status.ts:570-577`. Below the threshold the marker is LEFT: the terminal write
        // may simply not have landed yet, and releasing early is how a run ends up in neither
        // index. Past it, the repair runs `terminalIndexBeforeRelease`-style — terminal index
        // first, active marker second.
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        let running = status(RunState::Running, None);
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("active");

        // The run went terminal without ever releasing its own marker.
        let mut done = status(RunState::Complete, None);
        done.ended_at = Some(77);
        write_status(&async_dir, &done).await;

        assert_eq!(
            read_live_active_run_ids(tmp.path()).await,
            Some(Vec::new()),
            "a non-active entry is never reported as live, repaired or not"
        );
        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(vec!["run1".to_string()]),
            "a fresh marker is left alone"
        );
        assert!(terminal_markers(tmp.path()).is_empty());

        backdate_marker(&async_dir, 60_000 + 24 * 60 * 60 * 1000);
        assert_eq!(read_live_active_run_ids(tmp.path()).await, Some(Vec::new()));
        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(Vec::new()),
            "past the staleness threshold the marker is released"
        );
        let markers = terminal_markers(tmp.path());
        assert_eq!(markers.len(), 1, "got {markers:?}");
        assert!(
            markers[0]
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("0000000000000077-")),
            "the repair writes the TERMINAL index before releasing: {markers:?}"
        );
    }

    /// pi `async-status.ts:574-576`'s FIRST disjunct. A run that closed cleanly one second ago
    /// has an `observed` sidecar, and its marker is released on the very next read — without
    /// waiting out the 24-hour age rung, and without ever being reported as live.
    ///
    /// The counter-rung is in the same test: a `pending` sidecar — the shape a runner that was
    /// killed leaves behind — releases NOTHING, because the age rung is the only thing that may
    /// speak for a run whose process never made a statement about itself.
    #[tokio::test]
    async fn an_observed_proof_releases_the_marker_before_the_age_rung() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let async_dir = tmp.path().join("run1");
        let running = status(RunState::Running, None);
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("active");

        let mut done = status(RunState::Complete, None);
        done.ended_at = Some(77);
        write_status(&async_dir, &done).await;

        // A runner that was killed leaves the launch's `pending` sidecar in place.
        write_sidecar(&async_dir, &pending_proof()).await;
        assert_eq!(read_live_active_run_ids(tmp.path()).await, Some(Vec::new()));
        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(vec!["run1".to_string()]),
            "a pending proof is not a statement that the runner is gone"
        );
        assert!(terminal_markers(tmp.path()).is_empty());

        // The runner reached its close and proved it.
        write_sidecar(&async_dir, &observed_proof()).await;
        assert_eq!(read_live_active_run_ids(tmp.path()).await, Some(Vec::new()));
        assert_eq!(
            read_active_run_index(tmp.path()).await,
            Some(Vec::new()),
            "an observed proof releases the marker the moment it is read"
        );
        assert_eq!(
            terminal_markers(tmp.path()).len(),
            1,
            "the repair still writes the terminal index BEFORE releasing"
        );
    }

    /// A sidecar in `run_dir`, in the state named — built through the same typed
    /// [`ProcessTerminal`] the production writer encodes, so the file is exactly the shape
    /// [`finalize_process_terminal`](crate::background::process_terminal::finalize_process_terminal)
    /// writes and `validate_proof` accepts (`process-terminal.ts:170-175`).
    async fn write_sidecar(async_dir: &Path, proof: &ProcessTerminal) {
        tokio::fs::create_dir_all(async_dir).await.expect("mkdir");
        tokio::fs::write(
            crate::background::RunDir::for_existing(async_dir).process_terminal(),
            serde_json::to_vec(proof).expect("encode"),
        )
        .await
        .expect("write sidecar");
    }

    fn proof_base() -> crate::background::process_terminal::ProcessTerminalBase {
        crate::background::process_terminal::ProcessTerminalBase::new(
            RunId::from_token("run1"),
            crate::background::process_terminal::RunnerProcessInstanceId::from_token("inst-1"),
        )
    }

    fn pending_proof() -> ProcessTerminal {
        ProcessTerminal::Pending { base: proof_base() }
    }

    fn observed_proof() -> ProcessTerminal {
        ProcessTerminal::Observed {
            base: proof_base(),
            observed_at: 1_700_000_000_000,
            instances: vec![
                crate::background::process_terminal::ProcessInstanceExit::Runner {
                    process_instance_id:
                        crate::background::process_terminal::RunnerProcessInstanceId::from_token(
                            "inst-1",
                        ),
                    close_observed_at: 1_700_000_000_000,
                    exit_code: Some(0),
                    signal: None,
                },
            ],
            canonical_session: None,
        }
    }

    #[tokio::test]
    async fn the_active_index_dir_is_never_listed_as_a_run() {
        // `is_reserved_async_root_entry` now has a real directory behind it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let results = tmp.path().join("results");
        tokio::fs::create_dir_all(&results).await.expect("mkdir");
        let async_root = tmp.path().join("async");
        let async_dir = async_root.join("run1");
        let running = status(RunState::Running, None);
        write_status(&async_dir, &running).await;
        update_active_run_index(&async_dir, &running)
            .await
            .expect("active");
        assert!(async_root.join(".active-runs").is_dir());

        let listed = crate::background::run_status::list_active_runs(&async_root, &results, None)
            .await
            .expect("listing");
        assert_eq!(listed.len(), 1, "the reserved index dir is not a run");
        assert_eq!(listed[0].status.run_id.as_str(), "run1");
    }
}
