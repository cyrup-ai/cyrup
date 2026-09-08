//! [`ResultIndexEntry`] and [`IndexVersion`] — the on-disk index record.
//!
//! Ports pi `ResultIndexEntry` (`result-files.ts:18-26`) and `parseResultIndexEntry` (`:111-127`).

use std::path::PathBuf;

use crate::background::RunId;
use crate::identity::{ResultFileName, SessionId};

/// The index schema version, `1`.
///
/// pi `RESULT_INDEX_VERSION` (`result-files.ts:7`), whose `parseResultIndexEntry` rejects any
/// record with `record.version !== RESULT_INDEX_VERSION` (`:114`).
///
/// Modelled as a unit type rather than a `u32` field so "this entry is version 1" is a *parse*
/// outcome, not a value a reader has to remember to check. A future version 2 record deserializes
/// to `Err` here, which every caller already handles as "ignore this index file" — the same
/// outcome pi reaches by returning `undefined`, and specifically **not** a panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IndexVersion;

impl IndexVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl serde::Serialize for IndexVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for IndexVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported result index version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// One index record: "run R of session S has its payload at file F".
///
/// The same entry is written into up to four index locations (session, run, tool-call,
/// mission-observer) — see [`super::write::write_result_index_for_data`]. Every field is required
/// except `async_dir`, matching pi's parser at `result-files.ts:114-118`, which rejects a record
/// missing any of `version`/`runId`/`sessionId`/`file`/`writtenAt`.
///
/// # Every field is a parsed type
///
/// `session_id` cannot be empty, and `file` cannot be a path traversal — both enforced by their
/// own `Deserialize` impls, so a malformed record on disk fails to deserialize rather than
/// producing a structurally-valid entry that a later consumer must re-validate. That is the whole
/// reason pi needs its `file !== path.basename(file)` guard at three separate call sites and this
/// port needs it at none.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultIndexEntry {
    /// Always [`IndexVersion::VALUE`]; a record with any other version fails to parse.
    pub version: IndexVersion,
    /// The run this index points at.
    pub run_id: RunId,
    /// The session that owns the run — the partition key for `result-index/sessions/`.
    pub session_id: SessionId,
    /// The payload file's name, relative to the results directory. **Trust boundary**: validated
    /// as a single `.json` component at parse.
    pub file: ResultFileName,
    /// Epoch milliseconds at which this entry was written; the retention sweep's age input.
    pub written_at: i64,
    /// The run's async directory, when known. Present iff the writer had one; used to detect a
    /// mission binding (`write_result_index_for_data`) and by readers that want the run's tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub async_dir: Option<PathBuf>,
}

