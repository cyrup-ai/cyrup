//! The on-disk parallel-handoff manifest — pi `ParallelHandoffManifest` and friends
//! (`shared/types.ts:427-531` @v0.68.0), with pi's `boundedNonEmptyString`/`nonNegativeIndex`
//! runtime checks (`parallel-handoff.ts:38-48,206-247`) dissolved into newtypes.
//!
//! **The JSON is the interface.** Field names stay upstream's (`#[serde(rename_all =
//! "camelCase")]`); Rust identifiers do not have to. Three readers already depend on the exact
//! spelling: this crate's own retention scan
//! ([`crate::background::async_retention`], which pins `version`, a non-empty `groups`, and
//! `groups[].cleanup.state`), pi itself, and the cleanup-plan builder.
//!
//! [CYRUP-DELTA] every record here carries `#[serde(deny_unknown_fields)]`, which pi applies only
//! to lane bindings (`normalizeLaneBinding:52-53`'s `assertKnownFields`); pi's other records
//! silently CARRY an unrecognised key through a rewrite, because it rebuilds the manifest with
//! object spreads. The divergence is deliberate and fails CLOSED: cyrup both writes and reads this
//! file, and an unknown key in a file cyrup wrote is corruption, not forward compatibility. It is
//! recorded here rather than per-field because it is one decision applied uniformly.

use std::num::NonZeroU32;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::error::HandoffError;
use crate::workflows::{
    ManagedWorktreeProvider, WorkflowKey, WorkflowLaneMetadata, WorktreeNaming,
};

// =================================================================================================
// Bounds (pi `parallel-handoff.ts:206-210`)
// =================================================================================================

/// pi `MAX_ATTESTATION_ACTOR_LENGTH` (`:208`).
const MAX_ATTESTATION_ACTOR_BYTES: usize = 128;
/// pi `MAX_ATTESTATION_TIMESTAMP_LENGTH` (`:209`).
const MAX_ATTESTATION_TIMESTAMP_BYTES: usize = 64;
/// pi `MAX_SUPERSESSION_ID_LENGTH` (`:210`) — used for both `laneId` and `supersededBy`.
const MAX_LANE_ID_BYTES: usize = 128;
/// pi `normalizeStoredCleanupEligibility:288`'s `.slice(0, 256)`.
const MAX_BLOCK_REASON_BYTES: usize = 256;

/// pi `boundedString` (`:212-218`): trim, reject empty, reject `\n`/`\r`, bound the UTF-8 length.
fn bounded(value: &str, label: &'static str, max: usize) -> Result<String, HandoffError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(HandoffError::Empty { label });
    }
    if normalized.contains('\n') || normalized.contains('\r') {
        return Err(HandoffError::Newline { label });
    }
    if normalized.len() > max {
        return Err(HandoffError::TooLong { label, max });
    }
    Ok(normalized.to_string())
}

