//! Index path builders.
//!
//! Ports pi `result-files.ts:7-16` (constants) and `:27-110` (the builders).
//!
//! # The `&IndexSegment` rule
//!
//! Every builder that crosses this module's boundary takes a **typed** identity
//! ([`SessionId`], [`RunId`], or an already-encoded [`IndexSegment`]) and never a bare `&str`. A
//! session id is routinely a full filesystem path, so `results_dir.join(session_id)` is a
//! directory traversal waiting to happen; making the encoder the only route to a path component
//! removes the shape of that mistake rather than relying on every caller to remember it.
//!
//! The private helpers below *do* take `&str` — they are the encoding layer itself, and the
//! `&str` stops at this file.
//!
//! # Singular for writes, plural for reads
//!
//! `*_path`/`*_dir` return the ONE location a write goes to. `*_paths`/`*_dirs` return the alias
//! fan-out a read must consider ([`IndexSegment::aliases`]). That asymmetry is what makes an
//! encoding change backward-compatible; see [`IndexSegment::aliases`] for the mechanism.

use std::path::{Path, PathBuf};

use crate::background::RunId;
use crate::identity::{IndexSegment, ResultFileName, SessionId};

/// pi `RESULT_INDEX_DIR` (`result-files.ts:8`).
pub(crate) const RESULT_INDEX_DIR: &str = "result-index";
/// pi `SESSION_INDEX_DIR` (`:9`).
pub(crate) const SESSION_INDEX_DIR: &str = "sessions";
/// pi `RUN_INDEX_DIR` (`:10`).
pub(crate) const RUN_INDEX_DIR: &str = "runs";
/// pi `OBSERVER_INDEX_DIR` (`:11`).
pub(crate) const OBSERVER_INDEX_DIR: &str = "observers";
/// pi `TOOL_CALL_INDEX_DIR` (`:12`).
pub(crate) const TOOL_CALL_INDEX_DIR: &str = "tool-calls";
/// pi `RESULT_PENDING_DIR` (`:13`).
pub(crate) const RESULT_PENDING_DIR: &str = "result-pending";
/// The promotion target: `<results_dir>/result-owned/<enc(session)>/<enc(run)>.json`.
///
/// [CYRUP-DELTA] upstream promotes to `<results_dir>/<runId>.json`, the listable root of a
/// directory keyed by **cwd** and therefore shared by every cyrup instance running there. That is
/// safe upstream and is not here: this repository's own history includes long-lived processes
/// running builds that predate the result index, which enumerate the root by `readdir` and consume
/// whatever they find. Nineteen completed runs' payloads were destroyed that way in a single
/// working day, each leaving its index entries behind as the fingerprint.
///
/// A session-partitioned promotion target ends that by construction rather than by asking every
/// concurrent process to behave: an index-blind reader listing the root now finds nothing at all,
/// while an index-driven reader reaches the payload exactly as before.
pub(crate) const RESULT_OWNED_DIR: &str = "result-owned";
/// pi `MISSION_OBSERVER` (`:14`) — the one observer band that exists today.
pub(crate) const MISSION_OBSERVER: &str = "mission";

/// The byte budget for a `<stem>.json` index file name.
///
/// pi `MAX_JSON_FILE_STEM_BYTES` (`result-files.ts:16`):
/// `MAX_INDEX_SEGMENT_BYTES - Buffer.byteLength(".json")`. Encoding a run id into a file *name*
/// must reserve room for the suffix, so the stem budget is 250, not 255 — miss this and a
/// maximally-long id produces a 260-byte name that the filesystem rejects.
pub(crate) const MAX_JSON_FILE_STEM_BYTES: usize =
    IndexSegment::MAX_BYTES - ResultFileName::EXTENSION.len();

/// pi `encodedJsonFileName` (`result-files.ts:31-33`) — the single write name.
fn encoded_json_file_name(value: &str) -> String {
    format!(
        "{}{}",
        IndexSegment::encode_bounded(value, MAX_JSON_FILE_STEM_BYTES),
        ResultFileName::EXTENSION
    )
}