impl ResultIndexEntry {
    /// Parses an entry from raw index-file bytes, returning `None` for anything malformed,
    /// wrong-versioned, or not an object.
    ///
    /// pi `parseResultIndexEntry` (`result-files.ts:111-127`) returns `undefined` for all of
    /// these; the caller's response is uniformly "ignore, and usually unlink". Collapsing every
    /// failure mode into `None` matches that, and deliberately discards the serde error — no
    /// caller distinguishes them, and an index file is a cache that can always be rebuilt.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(bytes).ok()
    }

    /// `true` if this entry actually describes `session_id`/`run_id`.
    ///
    /// pi re-checks this after reading (`result-files.ts:321` `entry.sessionId !== sessionId ||
    /// entry.runId !== runId`, `:343` and `:356` `entry.runId !== runId`), because an index file's
    /// *path* is derived from an encoded — and for long or non-portable values, **hashed** —
    /// segment. Two distinct ids can therefore land on one path only if their digests collide, but
    /// the alias fan-out means a read may also legitimately arrive at a file written for a
    /// different value. Verifying the contents against what was asked for is what makes the hashed
    /// key safe to use as an address.
    #[must_use]
    pub fn describes(&self, session_id: &SessionId, run_id: &RunId) -> bool {
        &self.session_id == session_id && &self.run_id == run_id
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

    use super::*;

    fn entry_json(extra: &str) -> String {
        format!(
            "{{\"version\":1,\"runId\":\"r1\",\"sessionId\":\"s1\",\"file\":\"r1.json\",\"writtenAt\":123{extra}}}"
        )
    }

    #[test]
    fn a_well_formed_entry_parses() {
        let entry = ResultIndexEntry::parse(entry_json("").as_bytes()).expect("valid entry");
        assert_eq!(entry.run_id.as_str(), "r1");
        assert_eq!(entry.session_id.as_str(), "s1");
        assert_eq!(entry.file.as_str(), "r1.json");
        assert_eq!(entry.written_at, 123);
        assert_eq!(entry.async_dir, None);
    }

    #[test]
    fn async_dir_is_optional_and_round_trips() {
        let entry = ResultIndexEntry::parse(entry_json(",\"asyncDir\":\"/tmp/a\"").as_bytes())
            .expect("valid entry");
        assert_eq!(entry.async_dir, Some(PathBuf::from("/tmp/a")));
        let json = serde_json::to_string(&entry).expect("ser");
        let back = ResultIndexEntry::parse(json.as_bytes()).expect("round-trips");
        assert_eq!(back, entry);
    }

    #[test]
    fn the_camel_case_wire_shape_is_preserved() {
        let entry = ResultIndexEntry::parse(entry_json("").as_bytes()).expect("valid");
        let json = serde_json::to_string(&entry).expect("ser");
        assert!(json.contains("\"runId\""), "got {json}");
        assert!(json.contains("\"sessionId\""), "got {json}");
        assert!(json.contains("\"writtenAt\""), "got {json}");
        assert!(json.contains("\"version\":1"), "got {json}");
    }

    #[test]
    fn a_future_version_is_none_not_a_panic() {
        let json = entry_json("").replace("\"version\":1", "\"version\":2");
        assert!(ResultIndexEntry::parse(json.as_bytes()).is_none());
    }

    #[test]
    fn every_required_field_is_required() {
        for missing in ["\"runId\":\"r1\",", "\"sessionId\":\"s1\",", "\"file\":\"r1.json\",", "\"writtenAt\":123"] {
            let json = entry_json("").replace(missing, "");
            assert!(
                ResultIndexEntry::parse(json.as_bytes()).is_none(),
                "removing {missing} must make the entry unparseable"
            );
        }
    }

    #[test]
    fn a_traversal_in_file_makes_the_whole_entry_unparseable() {
        // The trust-boundary property: an index file written by another process cannot smuggle a
        // path out of the results dir, because the entry itself refuses to exist.
        let json = entry_json("").replace("\"r1.json\"", "\"../../etc/passwd.json\"");
        assert!(ResultIndexEntry::parse(json.as_bytes()).is_none());
    }

    #[test]
    fn an_empty_session_id_makes_the_entry_unparseable() {
        let json = entry_json("").replace("\"s1\"", "\"\"");
        assert!(ResultIndexEntry::parse(json.as_bytes()).is_none());
    }

    #[test]
    fn garbage_and_non_objects_are_none() {
        assert!(ResultIndexEntry::parse(b"not json").is_none());
        assert!(ResultIndexEntry::parse(b"[]").is_none());
        assert!(ResultIndexEntry::parse(b"null").is_none());
        assert!(ResultIndexEntry::parse(b"").is_none());
    }

    #[test]
    fn describes_verifies_the_contents_against_the_lookup_key() {
        let entry = ResultIndexEntry::parse(entry_json("").as_bytes()).expect("valid");
        let s1 = SessionId::parse("s1").expect("non-empty");
        let s2 = SessionId::parse("s2").expect("non-empty");
        assert!(entry.describes(&s1, &RunId::from_token("r1")));
        assert!(!entry.describes(&s2, &RunId::from_token("r1")));
        assert!(!entry.describes(&s1, &RunId::from_token("r2")));
    }
}