/// Deserializes a newtype THROUGH its own `parse`, the crate idiom pinned at
/// [`WorkflowKey`]'s hand-written impl (`workflows/key.rs:73-78`): "a new deserialized field
/// cannot forget an attribute that does not exist".
macro_rules! deserialize_through_parse {
    ($ty:ty) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                Self::parse(&raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

/// Boilerplate shared by every string newtype below: borrow, `Display`, and the
/// `deserialize-through-parse` impl.
macro_rules! string_newtype_impls {
    ($ty:ty) => {
        impl $ty {
            /// Borrows the validated value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        deserialize_through_parse!($ty);
    };
}

// =================================================================================================
// Newtypes
// =================================================================================================

/// A full 40-hex-digit commit id — pi `commitString` (`:220-224`), `COMMIT_PATTERN` (`:206`).
/// Stored lowercased, exactly as pi normalizes it before the pattern test.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CommitSha(String);

impl CommitSha {
    /// The only fallible constructor.
    ///
    /// # Errors
    ///
    /// [`HandoffError::Empty`]/[`HandoffError::Newline`]/[`HandoffError::TooLong`] from the
    /// shared bounded-string rule, then [`HandoffError::NotACommit`] when the 40-hex shape fails.
    pub fn parse(value: &str) -> Result<Self, HandoffError> {
        Self::parse_labelled(value, "reviewedHead")
    }

    /// [`CommitSha::parse`] with pi's own per-field label, so the error text matches the field
    /// that failed (`merge.reviewedHead`, `merge.mergeCommit`).
    ///
    /// # Errors
    ///
    /// As [`CommitSha::parse`].
    pub fn parse_labelled(value: &str, label: &'static str) -> Result<Self, HandoffError> {
        let normalized = bounded(value, label, 64)?.to_ascii_lowercase();
        if normalized.len() != 40 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(HandoffError::NotACommit { label });
        }
        Ok(Self(normalized))
    }
}
string_newtype_impls!(CommitSha);

/// A full 64-hex-digit SHA-256 digest — pi `manifestDigestString` (`:226-230`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ManifestDigest(String);

impl ManifestDigest {
    /// The only fallible constructor.
    ///
    /// # Errors
    ///
    /// [`HandoffError::NotADigest`] when the 64-hex shape fails, or the bounded-string errors.
    pub fn parse(value: &str) -> Result<Self, HandoffError> {
        Self::parse_labelled(value, "manifestDigest")
    }

    /// [`ManifestDigest::parse`] with pi's per-field label.
    ///
    /// # Errors
    ///
    /// As [`ManifestDigest::parse`].
    pub fn parse_labelled(value: &str, label: &'static str) -> Result<Self, HandoffError> {
        let normalized = bounded(value, label, 64)?.to_ascii_lowercase();
        if normalized.len() != 64 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(HandoffError::NotADigest { label });
        }
        Ok(Self(normalized))
    }
}
string_newtype_impls!(ManifestDigest);

/// Who attested a merge or supersession — pi `boundedString(…, MAX_ATTESTATION_ACTOR_LENGTH)`
/// (`:264`, `:277`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AttestationActor(String);

impl AttestationActor {
    /// The only fallible constructor.
    ///
    /// # Errors
    ///
    /// The bounded-string errors, labelled `attestedBy`.
    pub fn parse(value: &str) -> Result<Self, HandoffError> {
        Ok(Self(bounded(
            value,
            "attestedBy",
            MAX_ATTESTATION_ACTOR_BYTES,
        )?))
    }
}
string_newtype_impls!(AttestationActor);

/// When a merge or supersession was attested — pi `attestationTimestamp` (`:243-247`).
///
/// [CYRUP-DELTA] pi validates with `Number.isFinite(Date.parse(value))`, which accepts ISO-8601
/// **and** a pile of legacy/locale-ish forms (`Date.parse("Dec 2 1999")` is finite). cyrup accepts
/// RFC 3339 only. The divergence is deliberate and behavioural: an attestation record is read back
/// by a second process to decide whether removing a worktree is safe, and two readers that
/// disagree about *when* it happened is exactly the failure this record exists to prevent. A
/// cyrup-written timestamp always round-trips; a pi file carrying a legacy timestamp is refused.
/// The check is hand-rolled rather than pulling in `time`/`chrono` — verified absent from this
/// crate's dependency list — because one field does not justify a date library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AttestationTimestamp(String);

impl AttestationTimestamp {
    /// The only fallible constructor.
    ///
    /// # Errors
    ///
    /// The bounded-string errors, or [`HandoffError::BadTimestamp`] when the RFC 3339 shape fails.
    pub fn parse(value: &str) -> Result<Self, HandoffError> {
        let normalized = bounded(value, "attestedAt", MAX_ATTESTATION_TIMESTAMP_BYTES)?;
        if !is_rfc3339(&normalized) {
            return Err(HandoffError::BadTimestamp);
        }
        Ok(Self(normalized))
    }
}
string_newtype_impls!(AttestationTimestamp);

/// `YYYY-MM-DDThh:mm:ss[.fff](Z|±hh:mm)` — the RFC 3339 `date-time` production, shape-checked
/// including calendar-range bounds on each component.
fn is_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    // `1970-01-01T00:00:00Z` is the shortest legal form.
    if bytes.len() < 20 {
        return false;
    }
    let digits = |from: usize, to: usize| -> Option<u32> {
        value.get(from..to)?.parse::<u32>().ok().filter(|_| {
            value
                .get(from..to)
                .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
        })
    };
    if value.get(4..5) != Some("-") || value.get(7..8) != Some("-") {
        return false;
    }
    // RFC 3339 §5.6 permits `t`/`z` as well as `T`/`Z`.
    if !matches!(value.get(10..11), Some("T" | "t" | " ")) {
        return false;
    }
    if value.get(13..14) != Some(":") || value.get(16..17) != Some(":") {
        return false;
    }
    let (Some(_year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        digits(0, 4),
        digits(5, 7),
        digits(8, 10),
        digits(11, 13),
        digits(14, 16),
        digits(17, 19),
    ) else {
        return false;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    // Hour 24 is not legal in RFC 3339; second 60 is (leap second).
    if hour > 23 || minute > 59 || second > 60 {
        return false;
    }
    let Some(rest) = value.get(19..) else {
        return false;
    };
    // Optional fractional seconds.
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            let taken = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if taken == 0 {
                return false;
            }
            match fraction.get(taken..) {
                Some(tail) => tail,
                None => return false,
            }
        }
        None => rest,
    };
    if matches!(rest, "Z" | "z") {
        return true;
    }
    // `±hh:mm`
    let Some(offset) = rest.strip_prefix(['+', '-']) else {
        return false;
    };
    if offset.len() != 5 || offset.get(2..3) != Some(":") {
        return false;
    }
    let (Some(off_hour), Some(off_minute)) = (
        offset
            .get(0..2)
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok()),
        offset
            .get(3..5)
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok()),
    ) else {
        return false;
    };
    off_hour <= 23 && off_minute <= 59
}

