//! The persisted subscription record: its version, its token, its target kind, and the one
//! tolerant parser every read site goes through.
//!
//! Ports pi `WaitSubscriptionRecord` (`shared/types.ts:2189-2198` @`7fe9dee1`) and `parseRecord`
//! (`wait-subscriptions.ts:66-79`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::background::RunId;
use crate::identity::SessionId;

use super::SUBSCRIPTION_VERSION;

/// The literal `1` of [`SUBSCRIPTION_VERSION`], as a type.
///
/// pi's check is one disjunct of `parseRecord`'s eight-term rejection chain
/// (`record.version !== SUBSCRIPTION_VERSION`, `:69`). Expressing it as a `Deserialize` that
/// rejects every other value moves the check to the boundary and makes the parser one fallible
/// constructor — `serde_json::from_slice(..).ok()` — instead of a hand-written field-by-field
/// validator a future field could be forgotten from.
///
/// Two precedents in this crate already agree, and this copies the stricter of the two:
/// [`crate::background::completion_replay::ReplayVersion`] and
/// `extension/executor/foreground_history/record.rs`'s `HistoryVersion`. `SUBSCRIPTION_VERSION` is
/// an ON-DISK FORMAT constant — a record written by a future build must FAIL to parse here rather
/// than be repaired or silently half-read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubscriptionVersion;

impl serde::Serialize for SubscriptionVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(SUBSCRIPTION_VERSION)
    }
}

impl<'de> serde::Deserialize<'de> for SubscriptionVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == SUBSCRIPTION_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported wait-subscription version {raw} (this build reads version \
                 {SUBSCRIPTION_VERSION})"
            )))
        }
    }
}

/// A subscription's identity: a HYPHENATED UUIDv4, and the basename of its own record file.
///
/// # Why this is not a [`RunId`]
///
/// pi mints the token with `randomUUID()` (`:296`) and re-validates it on every read against
/// `/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i` (`:71`).
/// [`RunId::new`] mints `uuid::Uuid::new_v4().as_simple()` — 32 hex characters with NO hyphens —
/// which that regex rejects. A token minted through `RunId` would therefore produce a record pi
/// cannot parse, in a directory whose whole purpose is being read by whichever build resumes the
/// session. The hyphenated spelling is the FORMAT, not a style choice.
///
/// [`Self::parse`] is the only constructor that accepts foreign input, and it applies the regex's
/// exact shape, so a hand-edited file cannot smuggle in a token that disagrees with its own file
/// name — which is the check `:154`/`:326` then perform.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct SubscriptionToken(Arc<str>);

impl SubscriptionToken {
    /// pi `randomUUID()` (`:296`). `hyphenated()` is inherent on `uuid::Uuid` — no extra feature,
    /// and the `uuid` dependency is already pinned by [`RunId::new`].
    #[must_use]
    pub fn mint() -> Self {
        Self(Arc::from(
            uuid::Uuid::new_v4().hyphenated().to_string().as_str(),
        ))
    }

    /// The only fallible constructor — pi `:71`'s regex, spelled out.
    ///
    /// Hand-written rather than pulled through a regex crate because the pattern is fixed-width
    /// and fully positional: 36 bytes, dashes at 8/13/18/23, a version nibble in `1..=5` at 14, a
    /// variant nibble in `[89ab]` at 19, hex everywhere else. Case-insensitive, matching the
    /// upstream `/i` flag.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        if !is_upstream_uuid(raw) {
            return None;
        }
        Some(Self(Arc::from(raw)))
    }

    /// Borrows the token for comparison, display and file-name construction.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// pi `:71`'s regex as a positional scan. See [`SubscriptionToken::parse`].
fn is_upstream_uuid(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let ok = match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            // The VERSION nibble: upstream accepts `[1-5]`, not just `4`, so a token minted by an
            // older pi build (or by a different UUID version) round-trips rather than being
            // silently dropped on read.
            14 => byte.is_ascii_digit() && *byte != b'0' && *byte <= b'5',
            // The VARIANT nibble: `[89ab]`, case-insensitively.
            19 => matches!(byte.to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b'),
            _ => byte.is_ascii_hexdigit(),
        };
        if !ok {
            return false;
        }
    }
    true
}

impl std::fmt::Display for SubscriptionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes **through** [`SubscriptionToken::parse`], for the same reason
/// [`SessionId`]'s own `Deserialize` does: these bytes came off a shared directory this process
/// may not have written, so deserialization IS the validation boundary.
impl<'de> serde::Deserialize<'de> for SubscriptionToken {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(format!("not a wait-subscription token: {raw:?}"))
        })
    }
}

/// pi's `targetKind: "async" | "foreground"` (`shared/types.ts:2193`).
///
/// An enum, so `:73`'s `(record.targetKind !== "async" && record.targetKind !== "foreground")`
/// disjunct becomes an unrepresentable state rather than a check every reader has to remember.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitTargetKind {
    /// A detached background run, reconciled through [`crate::background::run_status`].
    Async,
    /// A foreground run of THIS process, reconciled through the injected
    /// [`super::ForegroundSubscriptionProbe`].
    Foreground,
}

