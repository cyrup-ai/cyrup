//! Resolving an index entry to the payload it points at.
//!
//! Ports pi `resultPayloadLocationFromIndex` (`result-files.ts:306-316`),
//! `pendingResultLocationForSessionRun` (`:293-299`), `readResultIndexForSessionRun` (`:317-333`)
//! and the `resultPayloadPathFor*` family (`:334-373`).

use std::path::{Path, PathBuf};

use crate::background::RunId;
use crate::identity::{ResultFileName, SessionId};

use super::entry::ResultIndexEntry;
use super::errno;
use super::exists;
use super::paths;
use super::promote::{self, PromotionState};

/// Where a payload actually is.
///
/// pi's `{ state: "public" | "pending" }` (`result-files.ts:178`) with the third case this port
/// needs. **Distinct from [`PromotionState`]**, which describes an *attempt*: conflating the two
/// loses the `None` case, which is the one that means "the payload vanished".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadState {
    /// Under `result-owned/<enc(session)>/` — where every payload this build promotes lands.
    /// Reachable through the index; invisible to a reader that lists the results root.
    Owned,
    /// At `<results_dir>/<runId>.json` — written by a build predating the owned directory. Still
    /// read and still consumed exactly once by its owner, and still stealable by an index-blind
    /// process: transitional by nature, which is why nothing writes here any more.
    LegacyRoot,
    /// Still under `result-pending/<enc(session)>/` — a runner died before promoting. Visible only
    /// through the session index, and promoted by the first reader that resolves it.
    Staged,
}

/// Where one run's payload actually is right now.
///
/// pi's `ResultPayloadLocation` (`result-files.ts:288-292`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultPayloadLocation {
    /// The payload's file name — a validated single component.
    pub file: ResultFileName,
    /// The absolute path to read. Only [`PayloadState::LegacyRoot`] is derivable from `file`
    /// alone; the owned and staged paths are session-private and are not.
    pub path: PathBuf,
    /// Which of the three locations this came from.
    pub state: PayloadState,
}

/// A payload path the holder is entitled to DESTROY.
///
/// # Why this is a distinct type and not a `PathBuf`
///
/// Destroying a payload destroys the only copy of a background child's answer. The operation is
/// therefore gated on evidence rather than on discipline: [`ConsumablePayload`] is minted only by
/// [`resolve_payload`], and only when the payload was reached through the caller's OWN session
/// partition. The cross-session mission-observer lookup — the one place this subsystem
/// deliberately reads another instance's results — resolves to a plain [`ResultPayloadLocation`]
/// and can therefore observe a completion it has no value capable of deleting.
///
/// Not `Clone`: it authorises one destruction, and the consumer takes it by value.
#[derive(Debug)]
pub struct ConsumablePayload {
    path: PathBuf,
    session_id: SessionId,
    run_id: RunId,
    state: PayloadState,
}

impl ConsumablePayload {
    /// The ONLY mint, private to this module so the entitlement cannot be forged elsewhere.
    fn new(location: &ResultPayloadLocation, session_id: &SessionId, run_id: &RunId) -> Self {
        Self {
            path: location.path.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            state: location.state,
        }
    }

    /// The file to unlink.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The run this payload belongs to — checked against the delivery receipt at consumption, so
    /// a receipt for one run cannot authorise destroying another's payload.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// The owning session, needed to clear the index entries in the same operation.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Which location it was resolved from.
    #[must_use]
    pub fn state(&self) -> PayloadState {
        self.state
    }
}

/// The result of looking for one indexed run's payload.
///
/// # Three variants, and why the third is not folded into the second
///
/// `Option` would say only "found" or "not found", and this subsystem acts on the difference
/// between *absent* and *unreadable*: an absent payload that stays absent is a lost result, which
/// is announced to the orchestrator and whose index entries are retired; an `EACCES`/`EIO` on a
/// probe is a transient fault that proves nothing at all. Collapsing them would let a permissions
/// blip retire a live result's index and announce a loss that never happened.
#[derive(Debug)]
pub enum PayloadResolution {
    /// Found, and the caller is entitled to consume it.
    Found(ConsumablePayload),
    /// Every location was probed cleanly and none held the payload.
    Absent,
    /// A probe failed for a reason other than "not there". NOT evidence of loss.
    Unreadable(std::io::Error),
}