/// A lane identity — the manifest's own `runId`, a `laneId` an attestation is recorded under, and
/// `supersession.supersededBy` all share pi's `MAX_SUPERSESSION_ID_LENGTH` rule (`:210`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct LaneId(String);

impl LaneId {
    /// The only fallible constructor.
    ///
    /// # Errors
    ///
    /// The bounded-string errors, labelled `laneId`.
    pub fn parse(value: &str) -> Result<Self, HandoffError> {
        Self::parse_labelled(value, "laneId")
    }

    /// [`LaneId::parse`] with pi's per-field label (`runId`, `supersession.supersededBy`).
    ///
    /// # Errors
    ///
    /// As [`LaneId::parse`].
    pub fn parse_labelled(value: &str, label: &'static str) -> Result<Self, HandoffError> {
        Ok(Self(bounded(value, label, MAX_LANE_ID_BYTES)?))
    }
}
string_newtype_impls!(LaneId);

impl From<&crate::background::RunId> for LaneId {
    /// A minted [`crate::background::RunId`] is a 32-hex-digit token, so it satisfies the lane-id
    /// rule by construction. A token adopted from a directory name could in principle not;
    /// [`LaneId::parse`] is the route for those.
    fn from(value: &crate::background::RunId) -> Self {
        Self(value.as_str().to_string())
    }
}

/// A cleanup blocker read back off disk — pi `normalizeStoredCleanupEligibility:288`'s
/// trim + newline rejection + **truncation** to 256 bytes (note: pi truncates rather than
/// rejecting, so this constructor is infallible past the empty/newline test).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BoundedReason(String);

impl BoundedReason {
    /// The only fallible constructor. Over-long input is TRUNCATED (pi `.slice(0, 256)`), not
    /// rejected; only empty-after-trim and embedded newlines fail.
    ///
    /// # Errors
    ///
    /// [`HandoffError::Empty`] or [`HandoffError::Newline`].
    pub fn parse(value: &str) -> Result<Self, HandoffError> {
        let normalized = value.trim();
        if normalized.is_empty() {
            return Err(HandoffError::Empty { label: "reason" });
        }
        if normalized.contains('\n') || normalized.contains('\r') {
            return Err(HandoffError::Newline { label: "reason" });
        }
        // `slice(0, 256)` on a char boundary: pi slices UTF-16 code units, cyrup bytes. The
        // ceiling is the same; a multi-byte char straddling it is dropped rather than split.
        let mut end = normalized.len().min(MAX_BLOCK_REASON_BYTES);
        while end > 0 && !normalized.is_char_boundary(end) {
            end -= 1;
        }
        Ok(Self(normalized.get(..end).unwrap_or_default().to_string()))
    }
}
string_newtype_impls!(BoundedReason);

/// pi `merge.prNumber` — "integer >= 1" (`:252`). A [`NonZeroU32`] makes the runtime check a
/// parse-time one; the JSON is still a plain number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PrNumber(NonZeroU32);

impl PrNumber {
    /// The only fallible constructor.
    ///
    /// # Errors
    ///
    /// [`HandoffError::BadPrNumber`] for zero or a negative/over-wide integer.
    pub fn parse(value: i64) -> Result<Self, HandoffError> {
        u32::try_from(value)
            .ok()
            .and_then(NonZeroU32::new)
            .map(Self)
            .ok_or(HandoffError::BadPrNumber)
    }

    /// The underlying PR number.
    #[must_use]
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl<'de> Deserialize<'de> for PrNumber {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = i64::deserialize(deserializer)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// The literal `version: 1` on a [`Manifest`] — same unit-type shape as
/// [`crate::workflows::LaneMetadataVersion`], for the same reason: the retention reader pins
/// `version == 1` (`background/async_retention/scan.rs`), so any other number must fail to parse
/// rather than round-trip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManifestVersion;

impl ManifestVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl Serialize for ManifestVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> Deserialize<'de> for ManifestVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported parallel handoff manifest version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

// =================================================================================================
// Enums (pi string unions)
// =================================================================================================

/// pi `ParallelHandoffManifest["mode"]` (`shared/types.ts:509`).
///
/// [CYRUP-DELTA] deliberately NOT [`crate::background::RunMode`]: that enum has a fourth variant,
/// `Workflow` (`background/state.rs:37`), which upstream's manifest union does not admit. Reusing
/// it would let `"mode":"workflow"` into a file pi refuses to parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HandoffMode {
    /// One task, one worktree.
    Single,
    /// A `tasks[]` fan-out.
    Parallel,
    /// A `chain[]` walk whose steps fanned out.
    Chain,
}

/// pi `ParallelHandoffManifest["source"]` (`shared/types.ts:510`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HandoffSource {
    /// Written by the in-process executor walk.
    Foreground,
    /// Written by the detached background runner.
    Async,
}

