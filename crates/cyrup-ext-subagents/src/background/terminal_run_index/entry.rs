//! [`TerminalRunIndexEntry`], its state predicate, and its on-disk address.
//!
//! Ports pi `terminal-run-index.ts:8-48` — the constants, `isTerminalState`, the four path
//! builders, and `parseEntry`.

use std::path::{Path, PathBuf};

use crate::background::{RunId, RunState};
use crate::identity::{IndexSegment, SessionId};

/// pi `TERMINAL_RUN_INDEX_DIR` (`terminal-run-index.ts:8`).
///
/// A dot-prefixed sibling of the run directories INSIDE the async root, not beside it — see
/// [`super`]'s docs for why that placement obliges every async-root scanner to skip it.
pub const TERMINAL_RUN_INDEX_DIR: &str = ".terminal-runs";

/// pi `TIMESTAMP_WIDTH` (`:11`). Zero-padding to a fixed width is what makes a plain
/// lexicographic sort of the marker names a *chronological* sort.
const TIMESTAMP_WIDTH: usize = 16;

/// The index schema version, `1` — pi `TERMINAL_RUN_INDEX_VERSION` (`:10`), whose `parseEntry`
/// rejects any other value (`:43`).
///
/// Same unit-type-with-custom-serde shape as [`crate::background::result_index::IndexVersion`]
/// (`result_index/entry.rs:21-46`): "this record is version 1" becomes a parse outcome rather
/// than a field every reader must remember to check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TerminalIndexVersion;

impl TerminalIndexVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl serde::Serialize for TerminalIndexVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for TerminalIndexVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported terminal-run index version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// One marker: "run R of session S reached a terminal state at T".
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRunIndexEntry {
    /// Always [`TerminalIndexVersion::VALUE`]; a record with any other version fails to parse.
    pub version: TerminalIndexVersion,
    /// The run the marker records. Non-emptiness is enforced by [`TerminalRunIndexEntry::parse`].
    pub run_id: RunId,
    /// The session the run belonged to — the partition key. Non-emptiness is enforced by
    /// [`SessionId`]'s own `Deserialize` (`identity/session_id.rs`), which is pi's
    /// `typeof … !== "string" || !entry.sessionId` guard (`:45`) moved to the parse boundary.
    pub session_id: SessionId,
    /// Epoch milliseconds at which the run reached its indexed state.
    pub ended_at: i64,
}

impl TerminalRunIndexEntry {
    /// pi `parseEntry` (`:39-48`): `None` for anything malformed, wrong-versioned, or not an
    /// object. Every failure collapses to `None` because every caller's response is uniform —
    /// unlink and continue.
    ///
    /// `run_id` is checked here rather than in serde because [`RunId`] is
    /// `#[serde(transparent)]` and would happily accept `""`, which pi rejects at `:44`.
    /// `ended_at` is `i64`, so a fractional `endedAt` on disk fails to parse; pi accepts any
    /// finite number and floors it later (`:33`). Nothing writes a fractional value — both
    /// implementations source it from an integer millisecond clock — and the divergent case
    /// resolves to "unlink the marker", which is already the outcome for every other malformed
    /// record.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let entry: Self = serde_json::from_slice(bytes).ok()?;
        (!entry.run_id.as_str().is_empty()).then_some(entry)
    }
}

/// pi `isTerminalState` (`terminal-run-index.ts:20-22`).
///
/// NOT [`RunState::is_terminal`]. Upstream's predicate here is the complement of
/// `queued`/`running` and therefore INCLUDES `paused`, while `RunState::is_terminal` excludes it
/// (`background/state.rs`: pause is soft and resumable, which is the right rule for a transition
/// guard and the wrong one for "has this run stopped moving?"). Upstream keeps four distinct
/// terminal sets in this subsystem (`async-retention.ts:28`, `chain-root-attachment.ts:82`,
/// `stale-run-reconciler.ts:298`) and they are not interchangeable.
///
/// Spelled as the complement rather than as a member list so that `partial`/`rejected` — the two
/// upstream states cyrup has no [`RunState`] variant for — are included by construction the day
/// they are added.
#[must_use]
pub(crate) const fn is_indexed_state(state: RunState) -> bool {
    !matches!(state, RunState::Queued | RunState::Running)
}