impl WaitTargetKind {
    /// The wire word, which is also what [`super::format_wait_subscriptions`] renders (`:94`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WaitTargetKind::Async => "async",
            WaitTargetKind::Foreground => "foreground",
        }
    }
}

impl std::fmt::Display for WaitTargetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// pi `WaitSubscriptionRecord` (`shared/types.ts:2189-2198`) — the whole on-disk format.
///
/// Four of upstream's eight `parseRecord` disjuncts (`:69-77`) are enforced by the TYPES above and
/// do not appear as runtime checks anywhere in this module: the version, the token's shape, the
/// target kind, and the session id's non-emptiness. That is deliberate — see each field.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitSubscriptionRecord {
    /// pi `version: 1` (`:2190`). See [`SubscriptionVersion`].
    pub version: SubscriptionVersion,
    /// pi `token` (`:2191`). Also this record's file name — see [`subscription_file`].
    pub token: SubscriptionToken,
    /// pi `sessionId` (`:2192`), and pi's `:72` `typeof record.sessionId !== "string"` gate
    /// enforced by the type.
    ///
    /// **Required, and a parsed [`SessionId`], never `Option<String>`.** This is the identical
    /// argument `foreground_history/record.rs:128-134` already makes for
    /// `ForegroundHistoryRun::session_id`: a record with no session cannot be CONSTRUCTED, so it
    /// cannot be persisted, and a file on disk carrying a null/empty session fails to deserialize
    /// instead of restoring as a subscription that belongs to nobody. `SessionId::parse` rejects
    /// the empty string and nothing else, which is exactly pi's `typeof … === "string"` plus its
    /// falsy check.
    pub session_id: SessionId,
    /// pi `targetKind` (`:2193`).
    pub target_kind: WaitTargetKind,
    /// pi `runId` (`:2194`) — the EXACT, already-resolved run identity this subscription binds.
    pub run_id: RunId,
    /// pi `requestedId` (`:2195`) — the raw prefix the caller typed, kept SEPARATELY from
    /// [`Self::run_id`] and only ever rendered. Collapsing the two would lose the audit of what
    /// the user actually asked for, which is why upstream carries both.
    pub requested_id: String,
    /// pi `createdAt` (`:2196`), from [`crate::time::now_epoch_millis`]. The
    /// [`super::format_wait_subscriptions`] sort key.
    pub created_at: i64,
    /// pi `expiresAt` (`:2197`) — `createdAt + timeoutMs`.
    pub expires_at: i64,
}

/// pi `ArmWaitSubscriptionInput` (`:39-44`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmWaitSubscriptionInput {
    /// pi `targetKind`.
    pub target_kind: WaitTargetKind,
    /// pi `runId` — the resolved EXACT id, never a prefix.
    pub run_id: RunId,
    /// pi `requestedId` — the caller's raw selector.
    pub requested_id: String,
    /// pi `timeoutMs`.
    pub timeout_ms: u64,
}

/// pi `subscriptionFile` (`:85-87`) — `<dir>/<token>.json`.
///
/// The token's own [`Display`](std::fmt::Display) is the basename, which is what makes
/// `:154`/`:326`'s `file !== \`${record.token}.json\`` re-check meaningful: a record can only be
/// addressed by the name it claims.
#[must_use]
pub fn subscription_file(dir: &Path, token: &SubscriptionToken) -> PathBuf {
    dir.join(format!(
        "{token}{}",
        crate::identity::ResultFileName::EXTENSION
    ))
}