/// pi `STORED_CHILD_STATUSES` (`parallel-handoff.ts:293`) — the READ-side union, wider than the
/// write-side `SubagentResultStatus`.
///
/// Both `Complete` and `Completed` are kept because pi's reader accepts both; the cyrup WRITER
/// emits [`ChildStatus::Completed`], which is upstream's own write-side value. That read/write
/// asymmetry is upstream's, ported, not a divergence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChildStatus {
    /// Not started.
    Pending,
    /// In flight.
    Running,
    /// Finished cleanly (the write-side spelling).
    Completed,
    /// Finished cleanly (the alternate spelling pi's reader also accepts).
    Complete,
    /// Ended non-zero.
    Failed,
    /// Soft-interrupted at a pause point.
    Paused,
    /// Hard-stopped.
    Stopped,
    /// Detached from its owner — NOT terminal.
    Detached,
    /// Refused before it ran.
    Rejected,
}

impl ChildStatus {
    /// pi `isTerminalParallelHandoffChildStatus` (`:295-297`).
    ///
    /// NOTE `Detached` is deliberately NOT terminal, and neither are `Pending`/`Running`.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Complete
                | Self::Failed
                | Self::Paused
                | Self::Stopped
                | Self::Rejected
        )
    }
}

/// pi `group.cleanup.state` (`shared/types.ts:502`) — also
/// [`HandoffReference::cleanup_state`]'s value.
///
/// The retention scan pins the literal `"complete"` spelling
/// (`background/async_retention/scan.rs`), so this enum's rename is load-bearing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CleanupState {
    /// Every worktree and branch is gone and the repo was pruned.
    Complete,
    /// Something is still on disk.
    Partial,
}

/// pi `merge.treeEquivalent: boolean | "unknown"` (`shared/types.ts:480`).
///
/// Three states, so a three-variant enum — NOT `Option<bool>`, which would admit `null`. The wire
/// form is a mixed bool/string union, which `#[serde(untagged)]` cannot express on a fieldless
/// enum, so `Serialize`/`Deserialize` are hand-written (the crate idiom, `workflows/key.rs:73`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeEquivalence {
    /// `true` — the merged tree matches the reviewed head.
    Yes,
    /// `false` — it does not.
    No,
    /// `"unknown"` — nobody attested either way.
    Unknown,
}

impl Serialize for TreeEquivalence {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Yes => serializer.serialize_bool(true),
            Self::No => serializer.serialize_bool(false),
            Self::Unknown => serializer.serialize_str("unknown"),
        }
    }
}

impl<'de> Deserialize<'de> for TreeEquivalence {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::Bool(true) => Ok(Self::Yes),
            serde_json::Value::Bool(false) => Ok(Self::No),
            serde_json::Value::String(text) if text == "unknown" => Ok(Self::Unknown),
            // pi `:254` verbatim.
            _ => Err(serde::de::Error::custom(
                "merge.treeEquivalent must be true, false, or 'unknown'.",
            )),
        }
    }
}

/// pi `merge.postMergeChecks: "recorded" | "unknown"` (`shared/types.ts:481`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PostMergeChecks {
    /// The required post-merge checks ran and were recorded.
    Recorded,
    /// Nobody said.
    Unknown,
}

/// pi `CleanupEligibility` (`shared/types.ts:469-474`) — a tagged union.
///
/// The blocker reason lives ONLY on the variant that has one, so "blocked without a reason" is
/// unrepresentable; upstream has to test `state` before touching `reason`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CleanupEligibility {
    /// `{"state":"active"}` — a child owner is still live.
    Active,
    /// `{"state":"terminal-eligible"}` — merge evidence is complete and fresh.
    TerminalEligible,
    /// `{"state":"terminal-blocked","reason":…}`.
    TerminalBlocked {
        /// Why removal is blocked.
        reason: BlockReason,
    },
    /// `{"state":"superseded-eligible"}` — another lane replaced this one.
    SupersededEligible,
    /// `{"state":"unknown"}` — removal is not safe.
    Unknown,
}

/// A [`CleanupEligibility`] as it was found on disk — pi `normalizeStoredCleanupEligibility`
/// (`:284-291`).
///
/// A distinct type from [`CleanupEligibility`] so the compiler enforces the anti-forgery rule:
/// the only way to get a trusted verdict is
/// [`crate::handoff::trusted_cleanup_eligibility`], which re-derives it. Deserialization is
/// LENIENT, exactly as pi is: `readParallelHandoffManifest` never validates this key, so an
/// unrecognised or malformed value must degrade to [`CleanupEligibility::Unknown`] rather than
/// making the whole manifest unreadable. (A strict derive here would turn one corrupt string into
/// "this run's worktrees can never be reasoned about".)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCleanupEligibility(CleanupEligibility);