/// pi `encodedJsonFileNames` (`:35-37`) — the read fan-out.
fn encoded_json_file_names(value: &str) -> Vec<String> {
    IndexSegment::aliases(value, MAX_JSON_FILE_STEM_BYTES)
        .into_iter()
        .map(|segment| format!("{segment}{}", ResultFileName::EXTENSION))
        .collect()
}

/// `<results_dir>/result-index`.
pub(crate) fn result_index_root(results_dir: &Path) -> PathBuf {
    results_dir.join(RESULT_INDEX_DIR)
}

// --- session index -----------------------------------------------------------------------

/// `<results_dir>/result-index/sessions/<enc(session)>` — pi `sessionIndexDir` (`:56-58`).
pub(crate) fn session_index_dir(results_dir: &Path, session_id: &SessionId) -> PathBuf {
    result_index_root(results_dir)
        .join(SESSION_INDEX_DIR)
        .join(IndexSegment::encode(session_id.as_str()).as_str())
}

/// The read fan-out of [`session_index_dir`] — pi `sessionIndexDirs` (`:60-62`).
pub(crate) fn session_index_dirs(results_dir: &Path, session_id: &SessionId) -> Vec<PathBuf> {
    let root = result_index_root(results_dir).join(SESSION_INDEX_DIR);
    IndexSegment::read_aliases(session_id.as_str())
        .into_iter()
        .map(|segment| root.join(segment.as_str()))
        .collect()
}

