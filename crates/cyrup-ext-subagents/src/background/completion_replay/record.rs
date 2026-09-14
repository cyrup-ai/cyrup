//! [`CompletionReplayRecord`] — the durable record itself, its tolerant parser, and the two
//! containment checks that decide whether a record on disk may speak for a run.

use std::path::{Component, Path, PathBuf};

use crate::background::RunId;
use crate::background::wait_completions::WaitCompletion;
use crate::identity::SessionId;

use super::{REPLAY_VERSION, completion_archive_path};

/// pi `CompletionReplayRecord` (`completion-replay.ts:31-39`).
///
/// # `session_id` is required, and that is load-bearing
///
/// Upstream types it `string` (`:34`) and `parseReplay` rejects a record without one (`:133`). The
/// read gate at `:228` compares it against the caller's session, so a record with no session could
/// never be read back by anyone — it would be pure garbage occupying the run's one replay slot.
/// [`SessionId`] (not `Option<SessionId>`) makes that state unconstructable rather than merely
/// unwritten. cyrup's own `Attribution::classify` (`background/delivery/custody.rs`) gives the same
/// guarantee structurally — the `Ours` band is only produced after `result.session_id` was
/// destructured out of an `Option` — so the required field costs the writer's call site nothing.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionReplayRecord {
    /// pi `:32`. [`ReplayVersion`] rejects anything but [`REPLAY_VERSION`] at DEserialization,
    /// which is what makes [`parse_replay`] skip a future version instead of misreading it.
    pub version: ReplayVersion,
    /// The run this record speaks for. Checked against the caller's (or the file name's) run id by
    /// [`validate_replay_record`], never by [`parse_replay`] — see that split's own doc.
    pub run_id: RunId,
    /// The session that owned the completion — the OPTIONAL read filter at
    /// [`super::read_completion_replay`].
    pub session_id: SessionId,
    /// Epoch millis at which the completion was observed (pi `:201`, `input.now`).
    pub completed_at: i64,
    /// `completed_at + ttl_ms` (pi `:202`). At or past this, the record and its archive are
    /// removed by whichever reader notices first.
    pub expires_at: i64,
    /// The projection a `wait` replays. Its own `archive_path` mirrors the field below (pi `:145`).
    pub completion: WaitCompletion,
    /// The companion archive. Must CANONICALLY equal [`super::completion_archive_path`] for this
    /// record's `results_dir`/`run_id`, or the record is a redirect — see
    /// [`validate_replay_record`].
    pub archive_path: PathBuf,
}

/// The literal `1` of [`REPLAY_VERSION`], as a type.
///
/// pi's version check is `record.version !== REPLAY_VERSION → undefined` (`:131`). Expressing it as
/// a `Deserialize` that rejects every other value moves the check to the boundary and makes
/// [`parse_replay`] one fallible constructor — `serde_json::from_slice(..).ok()` — rather than a
/// hand-written field-by-field validator a future field could be forgotten from. A `u32` field plus
/// a runtime `if` is the shape that rots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayVersion;

impl serde::Serialize for ReplayVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(REPLAY_VERSION)
    }
}

impl<'de> serde::Deserialize<'de> for ReplayVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == REPLAY_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unknown completion-replay record version {raw}"
            )))
        }
    }
}

/// pi `parseReplay` (`:128-139`) composed with `parseCompletion` (`:121-126`).
///
/// **Tolerant by contract**: this reads bytes written by a DIFFERENT build. Malformed JSON, a
/// missing field, an empty `sessionId` and an unknown `version` all become `None` — never an `Err`,
/// never a panic.
///
/// # It takes NO caller run id, and that is not an omission
///
/// Upstream splits the two run-id checks on purpose. `parseReplay` calls
/// `parseCompletion(record.completion, record.runId)` (`:137`), so `:124` compares the embedded
/// completion's run id against the RECORD'S OWN — an internal-consistency check. The caller's (or,
/// in the sweep, the FILE NAME's) run id is checked only in [`validate_replay_record`] at `:142`.
///
/// Collapsing them would change observable behaviour: `cleanupCompletionReplay` (`:265-274`)
/// branches on `record` versus `safeRecord` separately, and a record whose run id disagrees with
/// its file name must land in the "remove on sight" row. Folding the check in here would make such
/// a record parse as `None` and fall into the age-gated row instead, letting a misfiled record
/// survive inside the retention window as if it were a future-version record.
pub(super) fn parse_replay(bytes: &[u8]) -> Option<CompletionReplayRecord> {
    let record: CompletionReplayRecord = serde_json::from_slice(bytes).ok()?;
    // pi `:124`, through `parseCompletion(record.completion, record.runId)` at `:137`.
    if record.completion.run_id != record.run_id.as_str() {
        return None;
    }
    Some(record)
}