impl StoredCleanupEligibility {
    /// Wraps a freshly derived verdict for writing.
    #[must_use]
    pub fn new(eligibility: CleanupEligibility) -> Self {
        Self(eligibility)
    }

    /// The stored verdict, UNTRUSTED. Callers deciding whether removal is safe must go through
    /// [`crate::handoff::trusted_cleanup_eligibility`] instead.
    #[must_use]
    pub fn stored(&self) -> &CleanupEligibility {
        &self.0
    }
}

impl Serialize for StoredCleanupEligibility {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredCleanupEligibility {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        Ok(Self(
            serde_json::from_value::<CleanupEligibility>(raw)
                .unwrap_or(CleanupEligibility::Unknown),
        ))
    }
}

/// The eight blocker sentences pi produces (`:366,372,374,378,379,381,383`), plus an `Other` arm
/// for a reason read back from an already-stored manifest.
///
/// Upstream regex-matches one of these at `trustedStoredCleanupEligibility:320`
/// (`/ evidence is stale$/`). Here that becomes [`BlockReason::is_stale`] — a match arm, which
/// cannot drift from the sentences because the sentences are generated from the same enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// pi `:366`.
    StoredMergeEvidenceInvalid,
    /// pi `:379`.
    StoredMergeEvidenceStale,
    /// pi `:374`.
    StoredSupersessionEvidenceInvalid,
    /// pi `:372`.
    StoredSupersessionEvidenceStale,
    /// pi `:378`.
    NoEvidenceRecorded,
    /// pi `:381`.
    TreeEquivalenceNotAttested,
    /// pi `:381` (the `false` arm).
    TreeNotEquivalent,
    /// pi `:383`.
    PostMergeChecksNotRecorded,
    /// A reason string read back from disk that is none of the above.
    Other(BoundedReason),
}

impl BlockReason {
    /// pi's exact sentence for this blocker.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::StoredMergeEvidenceInvalid => "stored merge evidence is invalid",
            Self::StoredMergeEvidenceStale => "stored merge evidence is stale",
            Self::StoredSupersessionEvidenceInvalid => "stored supersession evidence is invalid",
            Self::StoredSupersessionEvidenceStale => "stored supersession evidence is stale",
            Self::NoEvidenceRecorded => "no merge or supersession evidence recorded",
            Self::TreeEquivalenceNotAttested => "merged tree equivalence was not attested",
            Self::TreeNotEquivalent => "merged tree is not equivalent to the reviewed head",
            Self::PostMergeChecksNotRecorded => "required post-merge checks were not recorded",
            Self::Other(reason) => reason.as_str(),
        }
    }

    /// Replaces pi's `/ evidence is stale$/` regex (`:320`): a re-derivation that DOWNGRADES a
    /// stored eligible verdict to one of these two is still honoured, because a stale digest is a
    /// fact about the evidence rather than a disagreement about the verdict.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        match self {
            Self::StoredMergeEvidenceStale | Self::StoredSupersessionEvidenceStale => true,
            // A reason read off disk is judged by the same predicate pi's regex applies.
            Self::Other(reason) => reason.as_str().ends_with(" evidence is stale"),
            _ => false,
        }
    }
}

impl std::fmt::Display for BlockReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for BlockReason {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for BlockReason {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::from_stored(&raw).map_err(serde::de::Error::custom)
    }
}

impl BlockReason {
    /// Re-interns one of pi's eight known sentences, falling back to [`BlockReason::Other`].
    ///
    /// # Errors
    ///
    /// [`HandoffError::Empty`]/[`HandoffError::Newline`] when the fallback's bounds fail — pi
    /// collapses those to `{state:"unknown"}` at `:289` rather than throwing, which
    /// [`CleanupEligibility`]'s stored-normalization path reproduces.
    pub fn from_stored(raw: &str) -> Result<Self, HandoffError> {
        Ok(match raw.trim() {
            "stored merge evidence is invalid" => Self::StoredMergeEvidenceInvalid,
            "stored merge evidence is stale" => Self::StoredMergeEvidenceStale,
            "stored supersession evidence is invalid" => Self::StoredSupersessionEvidenceInvalid,
            "stored supersession evidence is stale" => Self::StoredSupersessionEvidenceStale,
            "no merge or supersession evidence recorded" => Self::NoEvidenceRecorded,
            "merged tree equivalence was not attested" => Self::TreeEquivalenceNotAttested,
            "merged tree is not equivalent to the reviewed head" => Self::TreeNotEquivalent,
            "required post-merge checks were not recorded" => Self::PostMergeChecksNotRecorded,
            other => Self::Other(BoundedReason::parse(other)?),
        })
    }
}

// =================================================================================================
// Records
// =================================================================================================

