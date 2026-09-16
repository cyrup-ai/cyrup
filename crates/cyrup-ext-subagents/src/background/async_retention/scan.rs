//! One cursor-windowed pass over the async root, and every filesystem probe
//! [`decide`](super::decide) needs hoisted out of it.
//!
//! # The batching is a CURSOR-WINDOWED SELECTION, not "read 100 entries and stop"
//!
//! The mechanism is `streamDirWindow` (`async-retention-discovery-worker.mjs:22-48`), driven at
//! `:58-68`. Read literally, because a naive "stop after `limit` entries" scan never converges
//! and never wraps:
//!
//! 1. `opendir` and read **every** entry to exhaustion (`:30-39`). `raw_reads` counts all of
//!    them, so one pass always costs a full listing of the async root. The budget bounds the
//!    OUTPUT, not the read.
//! 2. Keep two ascending, `limit`-bounded windows of the smallest names (`insertSmallest`,
//!    `:14-20`): `wrapped` over all usable entries, and `next` over only those strictly greater
//!    than the cursor.
//! 3. `cursorCleared = after !== undefined && next.length === 0` (`:40`) — the cursor ran off the
//!    end. Return `wrapped` instead, and delete the cursor (`:66`). **This wrap-around is what
//!    makes repeated passes eventually visit every directory**; without it the scan sticks at the
//!    end of the name order forever and early directories are never revisited.
//! 4. `ENOENT` on the whole directory is an empty window, not an error (`:43`); anything else
//!    throws (`:43-44`).
//! 5. The caller advances the cursor to the LAST candidate returned (`:68`).
//!
//! The ordering is byte-lexicographic (`compareRelative`, `:8-12`), which in Rust is [`Ord`] on
//! [`str`] — never a locale collation, and never the OS's `read_dir` order.
//!
//! # Fallible on purpose
//!
//! Unlike `completion_replay::retention`'s swallow-everything sweep, this returns
//! [`std::io::Result`]. The split is the one
//! [`errno`](crate::background::result_index) already documents — *"Quiet when scanning, loud
//! when asked a direct question"*: an absent or permission-denied listing is an empty window, and
//! **any other** fault propagates. Every errno decision goes through that module; there is no
//! fresh `matches!` on [`std::io::ErrorKind`] in this file.

use std::path::Path;

use crate::background::result_index::errno;
use crate::background::terminal_run_index::is_reserved_async_root_entry;
use crate::background::{RunDir, RunId, RunStatus};

use super::policy::RunRetentionFacts;
use super::tombstone::{
    TombstoneMarkerState, normalize_tombstone_path, read_run_tombstone_marker_self_healing,
};
use super::{RUN_TOMBSTONE_PREFIX, maintenance_root};

/// pi `MISSION_BINDING_FILE` (`missions/lifecycle.ts:10`), imported by `async-retention.ts:9` and
/// probed at `:298`. cyrup's own constant, re-used rather than re-spelled.
use crate::missions::MISSION_BINDING_FILE;

/// `<run_dir>/handoff.json` — pi's local handoff manifest (`hasUnresolvedRunHandoff`, `:274`).
const HANDOFF_MANIFEST_FILE: &str = "handoff.json";

/// `<run_dir>/recovery-descriptor.json` — pi `hasResumableContract` (`:196`).
const RECOVERY_DESCRIPTOR_FILE: &str = "recovery-descriptor.json";

/// One pass's inputs. A struct rather than upstream's five positional arguments because three of
/// the five are roots that must agree with each other, and a positional list of `&Path`s is the
/// shape in which they get transposed.
///
/// [CYRUP-DELTA] upstream's discovery worker takes only `asyncDirRoot` because its `runSkipReason`
/// does its own I/O later, on the commit thread. Making the policy pure moves every probe here, so
/// this pass needs the two other roots those probes address: `results_dir` for the mission
/// observer index (READ ONLY — see the module boundary) and `maintenance_root` for the tombstone
/// markers.
#[derive(Clone, Copy, Debug)]
pub struct RunScanRequest<'a> {
    /// `<temp_root>/async/<cwd_key>` — [`crate::background::RunArtifactRoots::async_root`].
    pub async_root: &'a Path,
    /// `<temp_root>/results/<cwd_key>` — read for the mission-observer liveness guard and for
    /// nothing else. This module never writes or deletes inside it.
    pub results_dir: &'a Path,
    /// Where the run-tombstone markers live. `None` uses
    /// [`super::maintenance_root`]`(async_root)`.
    pub maintenance_root: Option<&'a Path>,
    /// The cursor: return names strictly greater than this, wrapping when it runs off the end.
    pub after: Option<&'a str>,
    /// The candidate budget —
    /// [`ASYNC_RETENTION_BATCH_SIZE`](super::ASYNC_RETENTION_BATCH_SIZE) at the call site,
    /// clamped to `[1, 100]` there (pi `:656`).
    pub budget: usize,
}

