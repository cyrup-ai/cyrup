//! The persisted, revision-checked extension state store — a 1:1 port of
//! `pi-intercom/broker/extension-state.ts` (v0.9.2 and v0.12.0 are identical here apart from a
//! local-variable refactor in `readEnvelope`).
//!
//! One file per namespace under `<intercomDir>/extension-state/`, named by the sha256 of the
//! namespace, written through a temp file + `fsync` + rename with a `.bak` of the previous
//! envelope, and integrity-checked on read by re-hashing the payload. [`ExtensionStateManager`]
//! caches what it reads, which is why the broker owns exactly one of them.
//!
//! Pure — no `BrokerState`, no sockets — exactly like upstream.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::limits::MAX_EXTENSION_STATE_BYTES;

/// `StateEnvelope` (`extension-state.ts:18-25`).
///
/// `formatVersion` is not modelled as a typed constant: it is written literally as `1` and any other
/// value fails the read, which [`ExtensionStateManager::read_envelope`] checks by hand.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StateEnvelope {
    format_version: u8,
    namespace: String,
    revision: u64,
    updated_at: u64,
    payload_sha256: String,
    payload: serde_json::Value,
}

/// `StateCommitResult` (`extension-state.ts:27-32`).
///
/// [`Self::payload`] is the CURRENT payload returned alongside a `"Revision mismatch"`; the broker
/// does not put it on the wire (`v0.9.2 broker/broker.ts:1478-1484` echoes only
/// committed/revision/reason), so it exists for the manager's contract, not for a frame.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StateCommitResult {
    pub(super) committed: bool,
    pub(super) revision: u64,
    pub(super) reason: Option<&'static str>,
    pub(super) payload: Option<serde_json::Value>,
}

/// One namespace's cached state (`extension-state.ts:51`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NamespaceState {
    pub(super) revision: u64,
    pub(super) payload: serde_json::Value,
}

/// `serializePayload` (`extension-state.ts:34-44`) WITHOUT its 64 KiB cap, plus
/// `serializedPayloadSize` (`v0.9.2 broker/broker.ts:44-51`) — one function, because upstream's two
/// differ only in whether the cap is applied and the callers apply different caps (16 KiB for a
/// publish, 64 KiB for a commit).
///
/// `None` is pi's `undefined`/`null` return: an ABSENT payload. `JSON.stringify(undefined)` is
/// `undefined`, so a frame with no `payload` key is refused by both call sites — that is upstream
/// behaviour, not a strictness the port adds.
///
/// [CYRUP-DELTA] `Buffer.byteLength(json, "utf8")` vs `String::len()`: both count UTF-8 bytes, and
/// `serde_json` escapes the same characters `JSON.stringify` does, so the two lengths agree for
/// every value that can come off the wire. Key ORDER differs (`serde_json`'s `Map` is sorted, JS
/// objects keep insertion order) and the length is order-invariant; the persisted envelope's payload
/// is re-serialized from the same `Map`, so its hash round-trips within this implementation.
pub(super) fn serialize_payload(payload: Option<&serde_json::Value>) -> Option<String> {
    serde_json::to_string(payload?).ok()
}

/// `createHash("sha256").update(json).digest("hex")` (`extension-state.ts:46`).
fn payload_hash(payload_json: &str) -> String {
    hex(&Sha256::digest(payload_json.as_bytes()))
}

/// Lowercase hex, matching node's `digest("hex")`. A local helper rather than a `hex` crate edge.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut acc, b| {
        // `write!` to a String is infallible; the Result is discarded deliberately.
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// `ExtensionStateManager` (`extension-state.ts:48-197`).
#[derive(Debug)]
pub(super) struct ExtensionStateManager {
    states: HashMap<String, NamespaceState>,
    state_dir: PathBuf,
}

impl ExtensionStateManager {
    /// `new ExtensionStateManager(INTERCOM_DIR)` (`extension-state.ts:53-56`):
    /// `mkdirSync(stateDir, { recursive: true, mode: 0o700 })`.
    ///
    /// Upstream's `mkdirSync` THROWS out of the constructor, i.e. the broker never starts. Here the
    /// failure is logged and deferred to the first commit, which already carries upstream's own
    /// `"Failed to persist extension state"` refusal for it — [`super::state::BrokerState::new`] is
    /// infallible, and every non-persisting bus operation (owner election, publish fan-out) works
    /// fine with an unwritable state dir, so refusing to start would be strictly worse than
    /// upstream.
    pub(super) fn new(state_dir: PathBuf) -> Self {
        if let Err(error) = std::fs::create_dir_all(&state_dir) {
            tracing::warn!(
                error = %error,
                dir = %state_dir.display(),
                "intercom broker: extension state dir unavailable; commits will be refused"
            );
        } else if let Err(error) = restrict_state_dir(&state_dir) {
            tracing::warn!(
                error = %error,
                dir = %state_dir.display(),
                "intercom broker: could not restrict extension state dir to 0700"
            );
        }
        Self { states: HashMap::new(), state_dir }
    }