/// The manifest — pi `ParallelHandoffManifest` (`shared/types.ts:507-519`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Always `1`.
    pub version: ManifestVersion,
    /// The owning run (pi trims and requires non-empty, `:74-75`).
    pub run_id: LaneId,
    /// How the run was launched.
    pub mode: HandoffMode,
    /// Which process wrote this file.
    pub source: HandoffSource,
    /// The shared working directory children were launched from.
    pub cwd: PathBuf,
    /// Epoch millis, stamped once and preserved across rewrites (`:594`).
    pub created_at: i64,
    /// Epoch millis, re-stamped on every write.
    pub updated_at: i64,
    /// One entry per fanned-out group, sorted by `stepIndex`. **Never empty** — this crate's own
    /// retention reader treats an empty `groups` as unresolved-forever, which would pin the run
    /// directory against retention permanently.
    pub groups: Vec<Group>,
    /// Merge attestation, when one has been recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeEvidence>,
    /// Supersession attestation, when one has been recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersession: Option<SupersessionEvidence>,
    /// The last derived eligibility, as it was found on disk.
    ///
    /// `Option` because its ABSENCE is meaningful: `writeParallelHandoffGroup:602-608` omits the
    /// key entirely on a fresh manifest, and `referenceFor:200` `hasOwnProperty`-tests it to
    /// decide whether to project it onto the reference. The value is [`StoredCleanupEligibility`]
    /// rather than [`CleanupEligibility`] because a value read off disk is NEVER trusted — see
    /// [`crate::handoff::trusted_cleanup_eligibility`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_eligibility: Option<StoredCleanupEligibility>,
}

/// One fanned-out group — pi `ParallelHandoffGroup` (`shared/types.ts:492-506`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Group {
    /// The group's position in the chain.
    pub step_index: u32,
    /// `git rev-parse HEAD` at setup time.
    ///
    /// A plain `String`, NOT a [`CommitSha`]: pi only requires non-empty here (`:303`), never
    /// length-checks it, and the value is whatever `rev-parse` printed.
    pub base_commit: String,
    /// The repository toplevel every worktree hangs off.
    pub repo_root: PathBuf,
    /// One row per settled child.
    pub children: Vec<Child>,
    /// Launch-time identity, retained ONLY while the group has no child rows yet (`:570`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_bindings: Option<Vec<LaneBinding>>,
    /// The worktree-removal ledger.
    pub cleanup: WorktreeCleanupReport,
}

/// One settled child — pi `ParallelHandoffChild` (`shared/types.ts:427-444`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Child {
    /// The child's index across the WHOLE manifest (`flatStartIndex + taskIndex`).
    pub index: u32,
    /// The child's index within its group.
    pub task_index: u32,
    /// The agent that ran.
    pub agent: String,
    /// Stable workflow identity used to join status/receipt metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_key: Option<WorkflowKey>,
    /// The child's own run id, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<LaneId>,
    /// Launch-declared lane metadata.
    ///
    /// [`crate::workflows::WorkflowLaneMetadata`] is REUSED rather than redeclared: it is already
    /// `camelCase` + `deny_unknown_fields`, already keys on [`WorkflowKey`], and already carries
    /// pi's 128/160/256-byte bounds (`workflows/types.rs:557-584`). A second lane serialization
    /// for the same JSON object would be a coin flip on which one a reader got.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<WorkflowLaneMetadata>,
    /// The child's outcome.
    pub status: ChildStatus,
    /// The child's delivered text (pi `result.output || result.error || "(no output)"`).
    pub summary: String,
    /// Where the child's `output:` handoff landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<PathBuf>,
    /// The child's validated structured output.
    ///
    /// `Option<Value>` reproduces pi's `!== undefined` test (`:555`) exactly: an absent value is
    /// elided, and `Some(Value::Null)` still serializes `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
    /// Where the structured output was persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output_path: Option<PathBuf>,
    /// The child's session transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_path: Option<PathBuf>,
    /// The captured worktree diff for this child.
    pub patch: Patch,
}

/// The captured diff reference — pi `ParallelHandoffPatch` (`shared/types.ts:413-425`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Patch {
    /// The `.patch` file.
    pub path: PathBuf,
    /// The worktree's branch.
    pub branch: String,
    /// Whether anything actually changed (pi `:565`: any of the four counters non-zero).
    pub changed: bool,
    /// `git diff --cached --stat <base>` output.
    pub diff_stat: String,
    /// Files changed.
    pub files_changed: u64,
    /// Lines inserted.
    pub insertions: u64,
    /// Lines deleted.
    pub deletions: u64,
    /// Why the diff could not be captured, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Launch-time identity retained while a group has no terminal child rows — pi
/// `ParallelHandoffLaneBinding` (`shared/types.ts:446-452`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaneBinding {
    /// The child's manifest-wide index.
    pub index: u32,
    /// The child's index within its group.
    pub task_index: u32,
    /// Stable workflow identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_key: Option<WorkflowKey>,
    /// The child's run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<LaneId>,
    /// Launch-declared lane metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<WorkflowLaneMetadata>,
}