/// One pass's output.
#[derive(Clone, Debug)]
pub struct RunScanWindow {
    /// At most `budget` candidates, in byte order, with every fact
    /// [`decide`](super::decide) needs already gathered.
    pub candidates: Vec<RunRetentionFacts>,
    /// The cursor value to persist for the next pass — pi `cursor.runAfter` after `:66-68`. The
    /// last candidate returned, or the previous cursor when the window came back empty, or `None`
    /// when the cursor was cleared and there was nothing to replace it with.
    ///
    /// Persisting it is SCOPE_14's (`CURSOR_NAME`, `:21`); producing it is this module's.
    pub next_after: Option<String>,
    /// `rawReads` (`AsyncRetentionResult::rawReads`, `:112`) — EVERY entry the listing returned,
    /// including the ones the usability filter rejected. This is the honest cost of a pass.
    pub raw_reads: usize,
    /// `cursorCleared` (`worker.mjs:40`) — the cursor ran off the end of the name order and this
    /// window wrapped back to the smallest names.
    pub cursor_cleared: bool,
}

/// One cursor-windowed pass over the async root.
///
/// # Errors
///
/// Propagates any [`std::io::Error`] that
/// [`errno::is_ignorable_listing_error`](crate::background::result_index) does not classify as
/// ignorable — from opening the root, from advancing the listing, or from reading a candidate's
/// `status.json`. An absent or permission-denied root is an empty window and NOT an error.
pub async fn scan_run_candidates(request: RunScanRequest<'_>) -> std::io::Result<RunScanWindow> {
    let maintenance = request
        .maintenance_root
        .map_or_else(|| maintenance_root(request.async_root), Path::to_path_buf);

    // pi `:23` — `limit <= 0` short-circuits to an empty, exhausted window with the cursor
    // untouched.
    if request.budget == 0 {
        return Ok(RunScanWindow {
            candidates: Vec::new(),
            next_after: request.after.map(str::to_string),
            raw_reads: 0,
            cursor_cleared: false,
        });
    }

    let mut entries = match tokio::fs::read_dir(request.async_root).await {
        Ok(entries) => entries,
        Err(error) if errno::is_ignorable_listing_error(&error) => {
            return Ok(RunScanWindow {
                candidates: Vec::new(),
                next_after: request.after.map(str::to_string),
                raw_reads: 0,
                cursor_cleared: false,
            });
        }
        Err(error) => return Err(error),
    };

    let mut wrapped: Vec<String> = Vec::new();
    let mut next: Vec<String> = Vec::new();
    let mut raw_reads = 0usize;
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(error) if errno::is_ignorable_listing_error(&error) => break,
            Err(error) => return Err(error),
        };
        raw_reads += 1;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // pi `worker.mjs:63` — `entry.isDirectory()`, which uses the dirent type and is therefore
        // FALSE for a symlink to a directory. `DirEntry::file_type` has the same semantics.
        match entry.file_type().await {
            Ok(file_type) if file_type.is_dir() => {}
            Ok(_) => continue,
            Err(error) if errno::is_ignorable_listing_error(&error) => continue,
            Err(error) => return Err(error),
        }
        if !is_run_candidate_name(&name) {
            continue;
        }
        if request.after.is_none_or(|after| name.as_str() > after) {
            insert_smallest(&mut next, name.clone(), request.budget);
        }
        insert_smallest(&mut wrapped, name, request.budget);
    }

    // pi `worker.mjs:40` — the cursor ran off the end, so wrap.
    let cursor_cleared = request.after.is_some() && next.is_empty();
    let selected = if cursor_cleared { wrapped } else { next };

    // pi `worker.mjs:66-68`: clear the cursor first if it wrapped, then advance it to the LAST
    // candidate returned. An empty window with a live cursor leaves the cursor alone.
    let mut next_after = if cursor_cleared {
        None
    } else {
        request.after.map(str::to_string)
    };
    if let Some(last) = selected.last() {
        next_after = Some(last.clone());
    }

    let mut candidates = Vec::with_capacity(selected.len());
    for name in selected {
        candidates.push(gather_facts(&request, &maintenance, name).await?);
    }

    Ok(RunScanWindow {
        candidates,
        next_after,
        raw_reads,
        cursor_cleared,
    })
}

/// Re-gather every fact for ONE async-root entry, by name.
///
/// The sweep needs this twice per candidate, and neither call can come from the window:
///
/// * after the `running`-run repair hop (pi `:768-779`), because
///   [`reconcile_now`](crate::background::reconcile::reconcile_now) may have REWRITTEN
///   `status.json` — deciding against the pre-repair status would reap on a state the repair just
///   changed, or spare a run the repair just made terminal;
/// * after the rename (pi `:817-818`), where the facts must be re-read from the TOMBSTONE path,
///   which did not exist when the window was built.
///
/// Same probes, same order, same errno policy as one entry of
/// [`scan_run_candidates`] — it is literally the same function, exposed. A second gatherer would
/// be a second, silently-diverging probe table, and the purity of
/// [`decide`](super::decide) rests on there being exactly one.
///
/// # Errors
///
/// As [`scan_run_candidates`]: an absent or permission-denied `status.json` is `None` inside the
/// facts, and any other fault propagates.
pub async fn run_candidate_facts(
    request: RunScanRequest<'_>,
    dir_name: &str,
) -> std::io::Result<RunRetentionFacts> {
    let maintenance = request
        .maintenance_root
        .map_or_else(|| maintenance_root(request.async_root), Path::to_path_buf);
    gather_facts(&request, &maintenance, dir_name.to_string()).await
}