/// Resolve an index entry to its payload — **promoting it on the way if it is still staged**.
///
/// pi `resultPayloadLocationFromIndex` (`result-files.ts:306-316`).
///
/// # This reader mutates, on purpose
///
/// Calling a promotion from inside a read looks wrong and is not. A payload can be staged by a
/// runner process that then exits before promoting — a crash, a kill, or simply losing a race.
/// Nothing else would ever promote it, so the result would stay invisible at its public path
/// forever. Making every reader a potential promoter is what guarantees a staged payload
/// eventually becomes public: the first reader that cares does the work.
pub(crate) async fn result_payload_location_from_index(
    results_dir: &Path,
    entry: &ResultIndexEntry,
) -> Option<ResultPayloadLocation> {
    match locate_payload(results_dir, &entry.session_id, &entry.run_id, &entry.file).await {
        PayloadLookup::Found(location) => Some(location),
        PayloadLookup::Absent | PayloadLookup::Unreadable(_) => None,
    }
}

/// Resolve one indexed run's payload, and hand back the entitlement to consume it.
///
/// This is the ONE resolver every owning reader goes through. `result_payload_location_from_index`
/// is the same lookup for readers that only need to *see* a payload (the cross-session observer
/// band), and deliberately yields no [`ConsumablePayload`].
pub async fn resolve_payload(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
    file: &ResultFileName,
) -> PayloadResolution {
    match locate_payload(results_dir, session_id, run_id, file).await {
        PayloadLookup::Found(location) => {
            PayloadResolution::Found(ConsumablePayload::new(&location, session_id, run_id))
        }
        PayloadLookup::Absent => PayloadResolution::Absent,
        PayloadLookup::Unreadable(error) => PayloadResolution::Unreadable(error),
    }
}

/// Where this build promotes `run_id`'s payload for `session_id`.
///
/// The write location, exposed because "where does a promoted payload live" is a legitimate
/// question for diagnostics and for callers reasoning about the layout — not something they should
/// answer by re-deriving the encoding.
#[must_use]
pub fn owned_payload_path(results_dir: &Path, session_id: &SessionId, run_id: &RunId) -> PathBuf {
    paths::result_owned_path(results_dir, session_id, run_id)
}

/// The lookup ladder shared by both resolvers, without the entitlement decision.
enum PayloadLookup {
    Found(ResultPayloadLocation),
    Absent,
    Unreadable(std::io::Error),
}

/// Owned → legacy root → staged (promoting on the way).
///
/// The order is the write history. Owned is where every payload this build promotes lands, so it
/// is probed first and hits on the overwhelmingly common path. The root probe reads what older
/// builds wrote and is what keeps their results deliverable across the change. The staged probe is
/// upstream's crash-before-promotion repair, unchanged: a runner can die between staging and
/// promoting, and making every reader a potential promoter is what guarantees such a payload
/// eventually becomes reachable.
async fn locate_payload(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
    file: &ResultFileName,
) -> PayloadLookup {
    // pi `:307`'s traversal guard is discharged by `file` being a `ResultFileName`.
    let promotion =
        promote::promote_pending_result_file(results_dir, session_id, run_id, true).await;

    if promotion == PromotionState::Pending {
        // Promotion could not publish it; the staged copy is still the payload.
        return match pending_result_location(results_dir, session_id, run_id, file).await {
            Some(location) => PayloadLookup::Found(location),
            None => PayloadLookup::Absent,
        };
    }

    let owned_path = paths::result_owned_path(results_dir, session_id, run_id);
    if promotion == PromotionState::Promoted {
        // This call performed the rename, so the destination is known-good without a re-probe.
        return PayloadLookup::Found(ResultPayloadLocation {
            file: file.clone(),
            path: owned_path,
            state: PayloadState::Owned,
        });
    }

    let mut fault: Option<std::io::Error> = None;
    for (path, state) in owned_candidates(results_dir, session_id, run_id)
        .into_iter()
        .map(|path| (path, PayloadState::Owned))
        .chain(std::iter::once((
            file.resolve_in(results_dir),
            PayloadState::LegacyRoot,
        )))
    {
        match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => {
                return PayloadLookup::Found(ResultPayloadLocation {
                    file: file.clone(),
                    path,
                    state,
                });
            }
            Ok(_) => {}
            Err(error) if errno::is_absent(&error) => {}
            // Remember the fault and keep probing: another location may still hold the payload,
            // and only a clean sweep of absences may be reported as `Absent`.
            Err(error) => fault = Some(error),
        }
    }

    match fault {
        Some(error) => PayloadLookup::Unreadable(error),
        None => PayloadLookup::Absent,
    }
}