/// `<async_root>/.terminal-runs` — pi `indexRoot` (`:24-26`).
pub(crate) fn index_root(async_root: &Path) -> PathBuf {
    async_root.join(TERMINAL_RUN_INDEX_DIR)
}

/// `<async_root>/.terminal-runs/<enc(session)>` — pi `sessionIndexDir` (`:28-30`).
///
/// ONE key, never [`IndexSegment::read_aliases`]: upstream does not fan out here, and
/// [`super::read`]'s re-verification against the live status is what makes the single hashed key
/// safe to use as an address.
pub(crate) fn session_index_dir(async_root: &Path, session_id: &SessionId) -> PathBuf {
    index_root(async_root).join(IndexSegment::encode(session_id.as_str()).as_str())
}

/// `<0-padded endedAt>-<enc(runId)>.json` — pi `markerName` (`:32-34`).
///
/// `Math.max(0, Math.floor(endedAt))` clamps a pre-epoch clock to `0`; `Math.floor` is a no-op on
/// an `i64`.
pub(crate) fn marker_name(ended_at: i64, run_id: &str) -> String {
    format!(
        "{:0width$}-{}.json",
        ended_at.max(0),
        IndexSegment::encode(run_id),
        width = TIMESTAMP_WIDTH
    )
}

/// pi `markerPath` (`:36-38`).
///
/// `None` when `async_dir` has no parent or a non-UTF-8 leaf — pi's `path.dirname`/`path.basename`
/// cannot fail, so the `Option` is cyrup's; the caller treats it as "nothing to index".
///
/// The name encodes the DIRECTORY leaf while [`super::update`] records `status.run_id` in the
/// entry. They can legitimately disagree (pi `:36-38` vs `:55`), which is precisely why
/// [`super::read`]'s re-verification against the live status exists — the entry's contents decide,
/// never the path.
pub(crate) fn marker_path(
    async_dir: &Path,
    session_id: &SessionId,
    ended_at: i64,
) -> Option<PathBuf> {
    let async_root = async_dir.parent()?;
    let dir_name = async_dir.file_name()?.to_str()?;
    Some(session_index_dir(async_root, session_id).join(marker_name(ended_at, dir_name)))
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

    fn entry_json(extra: &str) -> String {
        format!("{{\"version\":1,\"runId\":\"r1\",\"sessionId\":\"s1\",\"endedAt\":123{extra}}}")
    }

    #[test]
    fn a_well_formed_entry_parses() {
        let entry = TerminalRunIndexEntry::parse(entry_json("").as_bytes()).expect("valid entry");
        assert_eq!(entry.run_id.as_str(), "r1");
        assert_eq!(entry.session_id.as_str(), "s1");
        assert_eq!(entry.ended_at, 123);
    }

    #[test]
    fn the_camel_case_wire_shape_is_preserved() {
        let entry = TerminalRunIndexEntry::parse(entry_json("").as_bytes()).expect("valid");
        let json = serde_json::to_string(&entry).expect("ser");
        assert!(json.contains("\"runId\""), "got {json}");
        assert!(json.contains("\"sessionId\""), "got {json}");
        assert!(json.contains("\"endedAt\""), "got {json}");
        assert!(json.contains("\"version\":1"), "got {json}");
        let back = TerminalRunIndexEntry::parse(json.as_bytes()).expect("round-trips");
        assert_eq!(back, entry);
    }

    #[test]
    fn a_future_version_is_none_not_a_panic() {
        let json = entry_json("").replace("\"version\":1", "\"version\":2");
        assert!(TerminalRunIndexEntry::parse(json.as_bytes()).is_none());
    }

    #[test]
    fn an_empty_run_id_makes_the_entry_unparseable() {
        // pi `:44` — `typeof entry.runId !== "string" || !entry.runId`. RunId is transparent
        // serde, so the guard lives in `parse`.
        let json = entry_json("").replace("\"r1\"", "\"\"");
        assert!(TerminalRunIndexEntry::parse(json.as_bytes()).is_none());
    }

    #[test]
    fn an_empty_session_id_makes_the_entry_unparseable() {
        let json = entry_json("").replace("\"s1\"", "\"\"");
        assert!(TerminalRunIndexEntry::parse(json.as_bytes()).is_none());
    }

    #[test]
    fn a_fractional_ended_at_makes_the_entry_unparseable() {
        // The documented divergence: pi floors a finite fractional number, cyrup's i64 rejects it.
        // Both resolve to "the marker is unlinked" downstream.
        let json = entry_json("").replace(":123", ":123.5");
        assert!(TerminalRunIndexEntry::parse(json.as_bytes()).is_none());
    }

    #[test]
    fn every_required_field_is_required() {
        for missing in [
            "\"runId\":\"r1\",",
            "\"sessionId\":\"s1\",",
            ",\"endedAt\":123",
        ] {
            let json = entry_json("").replace(missing, "");
            assert!(
                TerminalRunIndexEntry::parse(json.as_bytes()).is_none(),
                "removing {missing} must make the entry unparseable"
            );
        }
    }

    #[test]
    fn garbage_and_non_objects_are_none() {
        assert!(TerminalRunIndexEntry::parse(b"not json").is_none());
        assert!(TerminalRunIndexEntry::parse(b"[]").is_none());
        assert!(TerminalRunIndexEntry::parse(b"null").is_none());
        assert!(TerminalRunIndexEntry::parse(b"").is_none());
    }

    #[test]
    fn the_indexed_set_is_the_complement_of_queued_and_running() {
        // §1.1 — the trap this module documents: `Paused` IS indexed here even though
        // `RunState::is_terminal` excludes it.
        assert!(!is_indexed_state(RunState::Queued));
        assert!(!is_indexed_state(RunState::Running));
        assert!(is_indexed_state(RunState::Paused));
        assert!(is_indexed_state(RunState::Complete));
        assert!(is_indexed_state(RunState::Failed));
        assert!(is_indexed_state(RunState::Stopped));
    }

    #[test]
    fn marker_names_zero_pad_and_clamp_the_timestamp() {
        // pi `markerName`: padStart(16, "0") over max(0, floor(endedAt)).
        assert_eq!(marker_name(123, "run-a"), "0000000000000123-run-a.json");
        assert_eq!(marker_name(-5, "run-a"), "0000000000000000-run-a.json");
        // Chronology is the sort: a later timestamp sorts strictly after an earlier one.
        assert!(marker_name(2, "z") > marker_name(1, "a"));
    }

    #[test]
    fn marker_and_session_paths_use_the_index_segment_encoder() {
        let sid = SessionId::parse("/home/u/s.jsonl").expect("non-empty");
        let dir = session_index_dir(Path::new("/a"), &sid);
        assert!(dir.starts_with("/a/.terminal-runs"), "got {dir:?}");
        // The leaf must be EXACTLY what the one shared encoder produces (for a `.jsonl` session id
        // that is its hashed fallback) — the path never carries the raw value, and this module
        // must never grow a second encoding.
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some(IndexSegment::encode("/home/u/s.jsonl").as_str()),
            "got {dir:?}"
        );
    }

    #[test]
    fn marker_path_derives_root_and_run_from_the_async_dir() {
        let sid = SessionId::parse("s1").expect("non-empty");
        let path = marker_path(Path::new("/a/run1"), &sid, 7).expect("has parent and leaf");
        assert_eq!(
            path,
            PathBuf::from("/a/.terminal-runs/s1/0000000000000007-run1.json")
        );
    }

    #[test]
    fn marker_path_is_none_for_a_rootless_async_dir() {
        let sid = SessionId::parse("s1").expect("non-empty");
        assert!(marker_path(Path::new("/"), &sid, 1).is_none());
    }
}
