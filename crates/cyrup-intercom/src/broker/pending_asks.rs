//! ICOM-057 — the broker's on-disk pending-ask records (`d69854d`, v0.11.0, issue #104; unchanged
//! through v0.14.0 `broker/broker.ts:31,103-111,167-203,1210-1255`).
//!
//! One JSON file per **delivered blocking ask**, under `<intercomDir>/pending-asks/`, written when
//! the ask is handed to a live target and removed as soon as its ask edge goes away — the reply, a
//! cancel, a dismiss, a mailbox eviction or expiry, or the ask timeout. Nothing in the broker reads
//! them back except the startup/per-send prune: they exist so that something OUTSIDE the broker
//! process (a status line, a supervisor, a human with `ls`) can see which asks are still waiting and
//! on whom, without a broker round-trip.
//!
//! The file is upstream's byte for byte: `JSON.stringify(record, null, 2) + "\n"` at mode `0600` in
//! a `0700` directory, named `encodeURIComponent(messageId).json`, prefixed with
//! `sha256(scopeId)-` for a scoped sender (ICOM-055) so two scopes can never collide on a name.
//!
//! # [CYRUP-DELTA] — which failures are fatal
//!
//! Upstream throws from all three operations. A throw out of the constructor stops the broker from
//! starting, and that is kept: [`PendingAskRecords::open`] returns the error and
//! `lifecycle::run` propagates it. A throw out of `writePendingAskRecord` inside `handleSend` ends up
//! in the frame reader's `onError` and destroys the sender's socket, and that is kept too:
//! [`super::state::BrokerState::write_pending_ask_record`]'s error becomes a protocol error on the
//! sending connection. Removal and the per-send prune are the exception: upstream would also
//! destroy whichever socket happened to trigger them — a replier, a canceller, or ANY sender whose
//! send ran the prune — for a stale observability file it did not create. Those are logged instead.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest as _, Sha256};

use super::extension_state::hex;
use crate::paths::{INTERCOM_DIR_MODE, restrict_intercom_runtime_file};
use crate::transport::protocol::{ScopeId, SessionInfo};

/// `PENDING_ASKS_DIR = join(INTERCOM_DIR, "pending-asks")` (`v0.14.0 broker/broker.ts:31`).
#[must_use]
pub(super) fn pending_asks_dir_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("pending-asks")
}

/// `{ sessionId, name: info.name ?? null }` — one side of a [`PendingAskRecord`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordParty<'a> {
    session_id: &'a str,
    name: Option<&'a str>,
}

/// `PendingAskRecord` (`v0.14.0 broker/broker.ts:103-111`), in upstream's field order — the file is
/// written with `JSON.stringify`, which emits keys in insertion order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingAskRecord<'a> {
    ask_id: &'a str,
    message_id: &'a str,
    asker: RecordParty<'a>,
    target: RecordParty<'a>,
    question: &'a str,
    created_at: u64,
    expires_at: u64,
}

/// The broker's handle on `PENDING_ASKS_DIR`.
pub(super) struct PendingAskRecords {
    dir: PathBuf,
}

impl PendingAskRecords {
    /// The constructor half (`v0.14.0 broker/broker.ts:225-226`): `ensurePendingAskRecordDir();
    /// this.prunePendingAskRecords();` — records a previous broker left behind that have expired
    /// (or that are not records at all) are gone before this broker accepts a connection.
    ///
    /// # Errors
    /// Any I/O failure creating/restricting the directory, listing it, or unlinking a stale file —
    /// upstream throws from its constructor, so the broker does not start.
    pub(super) fn open(dir: PathBuf, now: u64) -> std::io::Result<Self> {
        let records = Self { dir };
        records.prune(now)?;
        Ok(records)
    }