/// Every owned location this run's payload could be at, write location first.
fn owned_candidates(results_dir: &Path, session_id: &SessionId, run_id: &RunId) -> Vec<PathBuf> {
    paths::result_owned_paths(results_dir, session_id, run_id)
}

/// The staged location for a session/run, when the staged payload really is theirs.
///
/// pi `pendingResultLocationForSessionRun` (`result-files.ts:293-299`).
///
/// The payload is re-read and its `runId`/`sessionId` compared against what was asked for
/// (`pendingResultPayloadMatches`, `:244-252`). That check is not redundant: the staged path is
/// built from an *encoded* — and for long or non-portable ids, hashed — segment, so verifying the
/// contents is what makes a hashed address safe.
pub(crate) async fn pending_result_location(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
    file: &ResultFileName,
) -> Option<ResultPayloadLocation> {
    let pending_path = exists::first_existing(&paths::result_pending_paths(
        results_dir,
        session_id,
        run_id,
    ))
    .await?;
    if !pending_payload_matches(&pending_path, session_id, run_id).await {
        return None;
    }
    Some(ResultPayloadLocation {
        file: file.clone(),
        path: pending_path,
        state: PayloadState::Staged,
    })
}

/// pi `pendingResultPayloadMatches` (`result-files.ts:244-252`).
async fn pending_payload_matches(path: &Path, session_id: &SessionId, run_id: &RunId) -> bool {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            if !errno::is_absent(&error) {
                tracing::warn!(path = %path.display(), %error, "ignoring invalid pending async result");
            }
            return false;
        }
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        tracing::warn!(path = %path.display(), "ignoring unparseable pending async result");
        return false;
    };
    payload_run_id(&value).as_deref() == Some(run_id.as_str())
        && non_empty_str(value.get("sessionId")) == Some(session_id.as_str())
}

/// pi's `nonEmptyString(data.runId) ?? nonEmptyString(data.id)` (`result-files.ts:245`).
///
/// Both spellings are accepted because the result payload carries the run id twice — `id` and
/// `runId` — to preserve the upstream wire shape.
pub(crate) fn payload_run_id(value: &serde_json::Value) -> Option<String> {
    non_empty_str(value.get("runId"))
        .or_else(|| non_empty_str(value.get("id")))
        .map(str::to_string)
}