/// One worktree's removal ledger row — pi `WorktreeCleanupTask` (`runs/shared/worktree.ts:82-93`),
/// which is byte-identical to the manifest's `ParallelHandoffCleanupTask`
/// (`shared/types.ts:454-467`).
///
/// Declared HERE rather than in [`crate::spawn::worktree`] because it is the SERIALIZED shape, and
/// the serialization rules (camelCase, `deny_unknown_fields`, absent-not-null elision) belong with
/// the on-disk model. `spawn::worktree` imports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeCleanupTask {
    /// The task's 0-based index within the group.
    pub index: u32,
    /// The worktree root.
    pub path: PathBuf,
    /// The branch `git worktree add -b` created.
    pub branch: String,
    /// LANES_2 — which allocator made this worktree (pi `shared/types.ts:459`, *"omitted in old
    /// manifests"*). cyrup allocates with `git worktree add` only, so its own writer always
    /// records [`ManagedWorktreeProvider::Native`]; the field is `Option` because a manifest
    /// written before this key existed does not carry it, and because pi itself elides it.
    ///
    /// Before this key was declared, `deny_unknown_fields` made a manifest carrying it
    /// UNREADABLE — the interop hole `[EXEC — cleanup]` flagged rather than fixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ManagedWorktreeProvider>,
    /// LANES_2 — the branch/path naming evidence retained with the cleanup authority (pi
    /// `shared/types.ts:461`). Parsed through [`WorktreeNaming`]'s bounded fields, so a manifest
    /// naming a 4 KiB branch prefix is refused on read rather than rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub naming: Option<WorktreeNaming>,
    /// Whether `git worktree remove` succeeded.
    pub worktree_removed: bool,
    /// Whether `git branch -D` succeeded.
    pub branch_removed: bool,
    /// Whether the worktree was deliberately kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserved: Option<bool>,
    /// Why it was kept, or why removal is incomplete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Per-task removal errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
}

/// A group's whole removal ledger — pi `WorktreeCleanupReport` (`runs/shared/worktree.ts:105-110`),
/// serialized as the manifest's inline `group.cleanup` object (`shared/types.ts:501-506`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeCleanupReport {
    /// `complete` only when every task is doubly-removed AND the repo was pruned.
    pub state: CleanupState,
    /// One row per allocated worktree.
    pub tasks: Vec<WorktreeCleanupTask>,
    /// Whether `git worktree prune` succeeded.
    pub pruned: bool,
    /// Group-level removal errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
}

/// Why [`crate::spawn::worktree::cleanup_worktrees`] is running — pi `WorktreeCleanupIntent`
/// (`runs/shared/worktree.ts:95-103`).
///
/// All three of upstream's intents are modelled, each with its payload.
///
/// Both payloads exist for the same reason: they are what the two dirty-worktree gates inside
/// [`crate::spawn::worktree::cleanup_worktrees`] read before allowing a `--force` removal.
/// `discard`'s [`DiscardAuthorization`] is what pi's SECOND authority consult reads
/// (`worktree.ts:1232-1234`); `preserve`'s [`PreserveEvidence`] is what pi's captured-patch gate
/// reads (`worktree.ts:1197-1231`). Dropping either turns the corresponding gate from a refusal
/// into an implicit yes for exactly the worktrees that hold work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeCleanupIntent {
    /// Harvest is done; remove the worktrees, preserving anything whose work the harvest did not
    /// durably capture, and anything that does not fully remove.
    Preserve(PreserveEvidence),
    /// An operator explicitly authorized removing worktrees that were preserved earlier.
    Discard {
        /// How that authorization was obtained, and under which policy — pi
        /// `WorktreeCleanupIntent["authorization"]` (`worktree.ts:97-102`).
        authorization: DiscardAuthorization,
    },
    /// Allocation failed partway; unwind the worktrees created so far.
    ///
    /// Distinct from [`WorktreeCleanupIntent::Preserve`] because an attempted-but-unvalidated
    /// allocation is not a recovery task (pi's own comment at `parallel-handoff.ts:651`), so its
    /// leftovers are reported with a reason naming the rollback rather than a pending handoff.
    SetupRollback,
}