/// pi `validateReplayRecord` (`:141-147`) — the run-id check AND the containment check.
///
/// 1. `record.runId !== runId` (`:142`) — the caller's run id, which in the sweep comes from the
///    FILE NAME. This is what stops a record moved (or copied) between file names from replaying
///    another run's output under this run's id.
/// 2. The stored `archivePath` must CANONICALLY equal [`super::completion_archive_path`] for this
///    `results_dir`/`run_id` (`:143-144`). A record is otherwise a redirect: a hand-edited or
///    relocated record could name any file on disk and [`super::read_completion_archive`] would
///    read it. Upstream compares `path.resolve(...)`; cyrup canonicalizes where both sides exist
///    and falls back to a component-normalized compare when the archive is already gone — the
///    expired case, where the record is about to be deleted anyway.
///
/// On success the path is REWRITTEN to the canonical one and mirrored onto
/// `completion.archive_path` (pi `:145`), so a caller never sees the stored value.
/// [`WaitCompletion::archive_path`] is `Option<String>`, so the mirror is a lossy stringification —
/// which is exactly what the field's own wire shape (`archivePath`, a JSON string) already is.
pub(super) fn validate_replay_record(
    results_dir: &Path,
    run_id: &RunId,
    mut record: CompletionReplayRecord,
) -> Option<CompletionReplayRecord> {
    if &record.run_id != run_id {
        return None;
    }
    let archive_path = completion_archive_path(results_dir, run_id);
    if !same_file_path(&record.archive_path, &archive_path) {
        return None;
    }
    record.completion.archive_path = Some(archive_path.to_string_lossy().into_owned());
    record.archive_path = archive_path;
    Some(record)
}

/// pi's `path.resolve(a) === path.resolve(b)` (`:144`), with a filesystem rung in front.
///
/// `canonicalize` is the stronger answer — it resolves symlinks, which a purely lexical compare
/// cannot — but it requires the path to EXIST, and the single most important case for this check is
/// an expired record whose archive is already gone. So: canonicalize when both sides resolve, and
/// otherwise fall back to the lexical normalization upstream uses unconditionally.
fn same_file_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => resolved(left) == resolved(right),
    }
}