/// pi `worker.mjs:63`'s usability filter, widened to cyrup's reserved-name vocabulary.
///
/// Upstream excludes only `ACTIVE_RUN_INDEX_DIR`, because its `runSkipReason` would answer
/// `invalid-status` (a KEEP) for `.terminal-runs` anyway. cyrup asks
/// [`is_reserved_async_root_entry`] instead, so `.terminal-runs`, `.active-runs` and
/// `.async-retention` are all excluded by the one predicate that owns that vocabulary — and then
/// carves the tombstone prefix back IN, because a `.deleting-run-*` tree is precisely the
/// candidate this pass exists to find (`worker.mjs:63` excludes only the active index).
fn is_run_candidate_name(name: &str) -> bool {
    name.starts_with(RUN_TOMBSTONE_PREFIX) || !is_reserved_async_root_entry(name)
}

/// pi `insertSmallest` (`worker.mjs:14-20`) — insert in byte order, then drop the tail past
/// `limit`, so the vector always holds the `limit` smallest names seen so far.
fn insert_smallest(entries: &mut Vec<String>, candidate: String, limit: usize) {
    if limit == 0 {
        return;
    }
    let index = entries.partition_point(|entry| entry.as_str() < candidate.as_str());
    entries.insert(index, candidate);
    if entries.len() > limit {
        entries.truncate(limit);
    }
}

/// Every row of `runSkipReason`'s probe table, performed once, here, so the policy stays pure.
async fn gather_facts(
    request: &RunScanRequest<'_>,
    maintenance: &Path,
    dir_name: String,
) -> std::io::Result<RunRetentionFacts> {
    let run_dir = normalize_tombstone_path(&request.async_root.join(&dir_name));
    let status_path = RunDir::for_existing(&run_dir).status();
    let status = read_status(&status_path).await?;

    let timestamp = match status.as_ref() {
        Some(status) => status_timestamp(status, &run_dir, &status_path).await,
        None => None,
    };
    let tombstone_mtime = if dir_name.starts_with(RUN_TOMBSTONE_PREFIX) {
        mtime_millis(&run_dir).await
    } else {
        None
    };

    let (marker_state, active_marker, mission_reference, unresolved_handoff, resumable) =
        match status.as_ref() {
            Some(status) => (
                read_run_tombstone_marker_self_healing(maintenance, &status.run_id).await,
                active_marker_exists(request.async_root, &status.run_id).await,
                mission_reference(&run_dir, request.results_dir, &status.run_id).await,
                has_unresolved_run_handoff(&run_dir).await,
                has_resumable_contract(&run_dir, status).await,
            ),
            // pi never reaches any of these probes without a status (`:289` returns first), so
            // their values are immaterial; they are the identity element of each guard.
            None => (TombstoneMarkerState::Absent, false, false, false, false),
        };

    Ok(RunRetentionFacts {
        dir_name,
        run_dir,
        status,
        timestamp,
        tombstone_mtime,
        marker_state,
        active_marker,
        mission_reference,
        unresolved_handoff,
        resumable,
    })
}