/// `<session index dir>/<enc(run)>.json` — pi `resultIndexPath` (`:64-66`).
pub(crate) fn result_index_path(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> PathBuf {
    session_index_dir(results_dir, session_id).join(encoded_json_file_name(run_id.as_str()))
}

/// The full read fan-out — every session-dir alias × every run-name alias.
///
/// pi `resultIndexPaths` (`:68-70`) flat-maps both dimensions; a value whose encoding rules
/// changed may have moved in either.
pub(crate) fn result_index_paths(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> Vec<PathBuf> {
    let names = encoded_json_file_names(run_id.as_str());
    session_index_dirs(results_dir, session_id)
        .into_iter()
        .flat_map(|dir| {
            names
                .iter()
                .map(move |name| dir.join(name))
                .collect::<Vec<_>>()
        })
        .collect()
}

// --- run index ---------------------------------------------------------------------------

/// `<results_dir>/result-index/runs/<enc(run)>.json` — pi `runIndexPath` (`:72-74`).
pub(crate) fn run_index_path(results_dir: &Path, run_id: &RunId) -> PathBuf {
    result_index_root(results_dir)
        .join(RUN_INDEX_DIR)
        .join(encoded_json_file_name(run_id.as_str()))
}

// --- pending payloads --------------------------------------------------------------------

/// `<results_dir>/result-pending/<enc(session)>/<enc(run)>.json` — pi `resultPendingPath`
/// (`:76-78`). This is where a payload lands **first**, session-private, before promotion.
pub(crate) fn result_pending_path(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> PathBuf {
    pending_session_dir(results_dir, session_id).join(encoded_json_file_name(run_id.as_str()))
}

/// The read fan-out of [`result_pending_path`] — pi `resultPendingPaths` (`:80-82`).
pub(crate) fn result_pending_paths(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> Vec<PathBuf> {
    let names = encoded_json_file_names(run_id.as_str());
    pending_session_dirs(results_dir, session_id)
        .into_iter()
        .flat_map(|dir| {
            names
                .iter()
                .map(move |name| dir.join(name))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `<results_dir>/result-pending/<enc(session)>` — the write dir.
pub(crate) fn pending_session_dir(results_dir: &Path, session_id: &SessionId) -> PathBuf {
    results_dir
        .join(RESULT_PENDING_DIR)
        .join(IndexSegment::encode(session_id.as_str()).as_str())
}

/// The read fan-out of [`pending_session_dir`] — pi `pendingSessionDirs` (`:84-86`).
pub(crate) fn pending_session_dirs(results_dir: &Path, session_id: &SessionId) -> Vec<PathBuf> {
    let root = results_dir.join(RESULT_PENDING_DIR);
    IndexSegment::read_aliases(session_id.as_str())
        .into_iter()
        .map(|segment| root.join(segment.as_str()))
        .collect()
}

// --- owned payloads ----------------------------------------------------------------------

/// `<results_dir>/result-owned/<enc(session)>/<enc(run)>.json` — where a promoted payload lands.
///
/// Same session-partitioned shape as [`result_pending_path`], and for the same reason: the
/// partition key is what keeps one instance's results out of another's reach. The difference
/// between the two directories is lifecycle, not visibility — `result-pending` holds a payload
/// that is not yet a completion, `result-owned` holds one that is.
pub(crate) fn result_owned_path(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> PathBuf {
    owned_session_dir(results_dir, session_id).join(encoded_json_file_name(run_id.as_str()))
}

/// The read fan-out of [`result_owned_path`], over both alias dimensions.
pub(crate) fn result_owned_paths(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> Vec<PathBuf> {
    let names = encoded_json_file_names(run_id.as_str());
    owned_session_dirs(results_dir, session_id)
        .into_iter()
        .flat_map(|dir| {
            names
                .iter()
                .map(move |name| dir.join(name))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `<results_dir>/result-owned/<enc(session)>` — the write dir.
pub(crate) fn owned_session_dir(results_dir: &Path, session_id: &SessionId) -> PathBuf {
    results_dir
        .join(RESULT_OWNED_DIR)
        .join(IndexSegment::encode(session_id.as_str()).as_str())
}

/// The read fan-out of [`owned_session_dir`].
pub(crate) fn owned_session_dirs(results_dir: &Path, session_id: &SessionId) -> Vec<PathBuf> {
    let root = results_dir.join(RESULT_OWNED_DIR);
    IndexSegment::read_aliases(session_id.as_str())
        .into_iter()
        .map(|segment| root.join(segment.as_str()))
        .collect()
}

// --- observer index ----------------------------------------------------------------------

/// `<results_dir>/result-index/observers/<observer>` — pi `observerIndexDir` (`:95-97`).
///
/// The observer name is **not** encoded upstream: it is a fixed internal constant
/// ([`MISSION_OBSERVER`]), never user data, so there is nothing to escape.
pub(crate) fn observer_index_dir(results_dir: &Path, observer: &str) -> PathBuf {
    result_index_root(results_dir)
        .join(OBSERVER_INDEX_DIR)
        .join(observer)
}

/// `<observer dir>/<enc(run)>.json` — pi `observerIndexPath` (`:99-101`).
pub(crate) fn observer_index_path(results_dir: &Path, observer: &str, run_id: &RunId) -> PathBuf {
    observer_index_dir(results_dir, observer).join(encoded_json_file_name(run_id.as_str()))
}

/// The mission observer band's directory — the cross-session reconciliation index.
pub(crate) fn mission_observer_dir(results_dir: &Path) -> PathBuf {
    observer_index_dir(results_dir, MISSION_OBSERVER)
}

/// The mission observer entry for one run.
pub(crate) fn mission_observer_path(results_dir: &Path, run_id: &RunId) -> PathBuf {
    observer_index_path(results_dir, MISSION_OBSERVER, run_id)
}

// --- tool-call index ---------------------------------------------------------------------

/// `<results_dir>/result-index/tool-calls/<enc(toolCall)>` — pi `toolCallIndexDir` (`:103-105`).
pub(crate) fn tool_call_index_dir(results_dir: &Path, tool_call_id: &str) -> PathBuf {
    result_index_root(results_dir)
        .join(TOOL_CALL_INDEX_DIR)
        .join(IndexSegment::encode(tool_call_id).as_str())
}

/// `<tool-call dir>/<enc(run)>.json` — pi `toolCallIndexPath` (`:107-109`).
pub(crate) fn tool_call_index_path(
    results_dir: &Path,
    tool_call_id: &str,
    run_id: &RunId,
) -> PathBuf {
    tool_call_index_dir(results_dir, tool_call_id).join(encoded_json_file_name(run_id.as_str()))
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

    fn session(value: &str) -> SessionId {
        SessionId::parse(value).expect("non-empty test session id")
    }

    #[test]
    fn the_stem_budget_reserves_room_for_the_json_suffix() {
        assert_eq!(MAX_JSON_FILE_STEM_BYTES, 250);
        assert_eq!(
            MAX_JSON_FILE_STEM_BYTES + ResultFileName::EXTENSION.len(),
            IndexSegment::MAX_BYTES
        );
    }

    #[test]
    fn a_long_run_id_still_produces_a_name_within_the_component_limit() {
        // The reason the budget is 250: a 255-byte stem plus ".json" would be 260 bytes.
        let long = "r".repeat(300);
        let name = encoded_json_file_name(&long);
        assert!(
            name.len() <= IndexSegment::MAX_BYTES,
            "name was {} bytes",
            name.len()
        );
        assert!(name.ends_with(".json"));
    }

    #[test]
    fn the_session_partition_layout_matches_upstream() {
        let dir = session_index_dir(Path::new("/r"), &session("s1"));
        assert_eq!(dir, PathBuf::from("/r/result-index/sessions/s1"));
    }

    #[test]
    fn the_full_index_path_layout_matches_upstream() {
        let path = result_index_path(Path::new("/r"), &session("s1"), &RunId::from_token("run1"));
        assert_eq!(path, PathBuf::from("/r/result-index/sessions/s1/run1.json"));
    }

    #[test]
    fn the_run_and_observer_and_tool_call_layouts_match_upstream() {
        let run = RunId::from_token("run1");
        assert_eq!(
            run_index_path(Path::new("/r"), &run),
            PathBuf::from("/r/result-index/runs/run1.json")
        );
        assert_eq!(
            mission_observer_path(Path::new("/r"), &run),
            PathBuf::from("/r/result-index/observers/mission/run1.json")
        );
        assert_eq!(
            tool_call_index_path(Path::new("/r"), "tc1", &run),
            PathBuf::from("/r/result-index/tool-calls/tc1/run1.json")
        );
    }

    #[test]
    fn the_pending_payload_is_session_private() {
        let path = result_pending_path(Path::new("/r"), &session("s1"), &RunId::from_token("run1"));
        assert_eq!(path, PathBuf::from("/r/result-pending/s1/run1.json"));
        // It must NOT be a sibling of the public payload — that is the whole point of staging.
        assert_ne!(path.parent(), Some(Path::new("/r")));
    }

    #[test]
    fn the_owned_payload_is_session_private_and_never_in_the_root() {
        let path = result_owned_path(Path::new("/r"), &session("s1"), &RunId::from_token("run1"));
        assert_eq!(path, PathBuf::from("/r/result-owned/s1/run1.json"));
        // The property the whole relocation exists for: a process listing `/r` cannot see it.
        assert_ne!(path.parent(), Some(Path::new("/r")));
    }

    #[test]
    fn owned_and_pending_never_collide() {
        // Two lifecycle stages, two directories: promotion must be a rename BETWEEN them, never a
        // no-op onto the same path.
        let run = RunId::from_token("run1");
        assert_ne!(
            result_owned_path(Path::new("/r"), &session("s1"), &run),
            result_pending_path(Path::new("/r"), &session("s1"), &run)
        );
    }

    #[test]
    fn owned_paths_fan_out_over_aliases_leading_with_the_write_location() {
        let id = session("/home/u/s.jsonl");
        let run = RunId::from_token("run1");
        let paths = result_owned_paths(Path::new("/r"), &id, &run);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], result_owned_path(Path::new("/r"), &id, &run));
    }

    #[test]
    fn a_session_id_that_is_a_path_cannot_escape_the_results_dir() {
        // The invariant `IndexSegment` exists for: a raw join would produce
        // `/r/result-index/sessions//home/u/s.jsonl`, i.e. an absolute path outside `/r`.
        let dir = session_index_dir(Path::new("/r"), &session("/home/u/s.jsonl"));
        assert!(
            dir.starts_with("/r/result-index/sessions"),
            "escaped to {dir:?}"
        );
        assert_eq!(
            dir.components().count(),
            5,
            "must be exactly one component deeper: {dir:?}"
        );
    }

    #[test]
    fn a_traversal_shaped_session_id_stays_one_component() {
        // The separators are encoded (`%2F`), so this is a single oddly-named directory rather
        // than a traversal. The literal dots survive because `.` is in the unreserved set, which
        // is fine: only a separator can move you up a level.
        let dir = session_index_dir(Path::new("/r"), &session("../../../etc"));
        assert!(
            dir.starts_with("/r/result-index/sessions"),
            "escaped to {dir:?}"
        );
        assert_eq!(
            dir.components().count(),
            5,
            "must stay one component deep: {dir:?}"
        );
        assert!(
            !dir.to_string_lossy().contains("../"),
            "a real traversal segment: {dir:?}"
        );
    }

    #[test]
    fn a_bare_dotdot_session_id_hashes_rather_than_becoming_a_traversal() {
        // The genuine risk `is_portable_segment` exists to catch: `encodeURIComponent("..")` is
        // `".."` verbatim, so without the portability rule this would join to the PARENT of the
        // sessions directory.
        for hostile in ["..", "."] {
            let dir = session_index_dir(Path::new("/r"), &session(hostile));
            assert_eq!(dir.components().count(), 5, "{hostile} escaped: {dir:?}");
            assert!(
                dir.file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|n| n.starts_with("~sha256-")),
                "{hostile} must hash, got {dir:?}"
            );
        }
    }

    #[test]
    fn writes_go_to_exactly_one_place() {
        // Singular builders are the write path; there is no ambiguity about where a value lands.
        let dirs = session_index_dirs(Path::new("/r"), &session("s1"));
        assert_eq!(dirs.len(), 1, "a portable id has a single alias");
        assert_eq!(dirs[0], session_index_dir(Path::new("/r"), &session("s1")));
    }

    #[test]
    fn reads_fan_out_over_aliases_and_lead_with_the_write_location() {
        let id = session("/home/u/s.jsonl");
        let dirs = session_index_dirs(Path::new("/r"), &id);
        assert_eq!(
            dirs.len(),
            2,
            "a hashed id also exposes its pre-hash location"
        );
        assert_eq!(
            dirs[0],
            session_index_dir(Path::new("/r"), &id),
            "write location first"
        );
    }

    #[test]
    fn the_read_fan_out_crosses_both_dimensions() {
        // session aliases (2) × run-name aliases (2) == 4 candidate paths.
        let id = session("/home/u/s.jsonl");
        let run = RunId::from_token("/weird/run.jsonl");
        let paths = result_index_paths(Path::new("/r"), &id, &run);
        assert_eq!(paths.len(), 4, "got {paths:?}");
        assert_eq!(
            paths[0],
            result_index_path(Path::new("/r"), &id, &run),
            "write path first"
        );
    }

    #[test]
    fn pending_paths_fan_out_the_same_way() {
        let id = session("/home/u/s.jsonl");
        let run = RunId::from_token("run1");
        let paths = result_pending_paths(Path::new("/r"), &id, &run);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], result_pending_path(Path::new("/r"), &id, &run));
    }
}