    /// `statePath` (`extension-state.ts:59-62`) — the file is NAMED by the namespace's hash, so a
    /// namespace containing `/` or `..` can never escape the directory.
    fn state_path(&self, namespace: &str) -> PathBuf {
        self.state_dir.join(format!("{}.json", hex(&Sha256::digest(namespace.as_bytes()))))
    }

    /// `backupPath` (`extension-state.ts:64-66`) — `${statePath}.bak`.
    fn backup_path(&self, namespace: &str) -> PathBuf {
        let mut path = self.state_path(namespace).into_os_string();
        path.push(".bak");
        PathBuf::from(path)
    }

    /// `readEnvelope` (`extension-state.ts:68-108`).
    ///
    /// Every rejection upstream makes, in its order: unreadable or unparseable file, a non-object
    /// value, `formatVersion !== 1`, a namespace that does not match, a `revision` that is not a
    /// non-negative safe integer, a non-numeric `updatedAt`, a non-string `payloadSha256`, and a
    /// payload that re-serializes to a different hash.
    ///
    /// Checked field-by-field against a [`serde_json::Value`] rather than a `#[derive]`: the
    /// safe-integer bound on `revision` and the `formatVersion` literal are not expressible as
    /// derives, and routing the revision through [`super::js::js_safe_u64`] keeps one definition of
    /// that rule.
    fn read_envelope(&self, path: &Path, namespace: &str) -> Option<NamespaceState> {
        let raw = std::fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let obj = value.as_object()?;

        if obj.get("formatVersion").and_then(serde_json::Value::as_u64) != Some(1) {
            return None;
        }
        if obj.get("namespace").and_then(|v| v.as_str()) != Some(namespace) {
            return None;
        }
        let revision = super::js::js_safe_u64(obj.get("revision"))?;
        // `typeof envelope.updatedAt !== "number"` — the value is not otherwise used on read.
        obj.get("updatedAt").and_then(serde_json::Value::as_number)?;
        let stored_hash = obj.get("payloadSha256").and_then(|v| v.as_str())?;

        // An ABSENT `payload` key is `undefined` upstream, and `JSON.stringify(undefined)` is
        // `undefined`, so `serializePayload` returns null and `readEnvelope` REJECTS the envelope
        // (`extension-state.ts:92-95`) — it does NOT read the key as `null`. An explicit `null`
        // VALUE is legal and hashes as `"null"`, so it is the key's presence that is tested here.
        let payload = obj.get("payload")?;
        // The 64 KiB cap is applied at THIS call site rather than inside [`serialize_payload`],
        // because that function is shared with the publish path, which bounds at 16 KiB instead.
        // Upstream keeps the cap inside `serializePayload` (`:34-44`), which is why its
        // `readEnvelope` rejects an oversized stored envelope even when the hash matches and falls
        // through to the `.bak`; without this filter the port would accept, cache and replay one.
        let payload_json =
            serialize_payload(Some(payload)).filter(|j| j.len() <= MAX_EXTENSION_STATE_BYTES)?;
        if payload_hash(&payload_json) != stored_hash {
            return None;
        }
        Some(NamespaceState { revision, payload: payload.clone() })
    }

    /// `loadState` (`extension-state.ts:110-121`): the cache, then the primary file, then the
    /// `.bak`, caching whatever it found.
    pub(super) fn load_state(&mut self, namespace: &str) -> Option<&NamespaceState> {
        if !self.states.contains_key(namespace) {
            let found = self
                .read_envelope(&self.state_path(namespace), namespace)
                .or_else(|| self.read_envelope(&self.backup_path(namespace), namespace));
            if let Some(state) = found {
                self.states.insert(namespace.to_string(), state);
            }
        }
        self.states.get(namespace)
    }

    /// `getCurrentRevision` (`extension-state.ts:194-196`).
    pub(super) fn current_revision(&mut self, namespace: &str) -> u64 {
        self.load_state(namespace).map_or(0, |s| s.revision)
    }