/// pi `readStatus` (`:148-157`) — via [`RunDir::status`], never a re-spelled `"status.json"`.
///
/// An absent or permission-denied `status.json` is `None` (upstream's `readJson` catch); a
/// genuine fault propagates, which is what makes `scan_run_candidates` fallible.
async fn read_status(status_path: &Path) -> std::io::Result<Option<RunStatus>> {
    match tokio::fs::read(status_path).await {
        Ok(bytes) => Ok(serde_json::from_slice::<RunStatus>(&bytes).ok()),
        Err(error) if errno::is_ignorable_listing_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// pi `statusTimestamp` (`:174-185`): `max(ended_at ?? last_update, max(mtime(run_dir),
/// mtime(run_dir/status.json)))`, and `None` if EITHER `stat` fails — which becomes
/// [`SkipReason::UnknownAge`](super::SkipReason::UnknownAge), a keep.
async fn status_timestamp(status: &RunStatus, run_dir: &Path, status_path: &Path) -> Option<i64> {
    let dir_mtime = mtime_millis(run_dir).await?;
    let status_mtime = mtime_millis(status_path).await?;
    let logical = status.ended_at.unwrap_or(status.last_update);
    Some(logical.max(dir_mtime.max(status_mtime)))
}

/// A path's mtime in epoch milliseconds, through [`crate::time::epoch_millis`] — the crate's one
/// `SystemTime` → `i64` conversion. `None` when the `stat` or the conversion fails, which every
/// caller resolves to KEEP.
async fn mtime_millis(path: &Path) -> Option<i64> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    metadata.modified().ok().map(crate::time::epoch_millis)
}

/// pi `activeMarkerExists` (`:205-207`) — `<async_root>/.active-runs/<runId>`.
///
/// A real probe, never a hard-coded `false`: [`crate::background::active_run_index`] IS ported and
/// writes exactly this marker for every queued-or-running run, so this is a live liveness guard,
/// not a placeholder.
async fn active_marker_exists(async_root: &Path, run_id: &RunId) -> bool {
    let marker = async_root
        .join(crate::background::active_run_index::ACTIVE_RUN_INDEX_DIR)
        .join(run_id.as_str());
    tokio::fs::try_exists(&marker).await.unwrap_or(false)
}

/// pi `:298` — `<run_dir>/mission.json` exists, OR `missionObserverIndexExists` (`:209-211`).
///
/// The second half is a READ of `<results_dir>/result-index/observers/mission/<enc(runId)>.json`,
/// addressed through [`crate::background::result_index`]'s own path builder. Reading a
/// results-side index is not a breach of this module's boundary; deleting one would be.
async fn mission_reference(run_dir: &Path, results_dir: &Path, run_id: &RunId) -> bool {
    if tokio::fs::try_exists(run_dir.join(MISSION_BINDING_FILE))
        .await
        .unwrap_or(false)
    {
        return true;
    }
    tokio::fs::try_exists(crate::background::result_index::mission_observer_path(
        results_dir,
        run_id,
    ))
    .await
    .unwrap_or(false)
}

/// pi `existingRegularFile` (`:160-168`) — and its ERROR POLARITY, which is a correctness
/// property: a non-`ENOENT` `lstat` failure returns **`true`**. "I could not tell" resolves to
/// "protected", never to "reapable".
async fn existing_regular_file(path: Option<&Path>) -> bool {
    let Some(path) = path else {
        return false;
    };
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata.is_file() && !metadata.file_type().is_symlink(),
        Err(error) => !errno::is_absent(&error),
    }
}

/// pi `hasResumableContract` (`:191-203`).
///
/// [CYRUP-DELTA] no cyrup writer produces `recovery-descriptor.json` today
/// (`grep -rn "recovery.descriptor" src/ --include=*.rs` finds only two prose mentions of pi's
/// own). The probe is ported anyway rather than hard-coded to `false`, for
/// [`active_marker_exists`]'s reason: it costs one `stat`, it is correct either way, and hiding
/// the guard behind a constant is the change that is silently wrong the day a writer lands.
async fn has_resumable_contract(run_dir: &Path, status: &RunStatus) -> bool {
    if existing_regular_file(status.session_file.as_deref()).await {
        return true;
    }
    for step in &status.steps {
        if existing_regular_file(step.session_file.as_deref()).await {
            return true;
        }
    }
    let descriptor_path = run_dir.join(RECOVERY_DESCRIPTOR_FILE);
    if !tokio::fs::try_exists(&descriptor_path)
        .await
        .unwrap_or(false)
    {
        return false;
    }
    let Some(descriptor) = read_json_object(&descriptor_path).await else {
        // pi `if (!descriptor) return true` (`:198`) — a present-but-unreadable descriptor is a
        // resumable contract this pass cannot disprove.
        return true;
    };
    if descriptor
        .get("sourceRunId")
        .and_then(serde_json::Value::as_str)
        != Some(status.run_id.as_str())
    {
        return true;
    }
    existing_regular_file(
        descriptor
            .get("sessionFile")
            .and_then(serde_json::Value::as_str)
            .map(Path::new),
    )
    .await
}

/// pi `hasUnresolvedRunHandoff` (`:268-277`) + `unresolvedHandoff` (`:258-266`).
///
/// [CYRUP-DELTA] [`RunStatus`] has no `parallel_handoff` field, so upstream's first path source
/// (`status.parallelHandoff.path`, `:270-273`) has nothing to read on this side and is not
/// simulated. The local `<run_dir>/handoff.json` half is ported in full, on
/// [`has_resumable_contract`]'s reasoning: no cyrup writer produces the file today, the probe is
/// one `stat`, and a guard written as a constant `false` is the one that fails silently later.
async fn has_unresolved_run_handoff(run_dir: &Path) -> bool {
    let manifest_path = run_dir.join(HANDOFF_MANIFEST_FILE);
    if !tokio::fs::try_exists(&manifest_path).await.unwrap_or(false) {
        return false;
    }
    let Some(manifest) = read_json_object(&manifest_path).await else {
        return true;
    };
    if manifest.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return true;
    }
    let Some(groups) = manifest.get("groups").and_then(serde_json::Value::as_array) else {
        return true;
    };
    if groups.is_empty() {
        return true;
    }
    groups.iter().any(|group| {
        group
            .get("cleanup")
            .and_then(serde_json::Value::as_object)
            .and_then(|cleanup| cleanup.get("state"))
            .and_then(serde_json::Value::as_str)
            != Some("complete")
    })
}