    /// `ensurePendingAskRecordDir` (`v0.14.0 broker/broker.ts:199-204`): `mkdir -p` at `0700`, then
    /// re-`chmod` off Windows.
    fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                &self.dir,
                std::fs::Permissions::from_mode(INTERCOM_DIR_MODE),
            )?;
        }
        Ok(())
    }

    /// `scopedPendingAskRecordPath(scopeId, messageId)` (`v0.14.0 broker/broker.ts:167-173,195-197`).
    fn path(&self, scope: Option<&ScopeId>, message_id: &str) -> PathBuf {
        let name = encode_uri_component(message_id);
        match scope {
            None => self.dir.join(format!("{name}.json")),
            Some(scope) => {
                let scope_hash = hex(&Sha256::digest(scope.as_str().as_bytes()));
                self.dir.join(format!("{scope_hash}-{name}.json"))
            }
        }
    }

    /// `writePendingAskRecord(message, from, target, createdAt)` (`v0.14.0 broker/broker.ts:1210-1224`).
    /// `askId` and `messageId` are both the ask's message id; the record expires when the broker's
    /// ask edge would (`createdAt + askTimeoutMs`), and it is filed under the ASKER's scope.
    ///
    /// # Errors
    /// Any I/O failure creating the directory, writing the file or restricting it to `0600`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn write(
        &self,
        scope: Option<&ScopeId>,
        message_id: &str,
        question: &str,
        asker: &SessionInfo,
        target: &SessionInfo,
        created_at: u64,
        ask_timeout_ms: u64,
    ) -> std::io::Result<()> {
        self.ensure_dir()?;
        let record = PendingAskRecord {
            ask_id: message_id,
            message_id,
            asker: RecordParty {
                session_id: &asker.id,
                name: asker.name.as_deref(),
            },
            target: RecordParty {
                session_id: &target.id,
                name: target.name.as_deref(),
            },
            question,
            created_at,
            expires_at: created_at.saturating_add(ask_timeout_ms),
        };
        let path = self.path(scope, message_id);
        // `JSON.stringify(record, null, 2)` — two-space indentation, which is serde_json's pretty
        // printer's — plus the trailing newline.
        let body = format!("{}\n", serde_json::to_string_pretty(&record)?);
        write_restricted(&path, body.as_bytes())?;
        restrict_intercom_runtime_file(&path)
    }

    /// `removePendingAskRecord(messageId, scopeId)` (`v0.14.0 broker/broker.ts:1226-1234`): an
    /// already-absent file is the ordinary case (an ask to a mailbox, a second cancel), not an error.
    pub(super) fn remove(&self, scope: Option<&ScopeId>, message_id: &str) {
        let path = self.path(scope, message_id);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                %error,
                path = %path.display(),
                "intercom broker: could not remove a pending-ask record"
            ),
        }
    }

    /// `prunePendingAskRecords(now)` (`v0.14.0 broker/broker.ts:1236-1255`): every `*.json` regular
    /// file that does not parse, is not a record, or has expired (`now > expiresAt`) is removed.
    /// Anything else in the directory is left alone.
    ///
    /// # Errors
    /// Creating or listing the directory, or unlinking a file, failed.
    pub(super) fn prune(&self, now: u64) -> std::io::Result<()> {
        self.ensure_dir()?;
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || !entry.file_name().to_string_lossy().ends_with(".json")
            {
                continue;
            }
            let path = entry.path();
            let parsed = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            let keep = parsed
                .as_ref()
                .and_then(record_expires_at)
                .is_some_and(|expires_at| i128::from(now) <= i128::from(expires_at));
            if !keep {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    /// [`Self::prune`] from inside a handler, where upstream's throw would land on an unrelated
    /// connection (see the module docs).
    pub(super) fn prune_logged(&self, now: u64) {
        if let Err(error) = self.prune(now) {
            tracing::warn!(
                %error,
                dir = %self.dir.display(),
                "intercom broker: could not prune pending-ask records"
            );
        }
    }
}

/// `writeFileSync(filePath, …, { mode: INTERCOM_RUNTIME_FILE_MODE })` — the mode applies when the
/// file is created, so a record is never world-readable even for the instant before the `chmod`.
fn write_restricted(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(crate::paths::INTERCOM_RUNTIME_FILE_MODE);
    }
    options.open(path)?.write_all(body)
}