fn non_empty_str(value: Option<&serde_json::Value>) -> Option<&str> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Read the session index entry for one run, following the alias fan-out.
///
/// pi `readResultIndexForSessionRun` (`result-files.ts:317-333`).
///
/// # Errors
///
/// Rethrows a permission fault (`EPERM`/`EACCES`). A missing or malformed entry is `Ok(None)` —
/// see [`errno::is_access_denied`] for why enumeration and targeted reads differ here.
pub(crate) async fn read_result_index_for_session_run(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> std::io::Result<Option<ResultIndexEntry>> {
    for index_path in paths::result_index_paths(results_dir, session_id, run_id) {
        match tokio::fs::read(&index_path).await {
            Ok(bytes) => {
                if let Some(entry) = ResultIndexEntry::parse(&bytes)
                    && entry.describes(session_id, run_id)
                {
                    return Ok(Some(entry));
                }
            }
            Err(error) if errno::is_access_denied(&error) => return Err(error),
            Err(error) if errno::is_absent(&error) => {}
            Err(error) => {
                tracing::warn!(run_id = %run_id, %error, "ignoring invalid async result index");
            }
        }
    }
    Ok(None)
}

/// The payload path for a session/run, via the index and then the staged fallback.
///
/// pi `resultPayloadPathForSessionRun` (`result-files.ts:334-338`).
///
/// # Errors
///
/// As [`read_result_index_for_session_run`].
pub async fn result_payload_path_for_session_run(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> std::io::Result<Option<PathBuf>> {
    if let Some(entry) = read_result_index_for_session_run(results_dir, session_id, run_id).await?
        && let Some(location) = result_payload_location_from_index(results_dir, &entry).await
    {
        return Ok(Some(location.path));
    }
    let file = ResultFileName::for_run(run_id);
    Ok(
        pending_result_location(results_dir, session_id, run_id, &file)
            .await
            .map(|location| location.path),
    )
}

/// The STAGED payload path for a session/run, skipping the index entirely.
///
/// pi `fallbackResultPayloadPathForSessionRun` (`result-files.ts:300-304`). Distinct from
/// [`result_payload_path_for_session_run`], which consults the index FIRST: this is the recovery
/// path for a caller whose index read failed with a permission fault
/// (`wait-completions.ts:175-184`), where the staged location is still reachable because it is
/// addressed directly rather than through the unreadable index directory.
///
/// Upstream's variant asserts the payload match through `assertPendingResultPayloadMatches`
/// (`result-files.ts:254-257`), which lets a read fault throw; cyrup reuses the swallowing
/// [`pending_result_location`] — and therefore `pending_payload_matches` (`locate.rs:288`) — so an
/// unreadable staged file degrades to "no fallback path" rather than to a second error on the
/// recovery path. That is the better behaviour on a path that exists *because* the first read
/// faulted, and it is the only observable difference from upstream.
///
/// Returns a plain [`PathBuf`], never a [`ConsumablePayload`]: this is a READ address for a wait,
/// and a wait must never be able to destroy a payload the watcher still owns.
#[must_use = "the recovered path is the whole point of calling this"]
pub async fn fallback_result_payload_path_for_session_run(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> Option<PathBuf> {
    let file = ResultFileName::for_run(run_id);
    pending_result_location(results_dir, session_id, run_id, &file)
        .await
        .map(|location| location.path)
}

/// The payload path for a run known only by id, via the run index.
///
/// pi `resultPayloadPathForIndexedRun` (`result-files.ts:352-372`), including its self-healing
/// behaviour: an entry that does not describe the run asked for, or whose payload has gone, is
/// **unlinked**. The run index is a pure accelerator, so a stale entry has no value and would
/// otherwise be re-read on every lookup.
pub async fn result_payload_path_for_indexed_run(
    results_dir: &Path,
    run_id: &RunId,
) -> Option<PathBuf> {
    let entry_path = paths::run_index_path(results_dir, run_id);
    let bytes = match tokio::fs::read(&entry_path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            if errno::is_unaddressable(&error) || errno::is_absent(&error) {
                return None;
            }
            tracing::warn!(run_id = %run_id, %error, "ignoring invalid async result run index");
            let _ = tokio::fs::remove_file(&entry_path).await;
            return None;
        }
    };

    let Some(entry) = ResultIndexEntry::parse(&bytes).filter(|entry| &entry.run_id == run_id)
    else {
        let _ = tokio::fs::remove_file(&entry_path).await;
        return None;
    };
    match result_payload_location_from_index(results_dir, &entry).await {
        Some(location) => Some(location.path),
        None => {
            let _ = tokio::fs::remove_file(&entry_path).await;
            None
        }
    }
}

/// The payload path for a mission-bound run, via the cross-session observer index.
///
/// pi `resultPayloadPathForMissionObserverRun` (`result-files.ts:340-350`). This is the **one**
/// lookup that deliberately crosses sessions: mission reconciliation must see a completion
/// regardless of which instance launched it, and serving that from a dedicated narrow index is
/// what makes it possible without widening the ownership predicate.
pub async fn result_payload_path_for_mission_observer_run(
    results_dir: &Path,
    run_id: &RunId,
) -> Option<PathBuf> {
    let bytes = match tokio::fs::read(paths::mission_observer_path(results_dir, run_id)).await {
        Ok(bytes) => bytes,
        Err(error) => {
            if !errno::is_absent(&error) {
                tracing::warn!(run_id = %run_id, %error, "ignoring invalid observer index");
            }
            return None;
        }
    };
    let entry = ResultIndexEntry::parse(&bytes).filter(|entry| &entry.run_id == run_id)?;
    result_payload_location_from_index(results_dir, &entry)
        .await
        .map(|location| location.path)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::write::{
        ResultWrite, write_async_result_file, write_pending_async_result_file,
    };
    use super::*;

    fn session(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        run_id: String,
        session_id: String,
    }

    fn payload(run: &str, sess: &str) -> Payload {
        Payload {
            run_id: run.to_string(),
            session_id: sess.to_string(),
        }
    }

    #[tokio::test]
    async fn a_public_payload_resolves_to_its_public_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");

        let found = result_payload_path_for_session_run(tmp.path(), &session("s1"), &run)
            .await
            .expect("no io error")
            .expect("found");
        assert_eq!(
            found,
            paths::result_owned_path(tmp.path(), &session("s1"), &run)
        );
    }

    #[tokio::test]
    async fn reading_a_staged_payload_promotes_it() {
        // The self-healing property: a runner that died before promoting does not strand the file.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_pending_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");
        assert!(!paths::result_owned_path(tmp.path(), &session("s1"), &run).exists());

        let found = result_payload_path_for_session_run(tmp.path(), &session("s1"), &run)
            .await
            .expect("no io error")
            .expect("found");
        assert_eq!(
            found,
            paths::result_owned_path(tmp.path(), &session("s1"), &run),
            "the read promoted it"
        );
        assert!(paths::result_owned_path(tmp.path(), &session("s1"), &run).is_file());
    }

    #[tokio::test]
    async fn a_foreign_session_cannot_resolve_another_sessions_run() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_pending_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");

        let found = result_payload_path_for_session_run(tmp.path(), &session("s2"), &run)
            .await
            .expect("no io error");
        assert!(found.is_none(), "s2 must not resolve s1's staged payload");
    }

    #[tokio::test]
    async fn a_staged_payload_whose_contents_disagree_is_rejected() {
        // Guards the hashed-address case: the path matched, the contents did not.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let staged = paths::result_pending_path(tmp.path(), &session("s1"), &run);
        tokio::fs::create_dir_all(staged.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(&staged, br#"{"runId":"OTHER","sessionId":"s1"}"#)
            .await
            .expect("seed");

        let file = ResultFileName::for_run(&run);
        assert!(
            pending_result_location(tmp.path(), &session("s1"), &run, &file)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_run_index_resolves_a_run_by_id_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");

        let found = result_payload_path_for_indexed_run(tmp.path(), &run)
            .await
            .expect("found");
        assert_eq!(
            found,
            paths::result_owned_path(tmp.path(), &session("s1"), &run)
        );
    }

    #[tokio::test]
    async fn a_stale_run_index_entry_is_unlinked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");
        // Delete the payload, leaving the index pointing at nothing.
        tokio::fs::remove_file(paths::result_owned_path(tmp.path(), &session("s1"), &run))
            .await
            .expect("rm");

        assert!(
            result_payload_path_for_indexed_run(tmp.path(), &run)
                .await
                .is_none()
        );
        assert!(
            !paths::run_index_path(tmp.path(), &run).exists(),
            "a stale accelerator entry must be swept, not re-read forever"
        );
    }

    #[tokio::test]
    async fn an_unparseable_index_entry_is_ignored_and_removed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let index = paths::run_index_path(tmp.path(), &run);
        tokio::fs::create_dir_all(index.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(&index, b"not json").await.expect("seed");

        assert!(
            result_payload_path_for_indexed_run(tmp.path(), &run)
                .await
                .is_none()
        );
        assert!(!index.exists());
    }

    #[tokio::test]
    async fn a_missing_index_is_quietly_none_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("nope");
        assert!(
            read_result_index_for_session_run(tmp.path(), &session("s1"), &run)
                .await
                .expect("a missing index is not an error")
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_mission_observer_index_resolves_across_sessions() {
        // The one deliberate cross-session lookup.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let async_dir = tmp.path().join("adir");
        tokio::fs::create_dir_all(&async_dir).await.expect("mkdir");
        tokio::fs::write(async_dir.join("mission.json"), b"{}")
            .await
            .expect("bind");

        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: Some(&async_dir),
                tool_call_id: None,
            },
            &payload("run1", "s1"),
        )
        .await
        .expect("write");

        let found = result_payload_path_for_mission_observer_run(tmp.path(), &run)
            .await
            .expect("a mission-bound run is visible to any instance");
        assert_eq!(
            found,
            paths::result_owned_path(tmp.path(), &session("s1"), &run)
        );
    }
}