/// pi `readJson` (`:139-146`) — a JSON OBJECT or nothing. An array, a scalar, unreadable bytes and
/// malformed JSON all collapse to `None`, and every caller resolves `None` to a keep.
async fn read_json_object(path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let bytes = tokio::fs::read(path).await.ok()?;
    match serde_json::from_slice::<serde_json::Value>(&bytes).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
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

    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::super::{
        ASYNC_RETENTION_MS, ASYNC_RETENTION_TOMBSTONE_GRACE_MS, RetentionDecision, SkipReason,
        decide, write_run_tombstone_marker,
    };
    use super::*;
    use crate::background::{RunMode, RunState};

    struct Fixture {
        _temp: tempfile::TempDir,
        async_root: PathBuf,
        results_dir: PathBuf,
    }

    impl Fixture {
        async fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let async_root = temp.path().join("async");
            let results_dir = temp.path().join("results");
            tokio::fs::create_dir_all(&async_root).await.unwrap();
            tokio::fs::create_dir_all(&results_dir).await.unwrap();
            Self {
                _temp: temp,
                async_root,
                results_dir,
            }
        }

        fn request(&self, after: Option<&'static str>, budget: usize) -> RunScanRequest<'_> {
            RunScanRequest {
                async_root: &self.async_root,
                results_dir: &self.results_dir,
                maintenance_root: None,
                after,
                budget,
            }
        }

        async fn scan(&self, after: Option<&'static str>, budget: usize) -> RunScanWindow {
            scan_run_candidates(self.request(after, budget))
                .await
                .unwrap()
        }

        async fn plain_dir(&self, name: &str) {
            tokio::fs::create_dir_all(self.async_root.join(name))
                .await
                .unwrap();
        }

        /// A run directory with a real, terminal `status.json` written through the same type the
        /// runner writes.
        async fn run_dir(&self, dir_name: &str, run_token: &str, ended_at: i64) -> PathBuf {
            let dir = self.async_root.join(dir_name);
            tokio::fs::create_dir_all(&dir).await.unwrap();
            let mut status = RunStatus::queued(
                RunId::from_token(run_token.to_string()),
                RunMode::Single,
                Some(7),
            );
            status.state = RunState::Complete;
            status.started_at = ended_at;
            status.last_update = ended_at;
            status.ended_at = Some(ended_at);
            tokio::fs::write(
                RunDir::for_existing(&dir).status(),
                serde_json::to_vec_pretty(&status).unwrap(),
            )
            .await
            .unwrap();
            dir
        }
    }

    fn names(window: &RunScanWindow) -> Vec<String> {
        window
            .candidates
            .iter()
            .map(|facts| facts.dir_name.clone())
            .collect()
    }

    /// The budget bounds the OUTPUT, and the window is the byte-order SMALLEST names above the
    /// cursor — not "whatever `read_dir` handed back first".
    #[tokio::test]
    async fn the_scan_yields_at_most_a_batch_at_a_time() {
        let fixture = Fixture::new().await;
        let budget = 4usize;
        for index in 0..budget + 5 {
            fixture.plain_dir(&format!("run-{index:02}")).await;
        }

        let window = fixture.scan(None, budget).await;
        assert_eq!(window.candidates.len(), budget);
        assert_eq!(
            names(&window),
            vec!["run-00", "run-01", "run-02", "run-03"],
            "the smallest `budget` names in BYTE order"
        );
        assert_eq!(
            window.raw_reads,
            budget + 5,
            "rawReads counts the whole listing — the budget never shortens the read"
        );
        assert!(!window.cursor_cleared);
        assert_eq!(window.next_after.as_deref(), Some("run-03"));

        // The next pass resumes strictly after the cursor.
        let second = fixture.scan(Some("run-03"), budget).await;
        assert_eq!(names(&second), vec!["run-04", "run-05", "run-06", "run-07"]);
        assert_eq!(second.next_after.as_deref(), Some("run-07"));
    }

    /// Without the wrap-around the scan sticks at the end of the name order forever and never
    /// revisits an early directory — the single most likely silent defect in a windowed scan.
    #[tokio::test]
    async fn the_scan_wraps_when_the_cursor_runs_off_the_end() {
        let fixture = Fixture::new().await;
        for index in 0..4 {
            fixture.plain_dir(&format!("run-{index:02}")).await;
        }

        let window = fixture.scan(Some("zzzz-past-every-entry"), 2).await;
        assert!(window.cursor_cleared, "the cursor ran off the end");
        assert_eq!(
            names(&window),
            vec!["run-00", "run-01"],
            "and the window restarts at the smallest names"
        );
        assert_eq!(window.next_after.as_deref(), Some("run-01"));

        // An empty root with a live cursor clears it and leaves nothing to replace it with.
        let empty = Fixture::new().await;
        let window = empty.scan(Some("anything"), 2).await;
        assert!(window.cursor_cleared);
        assert!(window.next_after.is_none());
        assert!(window.candidates.is_empty());
    }

    /// `.terminal-runs` / `.active-runs` / `.async-retention` are not runs. A `.deleting-run-*`
    /// entry IS a candidate — it is the tombstone this pass exists to reap.
    #[tokio::test]
    async fn the_scan_skips_reserved_async_root_entries() {
        let fixture = Fixture::new().await;
        fixture.plain_dir(".terminal-runs").await;
        fixture.plain_dir(".active-runs").await;
        fixture.plain_dir(".async-retention").await;
        fixture.plain_dir(".deleting-run-abc").await;
        fixture.plain_dir("0123456789abcdef").await;
        // Regular files are not candidates either (`worker.mjs:63`'s `isDirectory()`).
        tokio::fs::write(fixture.async_root.join("stray.json"), b"{}")
            .await
            .unwrap();

        let window = fixture.scan(None, 100).await;
        assert_eq!(
            names(&window),
            vec![".deleting-run-abc", "0123456789abcdef"]
        );
        assert_eq!(
            window.raw_reads, 6,
            "every entry is still READ — the filter bounds the window, not the listing"
        );
    }

    /// An entry whose `status.json` cannot be read is still OFFERED, with `status: None`, and the
    /// policy keeps it as `invalid-status`. It is never dropped from the window (dropping it would
    /// make it invisible to a pass that could otherwise report it) and the fault never aborts the
    /// scan.
    ///
    /// The absent-status half is unconditional; the EACCES half is the same property reached
    /// through a different errno, and is asserted only where the mode bits are actually enforced —
    /// a root test runtime bypasses them, so it is probed rather than assumed (`#![forbid(unsafe_code)]`
    /// rules out a `geteuid` call).
    #[tokio::test]
    async fn an_entry_whose_status_cannot_be_read_is_offered_as_invalid_status() {
        use std::os::unix::fs::PermissionsExt as _;

        let keep_reason = |facts: &RunRetentionFacts| {
            decide(
                facts,
                1_700_000_000_000,
                ASYNC_RETENTION_MS,
                ASYNC_RETENTION_TOMBSTONE_GRACE_MS,
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
        };

        let fixture = Fixture::new().await;
        let dir = fixture.run_dir("locked", "locked", 1).await;
        // No `status.json` at all — deterministic on every runtime, root or not.
        fixture.plain_dir("headless").await;
        fixture.plain_dir("readable").await;

        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000))
            .await
            .unwrap();
        let enforced = tokio::fs::read(RunDir::for_existing(&dir).status())
            .await
            .is_err();
        let window = fixture.scan(None, 100).await;
        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .await
            .unwrap();

        assert_eq!(names(&window), vec!["headless", "locked", "readable"]);

        let headless = &window.candidates[0];
        assert_eq!(headless.dir_name, "headless");
        assert!(
            headless.status.is_none(),
            "an absent status.json is `None`, not a propagated error"
        );
        assert_eq!(
            keep_reason(headless),
            RetentionDecision::Keep(SkipReason::InvalidStatus),
            "and a status this pass cannot read is never a reap"
        );

        if enforced {
            let locked = &window.candidates[1];
            assert_eq!(locked.dir_name, "locked");
            assert!(
                locked.status.is_none(),
                "the permission fault was swallowed, not propagated"
            );
            assert_eq!(
                keep_reason(locked),
                RetentionDecision::Keep(SkipReason::InvalidStatus)
            );
        }
    }

    /// The other half of the errno policy: an error that is neither absent nor access-denied is a
    /// genuine fault and must ESCAPE — the direct contrast with
    /// `completion_replay::retention`, which returns on any error.
    #[tokio::test]
    async fn the_scan_propagates_a_genuine_io_fault() {
        let temp = tempfile::tempdir().unwrap();
        let loop_a = temp.path().join("loop-a");
        let loop_b = temp.path().join("loop-b");
        std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
        std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();

        let results_dir = temp.path().join("results");
        let error = scan_run_candidates(RunScanRequest {
            async_root: &loop_a,
            results_dir: &results_dir,
            maintenance_root: None,
            after: None,
            budget: 10,
        })
        .await
        .expect_err("ELOOP is not an ignorable listing error");
        // Naming the errno, not merely "some error": `scan_run_candidates` returns `Err` ONLY on
        // the not-ignorable arm, so re-asserting `!is_ignorable_listing_error` proves nothing the
        // control flow did not already guarantee. What is worth pinning is that the fault which
        // escaped is the UNDERLYING one this test induced — reported verbatim rather than
        // re-synthesised — because `compact_error` puts it straight into the operator-facing
        // report. (`ErrorKind::FilesystemLoop` is still unstable, so the raw errno is the portable
        // way to say "the same fault".)
        let induced = std::fs::read_dir(&loop_a).expect_err("the cycle really is a cycle");
        assert!(induced.raw_os_error().is_some());
        assert_eq!(
            error.raw_os_error(),
            induced.raw_os_error(),
            "the scan propagated the listing's own errno unchanged: {error}"
        );
        assert!(
            !crate::background::result_index::errno::is_ignorable_listing_error(&error),
            "and it is not one this crate would have swallowed anywhere else: {error}"
        );
    }

    /// An absent async root is an empty window, not an error (`worker.mjs:43`), and it leaves the
    /// cursor untouched.
    #[tokio::test]
    async fn an_absent_async_root_is_an_empty_window_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let window = scan_run_candidates(RunScanRequest {
            async_root: &temp.path().join("never-created"),
            results_dir: temp.path(),
            maintenance_root: None,
            after: Some("run-03"),
            budget: 10,
        })
        .await
        .unwrap();
        assert!(window.candidates.is_empty());
        assert_eq!(window.raw_reads, 0);
        assert!(!window.cursor_cleared);
        assert_eq!(window.next_after.as_deref(), Some("run-03"));
    }

    /// The whole point of the scan: every probe `runSkipReason` does inline is performed HERE, so
    /// that the policy can be pure. This drives the four hoisted facts through their real
    /// filesystem sources and checks each one flips the decision it is supposed to.
    #[tokio::test]
    async fn the_scan_gathers_every_probe_the_pure_policy_needs() {
        let now = 1_900_000_000_000i64;
        let old = now - ASYNC_RETENTION_MS - 60_000;
        let none = BTreeSet::new();
        let at = |window: &RunScanWindow, name: &str| -> RunRetentionFacts {
            window
                .candidates
                .iter()
                .find(|facts| facts.dir_name == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} was not a candidate"))
        };
        let verdict = |facts: &RunRetentionFacts| {
            decide(
                facts,
                now,
                ASYNC_RETENTION_MS,
                ASYNC_RETENTION_TOMBSTONE_GRACE_MS,
                &none,
                &none,
            )
        };

        let fixture = Fixture::new().await;
        fixture.run_dir("plain", "plain", old).await;

        // Row 6 — the active-run index marker, written by `active_run_index` for every
        // queued-or-running run.
        fixture.run_dir("active", "active", old).await;
        let active_dir = fixture
            .async_root
            .join(crate::background::active_run_index::ACTIVE_RUN_INDEX_DIR);
        tokio::fs::create_dir_all(&active_dir).await.unwrap();
        tokio::fs::write(active_dir.join("active"), b"")
            .await
            .unwrap();

        // Row 10 — the mission binding file.
        let mission = fixture.run_dir("mission", "mission", old).await;
        tokio::fs::write(mission.join(MISSION_BINDING_FILE), b"{}")
            .await
            .unwrap();

        // Row 10, second half — the mission OBSERVER index in the results dir.
        fixture.run_dir("observed", "observed", old).await;
        let observer = crate::background::result_index::mission_observer_path(
            &fixture.results_dir,
            &RunId::from_token("observed".to_string()),
        );
        tokio::fs::create_dir_all(observer.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&observer, b"{}").await.unwrap();

        // Row 11 — an unresolved handoff manifest.
        let handoff = fixture.run_dir("handoff", "handoff", old).await;
        tokio::fs::write(
            handoff.join("handoff.json"),
            br#"{"version":1,"groups":[{"cleanup":{"state":"pending"}}]}"#,
        )
        .await
        .unwrap();

        // Row 12 — a session transcript that still exists.
        let resumable_dir = fixture.async_root.join("resumable");
        tokio::fs::create_dir_all(&resumable_dir).await.unwrap();
        let transcript = resumable_dir.join("session.jsonl");
        tokio::fs::write(&transcript, b"{}").await.unwrap();
        let mut status = RunStatus::queued(
            RunId::from_token("resumable".to_string()),
            RunMode::Single,
            Some(7),
        );
        status.state = RunState::Complete;
        status.started_at = old;
        status.last_update = old;
        status.ended_at = Some(old);
        status.session_file = Some(transcript);
        tokio::fs::write(
            RunDir::for_existing(&resumable_dir).status(),
            serde_json::to_vec_pretty(&status).unwrap(),
        )
        .await
        .unwrap();

        let window = fixture.scan(None, 100).await;

        // `plain` is the control: nothing protects it, so it retires.
        let plain = at(&window, "plain");
        assert!(plain.status.is_some());
        assert!(plain.timestamp.is_some(), "statusTimestamp resolved");
        assert!(plain.run_dir.is_absolute(), "run_dir is pre-absolutised");
        assert_eq!(verdict(&plain), RetentionDecision::Tombstone);

        assert!(at(&window, "active").active_marker);
        assert_eq!(
            verdict(&at(&window, "active")),
            RetentionDecision::Keep(SkipReason::ActiveIndex)
        );
        assert_eq!(
            verdict(&at(&window, "mission")),
            RetentionDecision::Keep(SkipReason::MissionReference)
        );
        assert_eq!(
            verdict(&at(&window, "observed")),
            RetentionDecision::Keep(SkipReason::MissionReference)
        );
        assert_eq!(
            verdict(&at(&window, "handoff")),
            RetentionDecision::Keep(SkipReason::HandoffReference)
        );
        assert_eq!(
            verdict(&at(&window, "resumable")),
            RetentionDecision::Keep(SkipReason::Resumable)
        );
    }

    /// `statusTimestamp` (`:174-185`) is the MAX of the logical and physical clocks, so a run
    /// whose `status.json` was touched a moment ago is never old, whatever its `ended_at` claims.
    #[tokio::test]
    async fn the_physical_mtime_floors_a_backdated_status() {
        let fixture = Fixture::new().await;
        fixture.run_dir("backdated", "backdated", 1).await;
        let window = fixture.scan(None, 10).await;
        let facts = &window.candidates[0];
        let timestamp = facts.timestamp.unwrap();
        assert!(
            timestamp > 1_600_000_000_000,
            "the file was just written, so the physical mtime dominates: {timestamp}"
        );
        assert_eq!(
            decide(
                facts,
                crate::time::now_epoch_millis(),
                ASYNC_RETENTION_MS,
                ASYNC_RETENTION_TOMBSTONE_GRACE_MS,
                &BTreeSet::new(),
                &BTreeSet::new()
            ),
            RetentionDecision::Keep(SkipReason::Recent)
        );
    }

    /// The tombstone half, end to end through the real marker format: a `.deleting-run-*` tree
    /// with a matching marker and an aged mtime decides `Reap`, and the same tree with no marker
    /// decides `Keep(RunTombstoneMarker)`.
    #[tokio::test]
    async fn a_tombstone_candidate_carries_its_marker_state() {
        let fixture = Fixture::new().await;
        let tombstone = fixture.run_dir(".deleting-run-abc", "retired", 1).await;
        let maintenance = super::super::maintenance_root(&fixture.async_root);
        let run_id = RunId::from_token("retired".to_string());
        let none = BTreeSet::new();
        // A minute in the future with a ZERO retention window, so the tree's just-written mtime
        // can never read as `recent` and this test isolates the marker/grace decision alone.
        let now = crate::time::now_epoch_millis() + 60_000;
        let retention = 0i64;

        let window = fixture.scan(None, 10).await;
        let facts = &window.candidates[0];
        assert_eq!(facts.dir_name, ".deleting-run-abc");
        assert!(
            facts.tombstone_mtime.is_some(),
            "a tombstone-prefixed entry carries its own mtime"
        );
        assert_eq!(facts.marker_state, TombstoneMarkerState::Absent);
        assert_eq!(
            decide(facts, now, retention, 0, &none, &none),
            RetentionDecision::Keep(SkipReason::RunTombstoneMarker),
            "no marker means nothing maps the run id back to this tree, so it is never reaped"
        );

        write_run_tombstone_marker(&maintenance, &run_id, &tombstone, now)
            .await
            .unwrap();
        let window = fixture.scan(None, 10).await;
        let facts = &window.candidates[0];
        assert!(matches!(
            facts.marker_state,
            TombstoneMarkerState::Present { .. }
        ));
        // A zero grace makes "past the grace" unconditional, which isolates the marker check.
        assert_eq!(
            decide(facts, now, retention, 0, &none, &none),
            RetentionDecision::Reap
        );
        assert_eq!(
            decide(
                facts,
                now,
                retention,
                ASYNC_RETENTION_TOMBSTONE_GRACE_MS,
                &none,
                &none
            ),
            RetentionDecision::Keep(SkipReason::TombstoneGrace),
            "a tombstone minted moments ago is inside the 24 h hysteresis"
        );
    }

    /// The self-heal runs from the SCAN, which is what keeps the markers directory bounded: a
    /// marker naming a tree that no longer exists is unlinked the next time the scan meets its
    /// run id.
    #[tokio::test]
    async fn the_scan_self_heals_a_marker_whose_tree_has_vanished() {
        let fixture = Fixture::new().await;
        fixture.run_dir("survivor", "survivor", 1).await;
        let maintenance = super::super::maintenance_root(&fixture.async_root);
        let run_id = RunId::from_token("survivor".to_string());
        write_run_tombstone_marker(
            &maintenance,
            &run_id,
            &fixture.async_root.join(".deleting-run-long-gone"),
            1,
        )
        .await
        .unwrap();
        let marker = super::super::run_tombstone_marker_path(&maintenance, &run_id);
        assert!(marker.exists());

        let window = fixture.scan(None, 10).await;
        assert_eq!(
            window.candidates[0].marker_state,
            TombstoneMarkerState::Absent
        );
        assert!(!marker.exists(), "the dangling marker was unlinked");
    }

    /// A zero budget is an empty, cursor-preserving window (`worker.mjs:23`).
    #[tokio::test]
    async fn a_zero_budget_returns_an_empty_window_with_the_cursor_intact() {
        let fixture = Fixture::new().await;
        fixture.plain_dir("run-00").await;
        let window = fixture.scan(Some("run-00"), 0).await;
        assert!(window.candidates.is_empty());
        assert_eq!(window.raw_reads, 0);
        assert!(!window.cursor_cleared);
        assert_eq!(window.next_after.as_deref(), Some("run-00"));
    }
}