    /// `commitState` (`extension-state.ts:123-192`) — a compare-and-swap with three refusals and an
    /// atomic write.
    ///
    /// `now` is passed in rather than read here, matching every other `now` in the broker
    /// (`protocol::now_ms()` at the call site).
    ///
    /// Upstream's `"Invalid expected revision"` refusal (`:132-134`) is UNREACHABLE from the broker:
    /// [`super::js::js_safe_u64`] has already rejected every value that would trip it, so the `u64`
    /// parameter type IS that check. No second guard is added for a branch that cannot fire.
    pub(super) fn commit_state(
        &mut self,
        namespace: &str,
        expected_revision: u64,
        payload: Option<&serde_json::Value>,
        now: u64,
    ) -> StateCommitResult {
        let payload_json = serialize_payload(payload).filter(|j| j.len() <= MAX_EXTENSION_STATE_BYTES);
        let current = self.load_state(namespace).cloned();
        let current_revision = current.as_ref().map_or(0, |s| s.revision);

        let Some(payload_json) = payload_json else {
            return StateCommitResult {
                committed: false,
                revision: current_revision,
                reason: Some("Invalid extension state or payload exceeds 64 KiB limit"),
                payload: None,
            };
        };
        if expected_revision != current_revision {
            return StateCommitResult {
                committed: false,
                revision: current_revision,
                reason: Some("Revision mismatch"),
                payload: current.map(|s| s.payload),
            };
        }

        let revision = current_revision + 1;
        let payload_value = payload.cloned().unwrap_or(serde_json::Value::Null);
        let envelope = StateEnvelope {
            format_version: 1,
            namespace: namespace.to_string(),
            revision,
            updated_at: now,
            payload_sha256: payload_hash(&payload_json),
            payload: payload_value.clone(),
        };

        match self.persist(namespace, &envelope) {
            Ok(()) => {
                self.states
                    .insert(namespace.to_string(), NamespaceState { revision, payload: payload_value });
                StateCommitResult { committed: true, revision, reason: None, payload: None }
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    namespace,
                    "intercom broker: failed to persist extension state"
                );
                StateCommitResult {
                    committed: false,
                    revision: current_revision,
                    reason: Some("Failed to persist extension state"),
                    payload: None,
                }
            }
        }
    }

    /// The atomic write half of `commitState` (`extension-state.ts:151-186`): temp file, fsync,
    /// back up the previous envelope, rename, then a best-effort directory fsync.
    ///
    /// The temp file is removed on EVERY path, matching upstream's `finally` (`:189-191`).
    fn persist(&self, namespace: &str, envelope: &StateEnvelope) -> std::io::Result<()> {
        let state_path = self.state_path(namespace);
        let mut tmp = state_path.clone().into_os_string();
        tmp.push(format!(".tmp.{}.{}", std::process::id(), uuid::Uuid::new_v4()));
        let tmp = PathBuf::from(tmp);

        let result = self.persist_inner(&state_path, &tmp, namespace, envelope);
        // upstream's `finally { rmSync(tempPath, { force: true }) }` — never leak the temp file,
        // including on the error path.
        let _ = std::fs::remove_file(&tmp);
        result
    }

    fn persist_inner(
        &self,
        state_path: &Path,
        tmp: &Path,
        namespace: &str,
        envelope: &StateEnvelope,
    ) -> std::io::Result<()> {
        let body = serde_json::to_string(envelope)
            .map_err(|e| std::io::Error::other(format!("serialize state envelope: {e}")))?;
        // The port's own write-then-restrict idiom (`broker/lifecycle.rs:186-187`); the 0600 mode is
        // a no-op off POSIX.
        std::fs::write(tmp, body)?;
        crate::paths::restrict_intercom_runtime_file(tmp)?;
        std::fs::File::open(tmp)?.sync_all()?;

        // `if (this.readEnvelope(statePath, namespace)) copyFileSync(statePath, backupPath)`
        // (`:174-176`) — only a VALID current envelope is worth keeping as the fallback.
        if self.read_envelope(state_path, namespace).is_some() {
            std::fs::copy(state_path, self.backup_path(namespace))?;
        }
        std::fs::rename(tmp, state_path)?;

        // "Directory fsync is unavailable on some platforms." (`:181-185`) — best effort, swallowed.
        if let Some(dir) = state_path.parent()
            && let Ok(handle) = std::fs::File::open(dir)
        {
            let _ = handle.sync_all();
        }
        Ok(())
    }
}

/// `mode: 0o700` on the state directory (`extension-state.ts:55`). A no-op off POSIX, matching
/// [`crate::paths::restrict_intercom_runtime_file`]'s own platform split.
#[cfg(unix)]
fn restrict_state_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_state_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}