/// `isPendingAskRecord(value)` (`v0.14.0 broker/broker.ts:179-193`), answering with the record's
/// `expiresAt` when it is one. Checked on the raw JSON rather than through a `Deserialize` derive,
/// because upstream rejects an ABSENT `name` (`typeof undefined` is neither `"string"` nor null)
/// where an `Option` field would silently accept it.
fn record_expires_at(value: &serde_json::Value) -> Option<i64> {
    let record = value.as_object()?;
    let party_ok = |key: &str| {
        record
            .get(key)
            .and_then(|v| v.as_object())
            .is_some_and(|party| {
                party
                    .get("sessionId")
                    .is_some_and(serde_json::Value::is_string)
                    && party
                        .get("name")
                        .is_some_and(|name| name.is_string() || name.is_null())
            })
    };
    let string = |key: &str| record.get(key).is_some_and(serde_json::Value::is_string);
    if !(string("askId")
        && string("messageId")
        && party_ok("asker")
        && party_ok("target")
        && string("question"))
    {
        return None;
    }
    let created_at = safe_integer(record.get("createdAt")?)?;
    let expires_at = safe_integer(record.get("expiresAt")?)?;
    (expires_at >= created_at).then_some(expires_at)
}

/// `Number.isSafeInteger(v)`: an integral number within `±(2^53 - 1)`. A JSON `5.0` is the JS number
/// `5` and so qualifies; `5.5` does not.
fn safe_integer(v: &serde_json::Value) -> Option<i64> {
    const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;
    let n = match v.as_i64() {
        Some(n) => n,
        None => {
            let f = v.as_f64()?;
            if f.fract() != 0.0 || f.abs() > MAX_SAFE_INTEGER as f64 {
                return None;
            }
            f as i64
        }
    };
    (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER)
        .contains(&n)
        .then_some(n)
}

/// JS `encodeURIComponent`: every UTF-8 byte except `A-Z a-z 0-9 - _ . ! ~ * ' ( )` becomes `%XX`
/// (upper-case hex). Message ids are client-chosen strings, so a `/` or `..` in one must not reach
/// the filesystem as a path component.
fn encode_uri_component(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            // `write!` to a String is infallible.
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::json;

    #[test]
    fn encode_uri_component_matches_js() {
        // `encodeURIComponent("a/b c?é..!~*'()")` in node.
        assert_eq!(
            encode_uri_component("a/b c?é..!~*'()"),
            "a%2Fb%20c%3F%C3%A9..!~*'()"
        );
        assert_eq!(encode_uri_component("../x"), "..%2Fx");
    }

    /// `scopedPendingAskRecordPath` (`v0.14.0 broker/broker.ts:167-173`): a scoped asker's record is
    /// prefixed with `sha256(scopeId)` in hex; an unscoped one is the bare encoded id.
    #[test]
    fn a_scoped_record_is_prefixed_with_the_scope_hash() {
        let records = PendingAskRecords {
            dir: PathBuf::from("/records"),
        };
        assert_eq!(
            records.path(None, "a/b"),
            PathBuf::from("/records/a%2Fb.json")
        );
        assert_eq!(
            records.path(ScopeId::parse(" team ").as_ref(), "a/b"),
            PathBuf::from(
                "/records/ca8b22d0db83a22db163b560b3e4e51527e533d31d067b614a0c33c4d2df8432-a%2Fb.json"
            )
        );
    }

    #[test]
    fn is_pending_ask_record_mirrors_upstream() {
        let good = json!({
            "askId": "a", "messageId": "a",
            "asker": { "sessionId": "s1", "name": null },
            "target": { "sessionId": "s2", "name": "t" },
            "question": "q", "createdAt": 10, "expiresAt": 10.0
        });
        assert_eq!(record_expires_at(&good), Some(10));
        let mut absent_name = good.clone();
        absent_name["asker"].as_object_mut().unwrap().remove("name");
        assert_eq!(record_expires_at(&absent_name), None, "undefined name");
        let mut backwards = good.clone();
        backwards["expiresAt"] = json!(9);
        assert_eq!(record_expires_at(&backwards), None, "expiresAt < createdAt");
        let mut fractional = good.clone();
        fractional["createdAt"] = json!(1.5);
        assert_eq!(record_expires_at(&fractional), None, "not a safe integer");
        let mut unsafe_int = good;
        unsafe_int["expiresAt"] = json!(9_007_199_254_740_992_u64);
        assert_eq!(record_expires_at(&unsafe_int), None, "2^53 is not safe");
    }
}