/// pi `parseRecord` (`:66-79`).
///
/// **Tolerant by contract**: these bytes were written by a possibly-different build, so malformed
/// JSON, a missing field, an empty `sessionId`, a malformed token, an unknown `targetKind` and an
/// unknown `version` all collapse to `None` — never an `Err`, never a panic. Every one of
/// upstream's eight disjuncts is either a field of [`WaitSubscriptionRecord`] (so its absence is a
/// parse failure) or a `Deserialize` impl above that rejects the bad value.
#[must_use]
pub fn parse_record(bytes: &[u8]) -> Option<WaitSubscriptionRecord> {
    serde_json::from_slice(bytes).ok()
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

    fn sample_json(session: &str, token: &str) -> String {
        format!(
            r#"{{"version":1,"token":"{token}","sessionId":"{session}","targetKind":"async",
               "runId":"run-1","requestedId":"run","createdAt":10,"expiresAt":20}}"#
        )
    }

    const TOKEN: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

    /// The minted token is the HYPHENATED spelling upstream's `:71` regex accepts — and the
    /// `RunId` minter is NOT, which is the whole reason this newtype exists.
    #[test]
    fn a_minted_token_matches_upstreams_regex_and_a_run_id_does_not() {
        for _ in 0..16 {
            let token = SubscriptionToken::mint();
            assert!(
                SubscriptionToken::parse(token.as_str()).is_some(),
                "minted token must round-trip through parse: {token}"
            );
            assert_eq!(token.as_str().len(), 36);
        }
        // `RunId::new` is `as_simple()` — 32 hex chars, no hyphens. Upstream would drop it.
        let run_id = RunId::new();
        assert!(SubscriptionToken::parse(run_id.as_str()).is_none());
    }

    /// The positional scan is the regex, not "anything with dashes in it".
    #[test]
    fn the_token_parser_rejects_every_way_the_regex_can_fail() {
        assert!(SubscriptionToken::parse(TOKEN).is_some());
        // Upper case is accepted (`/i`).
        assert!(SubscriptionToken::parse(&TOKEN.to_uppercase()).is_some());
        // Version nibble outside `[1-5]`.
        assert!(SubscriptionToken::parse("3f2504e0-4f89-01d3-9a0c-0305e82c3301").is_none());
        assert!(SubscriptionToken::parse("3f2504e0-4f89-61d3-9a0c-0305e82c3301").is_none());
        // Variant nibble outside `[89ab]`.
        assert!(SubscriptionToken::parse("3f2504e0-4f89-41d3-0a0c-0305e82c3301").is_none());
        // A dash in the wrong place, and the wrong length.
        assert!(SubscriptionToken::parse("3f2504e04f89-41d3-9a0c-0305e82c33011").is_none());
        assert!(SubscriptionToken::parse("").is_none());
        assert!(SubscriptionToken::parse(&format!("{TOKEN}0")).is_none());
        // Non-hex.
        assert!(SubscriptionToken::parse("zf2504e0-4f89-41d3-9a0c-0305e82c3301").is_none());
    }

    /// Gate `:72` as a TYPE: a record with an empty or absent `sessionId` does not deserialize, so
    /// it can never reach the in-memory map at all.
    #[test]
    fn a_record_with_no_session_id_is_invalid() {
        assert!(parse_record(sample_json("sess-a", TOKEN).as_bytes()).is_some());
        // Empty string — `SessionId::parse` rejects it, so the whole record fails.
        assert!(parse_record(sample_json("", TOKEN).as_bytes()).is_none());
        // Key absent entirely.
        let absent = format!(
            r#"{{"version":1,"token":"{TOKEN}","targetKind":"async","runId":"run-1",
               "requestedId":"run","createdAt":10,"expiresAt":20}}"#
        );
        assert!(parse_record(absent.as_bytes()).is_none());
        // Explicit null.
        let null =
            sample_json("sess-a", TOKEN).replace(r#""sessionId":"sess-a""#, r#""sessionId":null"#);
        assert!(parse_record(null.as_bytes()).is_none());
    }

    /// The other four `parseRecord` disjuncts that this port turned into types.
    #[test]
    fn the_parser_rejects_a_bad_version_token_or_target_kind() {
        let good = sample_json("sess-a", TOKEN);
        assert!(parse_record(good.as_bytes()).is_some());
        assert!(
            parse_record(good.replace(r#""version":1"#, r#""version":2"#).as_bytes()).is_none()
        );
        assert!(parse_record(good.replace(TOKEN, "not-a-uuid").as_bytes()).is_none());
        assert!(
            parse_record(
                good.replace(r#""targetKind":"async""#, r#""targetKind":"chain""#)
                    .as_bytes()
            )
            .is_none()
        );
        assert!(parse_record(b"{ not json").is_none());
        // A field of the wrong type is a rejection too, not a default.
        assert!(
            parse_record(
                good.replace(r#""createdAt":10"#, r#""createdAt":"10""#)
                    .as_bytes()
            )
            .is_none()
        );
    }

    /// The file name IS the token — the property `:154`/`:326` re-check on every read.
    #[test]
    fn the_record_file_is_named_for_its_token() {
        let token = SubscriptionToken::parse(TOKEN).unwrap();
        assert_eq!(
            subscription_file(Path::new("/subs"), &token),
            Path::new("/subs/3f2504e0-4f89-41d3-9a0c-0305e82c3301.json")
        );
    }

    /// A record round-trips byte-for-byte through the camelCase wire shape pi reads.
    #[test]
    fn a_record_round_trips_through_pis_wire_shape() {
        let record = WaitSubscriptionRecord {
            version: SubscriptionVersion,
            token: SubscriptionToken::parse(TOKEN).unwrap(),
            session_id: SessionId::parse("sess-a").unwrap(),
            target_kind: WaitTargetKind::Foreground,
            run_id: RunId::from_token("run-1"),
            requested_id: "run".to_string(),
            created_at: 10,
            expires_at: 20,
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["version"], serde_json::json!(1));
        assert_eq!(json["targetKind"], serde_json::json!("foreground"));
        assert_eq!(json["requestedId"], serde_json::json!("run"));
        assert_eq!(json["expiresAt"], serde_json::json!(20));
        let bytes = serde_json::to_vec(&record).unwrap();
        assert_eq!(parse_record(&bytes), Some(record));
    }
}