/// `path.resolve` (`:144`): absolute against the process cwd, then `.`/`..` folded away.
///
/// A relative `results_dir` is legal — nothing in this crate forbids one — and both sides of the
/// comparison are then relative to the SAME cwd, so the fold alone would already agree. Making them
/// absolute matches upstream and means a record written under an absolute `results_dir` still
/// validates when read back through a relative one.
fn resolved(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            // No cwd (it was removed under us): the lexical fold over the relative path is still a
            // consistent answer for both sides of one comparison.
            Err(_) => path.to_path_buf(),
        }
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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
    use serde_json::json;

    fn record_json(results_dir: &Path, run: &str, version: u32) -> Vec<u8> {
        json!({
            "version": version,
            "runId": run,
            "sessionId": "s1",
            "completedAt": 1_000,
            "expiresAt": 601_000,
            "completion": { "runId": run, "agent": "coder" },
            "archivePath": completion_archive_path(results_dir, &RunId::from_token(run)),
        })
        .to_string()
        .into_bytes()
    }

    /// The version gate is a TYPE, so an unknown version is a parse miss rather than a misread —
    /// and, critically, it is NOT a panic and NOT an error: a record written by a newer build must
    /// read as "nothing here" so the age policy, not this parser, decides its fate.
    #[test]
    fn a_future_replay_version_is_ignored_not_a_panic() {
        let dir = Path::new("/results");
        assert!(
            parse_replay(&record_json(dir, "r1", 1)).is_some(),
            "v1 parses"
        );
        assert!(
            parse_replay(&record_json(dir, "r1", 2)).is_none(),
            "v2 is skipped, never repaired"
        );
        assert!(parse_replay(b"{not json").is_none(), "garbage is tolerated");
        assert!(parse_replay(b"[]").is_none(), "an array is not a record");
        // pi `:133`: a record with no usable session is unreadable by construction. cyrup's
        // `SessionId` deserializer rejects `""` outright, which is the same outcome one step
        // earlier.
        let empty_session = json!({
            "version": 1, "runId": "r1", "sessionId": "",
            "completedAt": 0, "expiresAt": 1, "completion": { "runId": "r1" },
            "archivePath": "/results/output-archives/r1.json",
        })
        .to_string();
        assert!(parse_replay(empty_session.as_bytes()).is_none());
    }

    /// pi `:124`, reached through `parseCompletion(record.completion, record.runId)` at `:137`:
    /// the EMBEDDED completion must agree with the record's own run id. This is the internal
    /// consistency check, and it is a different check from [`validate_replay_record`]'s.
    #[test]
    fn a_completion_disagreeing_with_its_own_record_does_not_parse() {
        let mismatched = json!({
            "version": 1, "runId": "r1", "sessionId": "s1",
            "completedAt": 0, "expiresAt": 1,
            "completion": { "runId": "someone-else" },
            "archivePath": "/results/output-archives/r1.json",
        })
        .to_string();
        assert!(parse_replay(mismatched.as_bytes()).is_none());
    }

    /// The containment property (`:143-144`). A record may name only ITS OWN canonical archive; a
    /// relocated or hand-edited one is a redirect that would let `read_completion_archive` read an
    /// arbitrary file, so it is rejected — and its caller removes it.
    #[test]
    fn a_replay_naming_a_foreign_archive_path_is_rejected_and_removed() {
        let dir = Path::new("/results");
        let run = RunId::from_token("r1");

        let good = parse_replay(&record_json(dir, "r1", 1)).expect("parses");
        let validated = validate_replay_record(dir, &run, good).expect("its own archive");
        assert_eq!(validated.archive_path, completion_archive_path(dir, &run));
        // pi `:145` — the canonical path is mirrored onto the completion, so a caller never sees
        // the stored spelling.
        assert_eq!(
            validated.completion.archive_path.as_deref(),
            Some("/results/output-archives/r1.json")
        );

        let redirected = json!({
            "version": 1, "runId": "r1", "sessionId": "s1",
            "completedAt": 0, "expiresAt": 1, "completion": { "runId": "r1" },
            "archivePath": "/etc/passwd",
        })
        .to_string();
        let parsed = parse_replay(redirected.as_bytes()).expect("parses");
        assert!(
            validate_replay_record(dir, &run, parsed).is_none(),
            "a record naming a file outside its own archive slot is a redirect"
        );

        // And rung 1: the caller's run id, which in the sweep is the FILE NAME's.
        let parsed = parse_replay(&record_json(dir, "r1", 1)).expect("parses");
        assert!(
            validate_replay_record(dir, &RunId::from_token("r2"), parsed).is_none(),
            "a record filed under another run's name must not answer for it"
        );
    }

    /// `path.resolve` equivalence (`:144`): the same file reached through `.`/`..` is the same
    /// file, and a genuinely different one is not — asserted without touching the filesystem, which
    /// is the fallback rung that the expired-record case always takes.
    #[test]
    fn archive_paths_compare_after_resolution_not_as_raw_strings() {
        assert!(same_file_path(
            Path::new("/results/output-archives/r1.json"),
            Path::new("/results/staging/../output-archives/./r1.json"),
        ));
        assert!(!same_file_path(
            Path::new("/results/output-archives/r1.json"),
            Path::new("/results/output-archives/r2.json"),
        ));
    }
}