/// What the harvest claims it durably captured, so the preserve cleanup can be CHECKED against it
/// — pi's `capturedDiffs` + `handoffManifestPath` payload (`worktree.ts:96`), passed at
/// `subagent-runner.ts:4426`.
///
/// Removing a worktree that still holds work is only safe if the work survives somewhere else.
/// These two fields are the "somewhere else", and the gate at
/// [`crate::spawn::worktree::cleanup_worktrees`] requires BOTH: the in-memory capture row (which
/// says whether the capture succeeded and which `.patch` it wrote) *and* the manifest that the
/// two-phase write already put on disk (which says the row is durable, not just in this
/// process's memory). Either one alone can be true while the work is unrecoverable — a patch file
/// no manifest names is invisible to every recovery path, and a manifest row naming a patch that
/// was never written is a promise with nothing behind it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PreserveEvidence {
    /// The harvest's per-task capture rows, in task order — pi `capturedDiffs`.
    ///
    /// Upstream falls back to `setup.capturedDiffs` when the intent omits them
    /// (`worktree.ts:1198`); cyrup's [`crate::spawn::worktree::WorktreeSetup`] holds no mutable
    /// capture slot, so the single caller that has the diffs passes them here and the fallback
    /// has nothing to fall back to. An empty vec is therefore "no capture is claimed", which the
    /// gate treats as: preserve every worktree that holds work.
    pub captured_diffs: Vec<crate::spawn::worktree::WorktreeDiff>,
    /// The manifest the harvest's FIRST phase wrote, which must already record each captured
    /// `.patch` — pi `handoffManifestPath`.
    ///
    /// `None` means no durable record exists yet. That is not a neutral value: it makes the gate
    /// refuse every worktree that holds work, because removing one whose only record is in this
    /// process's memory orphans the work if the process dies. It is the honest spelling of what
    /// the earlier `handoff_captured: false` flag claimed to do and did not — that flag chose a
    /// reason string AFTER the removal had already run.
    pub handoff_manifest_path: Option<PathBuf>,
}

/// How a discard came to be authorized — pi's inline union at `worktree.ts:99-102`.
///
/// Two variants, not a `bool`, because the distinction is not "yes/no" but *which gate said yes*:
/// under a `confirm` policy only [`DiscardAuthorizationKind::Confirmed`] clears a worktree that
/// holds uncommitted work (`worktree.ts:1234`), while [`DiscardAuthorizationKind::Policy`] means
/// the policy itself answered `auto` and no human was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscardAuthorization {
    /// Which gate authorized it.
    pub kind: DiscardAuthorizationKind,
    /// The policy in force, so the second consult re-derives the SAME decision the dispatch did
    /// rather than defaulting independently.
    pub policy: Option<crate::registration::authority::AuthorityPolicyConfig>,
}

/// The `kind` half of [`DiscardAuthorization`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscardAuthorizationKind {
    /// The authority policy answered `auto`; nobody was asked.
    Policy,
    /// The policy answered `confirm` and the user said yes.
    Confirmed,
}

/// The compact reference a caller is handed after a write — pi `ParallelHandoffReference`
/// (`shared/types.ts:521-531`).
///
/// This is the value that lands on [`crate::background::RunStatus::parallel_handoff`], and it is
/// how a caller learns the `handoffPath` to pass to a later `worktree.cleanup` / `lane.*` verb
/// (`extension/schemas.ts:298` @v0.68.0: *"Existing manifest for worktree/lane actions"*).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HandoffReference {
    /// Always `1`.
    pub version: ManifestVersion,
    /// The manifest file.
    pub path: PathBuf,
    /// How many groups it holds.
    pub group_count: u32,
    /// How many children across all groups.
    pub child_count: u32,
    /// How many of those children changed anything.
    pub changed_patches: u32,
    /// `complete` iff EVERY group's cleanup is complete.
    pub cleanup_state: CleanupState,
    /// Projected only when the manifest carries the key at all (pi's `hasOwnProperty` test,
    /// `:200`), and re-derived rather than copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_eligibility: Option<CleanupEligibility>,
}

/// Merge attestation — pi `ParallelHandoffMergeEvidence` (`shared/types.ts:476-485`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MergeEvidence {
    /// The PR that merged the lane.
    pub pr_number: PrNumber,
    /// The head the review was performed against.
    pub reviewed_head: CommitSha,
    /// The commit the merge produced.
    pub merge_commit: CommitSha,
    /// Whether the merged tree matches the reviewed head.
    pub tree_equivalent: TreeEquivalence,
    /// Whether the required post-merge checks were recorded.
    pub post_merge_checks: PostMergeChecks,
    /// Who attested.
    pub attested_by: AttestationActor,
    /// When.
    pub attested_at: AttestationTimestamp,
    /// The manifest facts this attestation was made against.
    ///
    /// NEVER accepted from a caller — [`crate::handoff::record_merge`] overwrites it with a
    /// freshly computed digest (pi `:442`'s `{...normalize, manifestDigest}` spread). `Option`
    /// only because pi's normalizer tolerates its absence when reading an older file (`:267`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<ManifestDigest>,
}

/// Supersession attestation — pi `ParallelHandoffSupersessionEvidence`
/// (`shared/types.ts:487-490`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupersessionEvidence {
    /// The lane that replaced this one. Must not be this lane (`:462`).
    pub superseded_by: LaneId,
    /// Who attested.
    pub attested_by: AttestationActor,
    /// When.
    pub attested_at: AttestationTimestamp,
    /// The manifest facts this attestation was made against. Recorder-stamped, never
    /// caller-supplied (`:461`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<ManifestDigest>,
}
