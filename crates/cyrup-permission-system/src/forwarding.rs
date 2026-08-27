//! The child→parent permission-forwarding transport — a faithful 1:1 port of pi's OWN
//! request/response spool (`permission-forwarding.ts` + `index.ts:1030-1504`), NOT the subagents
//! interrupt channel (port doc §0/§7.4). A subagent runs headless (no local human), so an `ask`
//! firing inside the child is forwarded up to the PARENT session's human via a shared-filesystem
//! spool rather than dying.
//!
//! ## Layout (pi `permission-forwarding.ts:74-127`)
//!
//! ```text
//! <agentDir>/sessions/permission-forwarding/sessions/<urlencode(sessionId)>/
//!     requests/<requestId>.json      (child writes; parent reads+deletes)
//!     responses/<requestId>.json     (parent writes; child reads+deletes)
//! ```
//!
//! Both sides resolve the SAME path from the SAME `agentDir` and the PARENT's session id (the child
//! addresses the parent's inbox by the `CYRUP_SUBAGENT_PARENT_SESSION` anchor; the parent addresses
//! its own inbox by `HostServices::session_id()`).
//!
//! ## Security (pi, port doc §7.4/§11)
//!
//! - **256-bit response nonce** (pi `randomBytes(32).toString("base64url")`, `index.ts:1135-1137`) +
//!   **constant-time** binding (`safeEqualString`/`timingSafeEqual`, `index.ts:1139-1143`): a hostile
//!   sibling cannot forge an approval without the nonce the child generated.
//! - **`wx` (no-clobber) + `0o600` atomic write** (pi `writeJsonFileAtomic`, `index.ts:1166-1178`):
//!   a per-write `<path>.<pid>.<uuid>.tmp` created O_EXCL, chmod 600, renamed into place.
//! - **`0o700` session dirs** (pi `ensureDirectoryExists`, `index.ts:1030-1039`).
//! - **safe-token + contains-root guard** on the encoded session id BEFORE any `Path::join`
//!   (reusing `cyrup_ext_subagents::{validate_safe_token, validate_contains_root}`, P-5, R-PERM-040).
//!
//! ## Reuse (P-5 building blocks)
//!
//! `validate_safe_token` / `validate_contains_root` / `CONTROL_INBOX_POLL_INTERVAL` come from
//! `cyrup-ext-subagents::background::control`; the request-inbox directory watch mirrors that module's
//! `watch_control_inbox` `notify::PollWatcher` pattern. The spool MECHANISM is pi's own.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use cyrup_ext::{HostServices, NotifyKind};
use cyrup_ext_subagents::{validate_contains_root, validate_safe_token, CONTROL_INBOX_POLL_INTERVAL};

use crate::ask::{
    AskChannel, AskOutcome, LocalAskChannel, PermissionDecisionState, PermissionPromptDecision,
    PromptOpts,
};
use crate::error::PermissionError;
use crate::ext_config::ExtensionConfig;
use crate::logging::{sensitive_log_metadata, AuditTrail};

/// pi `PERMISSION_FORWARDING_TIMEOUT_MS = 10 * 60 * 1000` (`permission-forwarding.ts:7`): the CHILD's
/// blocking-wait deadline AND the parent's expired-on-read cutoff.
pub const PERMISSION_FORWARDING_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The ops/test override for the child wait bound (defaults to the pi-faithful
/// [`PERMISSION_FORWARDING_TIMEOUT`]). A finite positive milliseconds value shortens the child's
/// deadline — the seam the fail-closed timeout proof (`tests/forwarding_subprocess.rs`) drives so the
/// 10-minute production default never has to elapse in a test.
pub const CHILD_WAIT_TIMEOUT_ENV: &str = "CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS";

/// pi `PERMISSION_FORWARDING_AGENT_DIR_ENV_KEY` (`permission-forwarding.ts:11` @v0.8.0) — the cyrup
/// analog of the one non-subagent-scoped agent-dir override (the explicit, always-consulted level of
/// pi's 5-level precedence, `permission-forwarding.ts:62-90`). The default (the passed `agent_dir`,
/// pi `defaultAgentDir`) is the last level.
///
/// **PERM-017 — the three middle levels, restated after PERM-025 landed.** Upstream's chain is
/// `PERMISSION_FORWARDING_AGENT_DIR` → `PI_DELEGATED_AUTH_RUNTIME_DIR` → `PI_MULTI_AUTH_RUNTIME_DIR`
/// → `PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR` → default, with the three middle levels guarded by
/// `options.isSubagent`. They exist for one reason pi states inline (`:83-85`): "Router-launched
/// subagents run with an isolated `PI_CODING_AGENT_DIR`", so a child must be pointed BACK at a
/// directory it shares with its parent or the two would spool into different trees.
///
/// Levels 2 and 3 still have no cyrup analog. **Level 4 now does** —
/// [`crate::extension::POLICY_AGENT_DIR_ENV_KEY`] (`CYRUP_PERMISSION_SYSTEM_POLICY_AGENT_DIR`) landed
/// with PERM-025 and relocates the POLICY root — and `forwarding_root_dir` deliberately does not
/// consult it. That is safe **only because of a precondition that lives in another crate**: no cyrup
/// subagent spawn site writes an isolated `CYRUP_AGENT_DIR` into the child's env overlay, so a child
/// and its parent always compute the same `default_agent_dir` and therefore the same spool root, with
/// or without the policy override. pi's level 4 is a *repair* for isolation cyrup does not have.
///
/// **If that ever changes** — if a subagent is launched with its own agent dir — this function must
/// grow the subagent-guarded level 4 (and whatever cyrup's analog of levels 2/3 becomes) in upstream's
/// order, or every forwarded child ask will be written where no parent watcher is looking and will
/// fail closed at the child's own 10-minute bound with nothing on either side saying why.
pub const FORWARDING_AGENT_DIR_ENV: &str = "CYRUP_PERMISSION_SYSTEM_FORWARDING_AGENT_DIR";

const PERMISSION_FORWARDING_DIRECTORY_NAME: &str = "permission-forwarding";
const SESSION_FORWARDING_ROOT_DIRECTORY_NAME: &str = "sessions";
const SESSION_FORWARDING_REQUESTS_DIRECTORY_NAME: &str = "requests";
const SESSION_FORWARDING_RESPONSES_DIRECTORY_NAME: &str = "responses";

/// pi `ForwardedPermissionRequest` (`permission-forwarding.ts:20-28`) — child → parent. Field names
/// are pi's exact camelCase so the on-disk JSON is byte-faithful (a pi parent could read a cyrup
/// child's request and vice-versa).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardedPermissionRequest {
    pub id: String,
    #[serde(rename = "responseNonce")]
    pub response_nonce: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "requesterSessionId")]
    pub requester_session_id: String,
    #[serde(rename = "targetSessionId")]
    pub target_session_id: String,
    #[serde(rename = "requesterAgentName")]
    pub requester_agent_name: String,
    pub message: String,
}

/// pi `ForwardedPermissionResponse` (`permission-forwarding.ts:30-38`) — parent → child; echoes the
/// request's `responseNonce` back so the child can bind it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardedPermissionResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "responseNonce")]
    pub response_nonce: String,
    pub approved: bool,
    pub state: PermissionDecisionState,
    #[serde(rename = "denialReason", default, skip_serializing_if = "Option::is_none")]
    pub denial_reason: Option<String>,
    #[serde(rename = "responderSessionId")]
    pub responder_session_id: String,
    #[serde(rename = "respondedAt", default)]
    pub responded_at: i64,
}

/// pi `PermissionForwardingLocation` (`permission-forwarding.ts:40-46`) — a session's request/response
/// dirs under the shared spool root.
#[derive(Debug, Clone)]
pub struct ForwardingLocation {
    pub session_root: PathBuf,
    pub requests_dir: PathBuf,
    pub responses_dir: PathBuf,
}

// ---------------------------------------------------------------------------- path resolution

/// pi `resolvePermissionForwardingRootDir` (`permission-forwarding.ts:74-103`), simplified to the two
/// non-subagent-scoped precedence levels cyrup uses (explicit override → default agent dir), then
/// `join("sessions", "permission-forwarding")`.
fn forwarding_root_dir(default_agent_dir: &Path) -> PathBuf {
    let agent_dir = crate::envx::var(FORWARDING_AGENT_DIR_ENV)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map_or_else(|| default_agent_dir.to_path_buf(), PathBuf::from);
    agent_dir
        .join(SESSION_FORWARDING_ROOT_DIRECTORY_NAME)
        .join(PERMISSION_FORWARDING_DIRECTORY_NAME)
}

/// pi `normalizePermissionForwardingSessionId` (`permission-forwarding.ts:48-59`): trim; reject empty
/// / `"unknown"` (case-insensitive) → `None`.
#[must_use]
pub fn normalize_session_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// pi `encodeSessionIdForPath` = `encodeURIComponent` (`permission-forwarding.ts:61-63`): percent-
/// encode every byte outside JavaScript's `encodeURIComponent` unreserved set
/// (`A-Za-z0-9 - _ . ! ~ * ' ( )`), uppercase hex, over the UTF-8 bytes.
#[must_use]
pub fn encode_session_id_for_path(session_id: &str) -> String {
    fn hex_digit(nibble: u8) -> char {
        char::from(if nibble < 10 { b'0' + nibble } else { b'A' + (nibble - 10) })
    }
    let mut out = String::with_capacity(session_id.len());
    for &b in session_id.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if unreserved {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0x0f));
        }
    }
    out
}

/// pi `createPermissionForwardingLocation` (`permission-forwarding.ts:105-127`): resolve the session's
/// `{requests,responses}` dirs, validating the encoded session-id token BEFORE any `Path::join`
/// (R-PERM-040, reusing the P-5 `validate_safe_token` / `validate_contains_root` primitives).
///
/// # Errors
/// Returns [`PermissionError::UnsafeToken`] if the session id is empty/`"unknown"`, or if its encoded
/// form fails the safe-token / contains-root guard.
pub fn forwarding_location(
    default_agent_dir: &Path,
    session_id: &str,
) -> Result<ForwardingLocation, PermissionError> {
    let normalized = normalize_session_id(session_id).ok_or_else(|| {
        PermissionError::UnsafeToken("session id must be a non-empty, non-\"unknown\" string".into())
    })?;
    let encoded = encode_session_id_for_path(&normalized);
    validate_safe_token(&encoded).map_err(|e| PermissionError::UnsafeToken(e.to_string()))?;
    let root = forwarding_root_dir(default_agent_dir);
    let session_root = root.join(SESSION_FORWARDING_ROOT_DIRECTORY_NAME).join(&encoded);
    validate_contains_root(&root, &session_root)
        .map_err(|e| PermissionError::UnsafeToken(e.to_string()))?;
    Ok(ForwardingLocation {
        requests_dir: session_root.join(SESSION_FORWARDING_REQUESTS_DIRECTORY_NAME),
        responses_dir: session_root.join(SESSION_FORWARDING_RESPONSES_DIRECTORY_NAME),
        session_root,
    })
}

// ---------------------------------------------------------------------------- filesystem helpers

/// pi `setRestrictiveFileSystemMode` (v0.8.0 `index.ts:746-752`): chmod, and on failure raise
/// `Failed to restrict {description} permissions for '{path}'` on the forwarding WARNING stream.
///
/// `audit` is `Option` only because pi's logger is module-scope and cyrup's is an owned
/// [`AuditTrail`] handle: the crate's own unit tests call these helpers with no trail in scope, and
/// upstream's shape has no way to express that. Every PRODUCTION path passes `Some`.
#[cfg(unix)]
fn set_restrictive_mode(path: &Path, mode: u32, description: &str, audit: Option<&AuditTrail>) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        && let Some(audit) = audit
    {
        audit.forwarding_warning(
            &format!("Failed to restrict {description} permissions for '{}'", path.display()),
            Some(&err.to_string()),
        );
    }
}

#[cfg(not(unix))]
fn set_restrictive_mode(
    _path: &Path,
    _mode: u32,
    _description: &str,
    _audit: Option<&AuditTrail>,
) {
}

/// pi `ensureDirectoryExists` (v0.8.0 `index.ts:754-763`): mkdir recursive + chmod `0o700`, with
/// `Failed to create {description} directory '{path}'` on the forwarding ERROR stream when the
/// mkdir fails. pi logs the mkdir failure at `error` and the chmod failure at `warning`; both are
/// reproduced.
fn ensure_directory_exists(path: &Path, description: &str, audit: Option<&AuditTrail>) -> bool {
    if let Err(err) = std::fs::create_dir_all(path) {
        if let Some(audit) = audit {
            audit.forwarding_error(
                &format!("Failed to create {description} directory '{}'", path.display()),
                Some(&err.to_string()),
            );
        }
        return false;
    }
    set_restrictive_mode(path, 0o700, description, audit);
    true
}

/// pi `ensurePermissionForwardingLocation` (v0.8.0 `index.ts:793-809`): the three
/// [`ensure_directory_exists`] calls, in upstream's order, with upstream's three literal
/// descriptions. Returns `true` only if all three dirs are ready.
///
/// **Upstream evaluates all three even when an earlier one failed** (`:803-805` are three
/// unconditional `const` bindings ANDed at `:808`), so a run with a broken spool reports all three
/// causes rather than only the first. `Iterator::all` short-circuits, which would have logged one —
/// hence the fold.
#[must_use]
pub fn ensure_location(location: &ForwardingLocation, audit: Option<&AuditTrail>) -> bool {
    [
        (&location.session_root, "permission forwarding session root"),
        (&location.requests_dir, "permission forwarding requests"),
        (&location.responses_dir, "permission forwarding responses"),
    ]
    .into_iter()
    .fold(true, |ready, (dir, description)| {
        ensure_directory_exists(dir, description, audit) && ready
    })
}

/// pi `createPermissionForwardingNonce` (`index.ts:1135-1137`): 32 CSPRNG bytes, base64url (no pad),
/// = 256 bits. Falls back to two v4 UUIDs (244 random bits) only if `getrandom` is unavailable — a
/// no-panic degrade that still binds meaningfully; on every supported platform the CSPRNG path runs.
#[must_use]
pub fn create_nonce() -> String {
    let mut buf = [0u8; 32];
    if getrandom::fill(&mut buf).is_err() {
        let a = *uuid::Uuid::new_v4().as_bytes();
        let b = *uuid::Uuid::new_v4().as_bytes();
        let (first, second) = buf.split_at_mut(16);
        first.copy_from_slice(&a);
        second.copy_from_slice(&b);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// pi `safeEqualString` (`index.ts:1139-1143`): a constant-time (over equal-length) byte comparison
/// after a length pre-check, so a forged nonce of the wrong length is rejected without a timing leak
/// on the matching-length path.
#[must_use]
pub fn safe_equal_string(left: &str, right: &str) -> bool {
    let (l, r) = (left.as_bytes(), right.as_bytes());
    if l.len() != r.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in l.iter().zip(r.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// pi `isForwardedPermissionResponseBoundToRequest` (`index.ts:1145-1164`): a response binds to a
/// request iff `requestId` matches AND the nonce matches (constant-time) AND the responder is the
/// addressed target session.
#[must_use]
pub fn response_is_bound(
    response: &ForwardedPermissionResponse,
    request: &ForwardedPermissionRequest,
    target_session_id: &str,
) -> bool {
    response.request_id == request.id
        && safe_equal_string(&response.response_nonce, &request.response_nonce)
        && response.responder_session_id == target_session_id
}

/// [`response_is_bound`] with pi's TWO rejection warnings
/// (`isForwardedPermissionResponseBoundToRequest`, v0.8.0 `index.ts:879-898`), which name the two
/// failures separately because they mean different things: the first is a stale or forged response,
/// the second is a response written by the wrong session. cyrup dropped both, so a forged response
/// was discarded in silence and the operator saw only the `response_received` review entry with
/// every field null — enough to notice something, not enough to say what.
#[must_use]
fn response_is_bound_logged(
    response: &ForwardedPermissionResponse,
    request: &ForwardedPermissionRequest,
    target_session_id: &str,
    response_path: &Path,
    audit: &AuditTrail,
) -> bool {
    if response.request_id != request.id
        || !safe_equal_string(&response.response_nonce, &request.response_nonce)
    {
        audit.forwarding_warning(
            &format!(
                "Ignoring forwarded permission response '{}' because it is not bound to request '{}'",
                response_path.display(),
                request.id
            ),
            None,
        );
        return false;
    }
    if response.responder_session_id != target_session_id {
        audit.forwarding_warning(
            &format!(
                "Ignoring forwarded permission response '{}' because responder session '{}' does not match target session '{target_session_id}'",
                response_path.display(),
                response.responder_session_id
            ),
            None,
        );
        return false;
    }
    true
}

/// pi `writeJsonFileAtomic` (`index.ts:1166-1178`): write a unique `<path>.<pid>.<uuid>.tmp` with
/// `O_EXCL` (`wx`, no-clobber) + `0o600`, chmod, then `rename` into place, then chmod again. On any
/// failure the temp is removed. The no-clobber flag on the (unique) temp is load-bearing for
/// at-most-once creation.
///
/// # Errors
/// Returns [`PermissionError::Io`] if serialization, the exclusive create/write, or the rename fails.
pub fn write_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
    audit: Option<&AuditTrail>,
) -> Result<(), PermissionError> {
    let body = serde_json::to_string(value).map_err(|e| PermissionError::Io(e.to_string()))?;
    let pid = std::process::id();
    let uid = uuid::Uuid::new_v4();
    let temp_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => format!("{name}.{pid}.{uid}.tmp"),
        None => format!("permission-forwarding.{pid}.{uid}.tmp"),
    };
    let temp = path.with_file_name(temp_name);

    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }

    let write_result = (|| -> std::io::Result<()> {
        use std::io::Write as _;
        let mut file = open.open(&temp)?;
        file.write_all(body.as_bytes())?;
        let _ = file.sync_all();
        drop(file);
        // pi's two chmod descriptions, `index.ts:983` and `:985`.
        set_restrictive_mode(&temp, 0o600, "temporary permission-forwarding file", audit);
        std::fs::rename(&temp, path)?;
        set_restrictive_mode(path, 0o600, "permission-forwarding file", audit);
        Ok(())
    })();

    if let Err(e) = write_result {
        // pi `safeDeleteFile(tempPath, "temporary permission-forwarding")` (`index.ts:987`) — note
        // upstream's description here omits the trailing `file`, which `safeDeleteFile` appends.
        safe_delete_file(&temp, "temporary permission-forwarding", audit);
        return Err(PermissionError::Io(e.to_string()));
    }
    Ok(())
}

/// The read-then-validate ladder shared by [`read_request`] and [`read_response`], reproducing the
/// TWO DISTINCT diagnostics upstream raises for what Rust would otherwise collapse into one
/// `serde_json::from_str` failure:
///
/// * an unreadable file or non-JSON bytes land in pi's `catch` and log
///   `Failed to read forwarded permission {kind} '{path}'` **with the cause**
///   (v0.8.0 `index.ts:942` / `:973`);
/// * well-formed JSON that fails the field-shape check logs
///   `Ignoring invalid forwarded permission {kind} format in '{path}'` **with no cause**
///   (`:928` / `:959`), because upstream has no error object at that point.
///
/// Deserializing to [`serde_json::Value`] first is what keeps the two apart: it is exactly pi's
/// `JSON.parse` boundary, and only the typed step after it is pi's field ladder.
fn read_forwarded_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    kind: &str,
    audit: Option<&AuditTrail>,
) -> Option<T> {
    let parsed = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|text| {
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())
        });
    let value = match parsed {
        Ok(value) => value,
        Err(err) => {
            if let Some(audit) = audit {
                audit.forwarding_warning(
                    &format!(
                        "Failed to read forwarded permission {kind} '{}'",
                        path.display()
                    ),
                    Some(&err),
                );
            }
            return None;
        }
    };
    match serde_json::from_value::<T>(value) {
        Ok(typed) => Some(typed),
        Err(_) => {
            if let Some(audit) = audit {
                // pi passes NO error to this call (`index.ts:928`, `:959`) — the field ladder is a
                // boolean check upstream, so the entry carries `{message}` alone. Attaching serde's
                // message here would emit a shape pi's readers do not produce.
                audit.forwarding_warning(
                    &format!(
                        "Ignoring invalid forwarded permission {kind} format in '{}'",
                        path.display()
                    ),
                    None,
                );
            }
            None
        }
    }
}

/// pi `readForwardedPermissionRequest` (v0.8.0 `index.ts:906-948`) WITHOUT the audit trail — the
/// shape the crate's own tests and out-of-crate probes use. Production reads go through
/// [`read_request_with_audit`].
#[must_use]
pub fn read_request(path: &Path) -> Option<ForwardedPermissionRequest> {
    read_forwarded_json(path, "request", None)
}

/// pi `readForwardedPermissionRequest` (v0.8.0 `index.ts:906-948`), including both of its warning
/// entries.
#[must_use]
pub fn read_request_with_audit(
    path: &Path,
    audit: &AuditTrail,
) -> Option<ForwardedPermissionRequest> {
    read_forwarded_json(path, "request", Some(audit))
}

/// pi `readForwardedPermissionResponse` (v0.8.0 `index.ts:950-977`) without the audit trail.
#[must_use]
pub fn read_response(path: &Path) -> Option<ForwardedPermissionResponse> {
    read_forwarded_json(path, "response", None)
}

/// pi `readForwardedPermissionResponse` (v0.8.0 `index.ts:950-977`), including both of its warning
/// entries.
#[must_use]
pub fn read_response_with_audit(
    path: &Path,
    audit: &AuditTrail,
) -> Option<ForwardedPermissionResponse> {
    read_forwarded_json(path, "response", Some(audit))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// pi `tryRemoveDirectoryIfEmpty` (v0.8.0 `index.ts:823-849`): an absent dir is silent; a dir that
/// cannot be listed warns `Failed to inspect …`; a non-empty dir returns; a removal that fails with
/// anything other than `ENOENT`/`ENOTEMPTY` warns `Failed to remove empty …`.
///
/// The two ignored codes are load-bearing and are why `remove_dir`'s error is inspected rather than
/// discarded: both mean another process won the race, which is normal for a shared spool.
fn try_remove_directory_if_empty(path: &Path, description: &str, audit: Option<&AuditTrail>) {
    if !path.exists() {
        return;
    }
    let is_empty = match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(err) => {
            if let Some(audit) = audit {
                audit.forwarding_warning(
                    &format!("Failed to inspect {description} directory '{}'", path.display()),
                    Some(&err.to_string()),
                );
            }
            return;
        }
    };
    if !is_empty {
        return;
    }
    if let Err(err) = std::fs::remove_dir(path) {
        let ignorable = matches!(
            err.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
        );
        if !ignorable && let Some(audit) = audit {
            audit.forwarding_warning(
                &format!("Failed to remove empty {description} directory '{}'", path.display()),
                Some(&err.to_string()),
            );
        }
    }
}

/// pi `cleanupPermissionForwardingLocationIfEmpty` (v0.8.0 `index.ts:851-855`): best-effort removal
/// of now-empty spool dirs (leaf → root), with upstream's `${location.label} …` descriptions. The
/// label is the literal `"primary"` for every location this crate builds
/// (`permission-forwarding.ts:120`), which is why it is inlined rather than carried on
/// [`ForwardingLocation`] — the same choice the crate's existing `"Failed to read primary permission
/// forwarding requests from …"` message already made.
fn cleanup_location_if_empty(location: &ForwardingLocation, audit: Option<&AuditTrail>) {
    try_remove_directory_if_empty(
        &location.requests_dir,
        "primary permission forwarding requests",
        audit,
    );
    try_remove_directory_if_empty(
        &location.responses_dir,
        "primary permission forwarding responses",
        audit,
    );
    try_remove_directory_if_empty(
        &location.session_root,
        "primary permission forwarding session root",
        audit,
    );
}

/// pi `safeDeleteFile` (v0.8.0 `index.ts:857-867`): unlink, silent on `ENOENT`, warning otherwise.
fn safe_delete_file(path: &Path, description: &str, audit: Option<&AuditTrail>) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
        && let Some(audit) = audit
    {
        audit.forwarding_warning(
            &format!("Failed to delete {description} file '{}'", path.display()),
            Some(&err.to_string()),
        );
    }
}

/// A `notify::PollWatcher` on `dir`, mirroring `cyrup_ext_subagents::watch_control_inbox`
/// (`control.rs:548-587`): poll at [`CONTROL_INBOX_POLL_INTERVAL`], send `()` on any event. The
/// caller keeps the watcher alive for the watch's duration.
///
/// # Errors
/// Returns [`PermissionError::Io`] if the watcher cannot be constructed or attached.
fn watch_dir(
    dir: &Path,
) -> Result<(notify::PollWatcher, tokio::sync::mpsc::UnboundedReceiver<()>), PermissionError> {
    use notify::Watcher as _;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let cfg = notify::Config::default()
        .with_poll_interval(CONTROL_INBOX_POLL_INTERVAL)
        .with_compare_contents(true);
    let mut watcher = notify::PollWatcher::new(
        move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        },
        cfg,
    )
    .map_err(|e| PermissionError::Io(e.to_string()))?;
    watcher
        .watch(dir, notify::RecursiveMode::NonRecursive)
        .map_err(|e| PermissionError::Io(e.to_string()))?;
    Ok((watcher, rx))
}

fn denied() -> PermissionPromptDecision {
    PermissionPromptDecision { approved: false, state: PermissionDecisionState::Denied, denial_reason: None }
}

/// The child wait bound: the [`CHILD_WAIT_TIMEOUT_ENV`] override if set to a finite positive ms value,
/// else the pi-faithful [`PERMISSION_FORWARDING_TIMEOUT`] (10 min).
#[must_use]
pub fn resolve_child_wait_timeout() -> Duration {
    crate::envx::var(CHILD_WAIT_TIMEOUT_ENV)
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(PERMISSION_FORWARDING_TIMEOUT, Duration::from_millis)
}

// ---------------------------------------------------------------------------- child (requester)

/// pi `waitForForwardedPermissionApproval` (`index.ts:1255-1355`): write a nonce-bound REQUEST into
/// the parent's spool, then BLOCK polling for the bound RESPONSE up to `timeout`. Every failure path
/// (null target, spool-prepare failure, request-write failure, timeout) resolves to a DENY — the
/// child gate fail-CLOSES, never hangs, never allows.
pub async fn wait_for_forwarded_approval(
    default_agent_dir: &Path,
    target_session_id: &str,
    requester_session_id: &str,
    requester_agent_name: &str,
    message: &str,
    timeout: Duration,
    audit: &AuditTrail,
) -> PermissionPromptDecision {
    let Some(target) = normalize_session_id(target_session_id) else {
        // pi null-target deny (v0.8.0 `index.ts:1000-1005`), WITH its error entry (`:1001-1003`).
        audit.forwarding_error(
            "Permission forwarding target session could not be resolved from subagent runtime metadata (expected CYRUP_SUBAGENT_PARENT_SESSION)",
            None,
        );
        return denied();
    };
    let location = match forwarding_location(default_agent_dir, &target) {
        Ok(l) => l,
        Err(err) => {
            audit.forwarding_error(
                &format!("Permission forwarding is unavailable because session-scoped directories could not be prepared for '{target}'"),
                Some(&err.to_string()),
            );
            return denied();
        }
    };
    if !ensure_location(&location, Some(audit)) {
        // pi unavailable-dirs deny + error entry (v0.8.0 `index.ts:1007-1013`).
        audit.forwarding_error(
            &format!("Permission forwarding is unavailable because session-scoped directories could not be prepared for '{target}'"),
            None,
        );
        return denied();
    }

    let request_id = uuid::Uuid::new_v4().to_string();
    let request = ForwardedPermissionRequest {
        id: request_id.clone(),
        response_nonce: create_nonce(),
        created_at: now_millis(),
        requester_session_id: requester_session_id.to_string(),
        target_session_id: target.clone(),
        requester_agent_name: requester_agent_name.to_string(),
        message: message.to_string(),
    };
    let request_path = location.requests_dir.join(format!("{request_id}.json"));
    let response_path = location.responses_dir.join(format!("{request_id}.json"));

    // pi `writeReviewEntry("forwarded_permission.request_created", …)`
    // (v0.8.0 `index.ts:1030-1037`) — written BEFORE the spool write, so a request that fails to
    // land still leaves evidence that it was attempted.
    audit.review(
        "forwarded_permission.request_created",
        &serde_json::json!({
            "requestId": request_id,
            "requesterAgentName": request.requester_agent_name,
            "requesterSessionId": request.requester_session_id,
            "targetSessionId": target,
            "requestPath": request_path.display().to_string(),
            "responsePath": response_path.display().to_string(),
        }),
    );

    if let Err(err) = write_json_atomic(&request_path, &request, Some(audit)) {
        // pi request-write failure deny + error entry (v0.8.0 `index.ts:1039-1044`).
        audit.forwarding_error(
            &format!("Failed to write forwarded permission request '{}'", request_path.display()),
            Some(&err.to_string()),
        );
        return denied();
    }

    // pi poll-to-deadline (`index.ts:1314-1343`). Reuse ONE `notify::PollWatcher` on the responses dir
    // for low-latency wakeups (fed by the parent's response write), with the poll interval as the
    // fallback tick — the pi `fs.watch` + `min(POLL, remaining)` structure.
    let mut watcher = match watch_dir(&location.responses_dir) {
        Ok(w) => Some(w),
        Err(err) => {
            // pi `writeDebugEntry("permission_forwarding.watch_setup_error", { responseDir, error })`
            // (v0.8.0 `index.ts:658-664`), inside `waitForForwardedPermissionResponseFile`: a
            // directory watch is best-effort across filesystems, and the poll tick below is the
            // documented safe fallback, so this is diagnostic rather than a warning.
            //
            // \[CYRUP-DELTA] pi's sibling `permission_forwarding.watcher_close_error` (`:641-644`)
            // has no reachable analog: it fires when node's `watcher.close()` throws, and Rust's
            // `notify::PollWatcher` is torn down by `Drop`, which cannot fail or report. Nothing is
            // dropped from the trail that a cyrup run could ever have produced.
            audit.debug(
                "permission_forwarding.watch_setup_error",
                &serde_json::json!({
                    "responseDir": location.responses_dir.display().to_string(),
                    "error": err.to_string(),
                }),
            );
            None
        }
    };
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if response_path.exists() {
            let bound = read_response_with_audit(&response_path, audit).filter(|resp| {
                response_is_bound_logged(resp, &request, &target, &response_path, audit)
            });
            // pi `writeReviewEntry("forwarded_permission.response_received", …)`
            // (v0.8.0 `index.ts:1056-1064`) — written for EVERY response file observed, bound or
            // not, with every field `null` when the binding check rejected it. That is the whole
            // point: a forged or stale response is exactly what an operator needs to see.
            audit.review(
                "forwarded_permission.response_received",
                &serde_json::json!({
                    "requestId": request_id,
                    "approved": bound.as_ref().map_or(serde_json::Value::Null, |r| serde_json::Value::Bool(r.approved)),
                    "state": bound.as_ref().map_or(serde_json::Value::Null, |r| serde_json::json!(r.state)),
                    "denialReasonMetadata": sensitive_log_metadata(
                        bound.as_ref().and_then(|r| r.denial_reason.as_deref()),
                    ),
                    "responderSessionId": bound.as_ref().map_or(serde_json::Value::Null, |r| serde_json::Value::String(r.responder_session_id.clone())),
                    "targetSessionId": target,
                    "responsePath": response_path.display().to_string(),
                }),
            );
            let _ = std::fs::remove_file(&response_path); // consume-on-read (pi `:1066`).
            if let Some(resp) = bound {
                let _ = std::fs::remove_file(&request_path);
                cleanup_location_if_empty(&location, Some(audit));
                return PermissionPromptDecision {
                    approved: resp.approved,
                    state: resp.state,
                    denial_reason: resp.denial_reason,
                };
            }
            // Unbound (forged / stale) response: ignore and keep waiting (pi `:1334-1336`).
            continue;
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait = remaining.min(CONTROL_INBOX_POLL_INTERVAL);
        match watcher.as_mut() {
            Some((_w, rx)) => {
                tokio::select! {
                    _ = rx.recv() => {}
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            None => tokio::time::sleep(wait).await,
        }
    }

    // pi timeout deny (v0.8.0 `index.ts:1077-1086`): a `permission_forwarding.warning` FIRST
    // (`:1077`), then the `forwarded_permission.response_timed_out` review entry (`:1078-1083`).
    audit.forwarding_warning(
        &format!(
            "Timed out waiting for forwarded permission response '{}'",
            response_path.display()
        ),
        None,
    );
    audit.review(
        "forwarded_permission.response_timed_out",
        &serde_json::json!({
            "requestId": request_id,
            "requesterAgentName": request.requester_agent_name,
            "targetSessionId": target,
            "responsePath": response_path.display().to_string(),
        }),
    );
    let _ = std::fs::remove_file(&request_path);
    cleanup_location_if_empty(&location, Some(audit));
    denied()
}

// ---------------------------------------------------------------------------- parent (responder)

/// pi `formatForwardedPermissionPrompt` (`index.ts:1244-1253`).
fn format_forwarded_prompt(request: &ForwardedPermissionRequest) -> String {
    let agent = if request.requester_agent_name.is_empty() {
        "unknown"
    } else {
        request.requester_agent_name.as_str()
    };
    let session = if request.requester_session_id.is_empty() {
        "unknown"
    } else {
        request.requester_session_id.as_str()
    };
    format!("Subagent '{agent}' requested permission.\nSession ID: {session}\n\n{}", request.message)
}

/// pi's `options` bag for [`process_forwarded_requests`] (`index.ts:1358`
/// `options: { preserveLocation?: boolean } = {}`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessForwardedOptions {
    /// Skip the trailing [`cleanup_location_if_empty`] (pi `if (!options.preserveLocation) { … }`,
    /// `index.ts:1501-1503`).
    ///
    /// The DEFAULT (`false`, pi's `= {}`) tears the spool down once it drains, which is right for a
    /// one-shot scan whose caller is done with the location. It is WRONG for the long-lived parent
    /// watcher, which owns the location for the whole session: pi's only production caller is
    /// `runForwardedPermissionRequestScan` at `index.ts:1935`, and it passes
    /// `{ preserveLocation: true }` precisely because it re-scans on every wake and must not delete
    /// the inbox a child may be writing into between two scans.
    pub preserve_location: bool,
}

impl ProcessForwardedOptions {
    /// pi's `{ preserveLocation: true }` — the watcher's option bag.
    #[must_use]
    pub const fn preserve_location() -> Self {
        Self { preserve_location: true }
    }
}

/// pi `processForwardedPermissionRequests` (`index.ts:1357-1504`): scan the parent's OWN inbox, and
/// for each valid request targeting this session, resolve a decision (expired → deny; yolo → approve;
/// else surface the live dialog UNDER the shared C3 human-interaction lock) and write the nonce-bound
/// RESPONSE, then delete the request. `services` is the captured live backend (P-1); `session_id` is
/// the parent's own id (pi `getSessionId(ctx)`).
///
/// `options.preserve_location` ports pi's `preserveLocation` (`index.ts:1358`, `:1501-1503`) — see
/// [`ProcessForwardedOptions`] for why the watcher must set it.
pub async fn process_forwarded_requests(
    default_agent_dir: &Path,
    session_id: &str,
    services: &Arc<dyn HostServices>,
    config: &ExtensionConfig,
    options: ProcessForwardedOptions,
    audit: &AuditTrail,
    has_ui: bool,
) {
    // PERM-031 / pi `if (!ctx.hasUI) { return; }` (v0.8.0 `index.ts:1113-1116`) — re-checked on
    // EVERY scan, not only at watcher start. Upstream's `permissionForwardingContext` is a live
    // `ExtensionContext` reference, so a UI that detaches mid-session flips `ctx.hasUI` under the
    // running poller and the spool simply stops being serviced; each request then stays on disk
    // until the UI returns or the CHILD's own 10-minute bound expires.
    //
    // Without this, cyrup's watcher kept scanning between hooks and every pending child ask fell
    // through to `AskOutcome::NoLiveChannel => denied()`, which writes a nonce-bound DENY the child
    // consumes as the operator's FINAL answer. pi defers; cyrup answered "denied" on behalf of an
    // absent human.
    if !has_ui {
        return;
    }
    let Some(current) = normalize_session_id(session_id) else {
        return;
    };
    let location = match forwarding_location(default_agent_dir, &current) {
        Ok(l) => l,
        Err(_) => return,
    };
    // pi `getExistingPermissionForwardingLocation` (`index.ts:1075-1087`): no requests dir ⇒ nothing.
    if !location.requests_dir.exists() {
        return;
    }

    let mut request_files: Vec<PathBuf> = match std::fs::read_dir(&location.requests_dir) {
        Ok(read_dir) => read_dir
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
            .collect(),
        Err(err) => {
            // pi `logPermissionForwardingWarning("Failed to read … requests from '…'")`
            // (v0.8.0 `index.ts:1131`).
            audit.forwarding_warning(
                &format!(
                    "Failed to read primary permission forwarding requests from '{}'",
                    location.requests_dir.display()
                ),
                Some(&err.to_string()),
            );
            return;
        }
    };
    request_files.sort(); // pi `.sort()` (`index.ts:1375`) — deterministic order.

    for request_path in request_files {
        let request = match read_request_with_audit(&request_path, audit) {
            Some(r) => r,
            None => {
                // pi `safeDeleteFile(requestPath, "${location.label} forwarded permission request")`
                // (v0.8.0 `index.ts:1147`). The READER has already said WHY it was unusable; this
                // call only reports a delete that itself failed.
                safe_delete_file(
                    &request_path,
                    "primary forwarded permission request",
                    Some(audit),
                );
                continue;
            }
        };

        // pi `isForwardedPermissionRequestForSession` (`:1397-1403`): drop a request for another
        // session (a hostile/stale spool entry cannot steer THIS parent's human).
        if normalize_session_id(&request.target_session_id).as_deref() != Some(current.as_str()) {
            // pi `logPermissionForwardingWarning` (v0.8.0 `index.ts:1149-1151`).
            audit.forwarding_warning(
                &format!(
                    "Ignoring forwarded permission request '{}' because it targets session '{}' instead of '{current}'",
                    request.id, request.target_session_id
                ),
                None,
            );
            let _ = std::fs::remove_file(&request_path);
            continue;
        }

        // `request.id` is attacker-controlled: it is read verbatim out of a spool file any child
        // that can forward a request may write, and it is joined into `responses_dir` below. An id
        // of `../../../.bashrc` would make THIS process — the parent, which is the trusted one —
        // write a JSON document outside the spool. `validate_safe_token` rejects `/`, `\` and
        // `..`, exactly as it already does for the session-id token at `forwarding_location`
        // (`:187`), and it is what actually stops traversal here.
        //
        // `validate_contains_root` is R-SA-087's second clause, but note precisely what it can and
        // cannot do: it is `resolved.starts_with(root)`, a LEXICAL comparison that does NOT resolve
        // `..` components — `responses/../../x` "starts with" `responses` as far as it is
        // concerned. It therefore adds nothing against traversal and must never be mistaken for
        // the defence. What it does catch is the ABSOLUTE-path shape: `join("/etc/cron.d/x")`
        // discards the root entirely, and the result fails `starts_with`. Both checks are kept
        // because they cover different escapes; neither alone is sufficient.
        //
        // This runs BEFORE `resolve_forwarded_decision` on purpose. That call surfaces the human
        // ask dialog, so validating afterwards would still let a hostile request interrupt the
        // user with a plausible-looking prompt — and the response is written on *either* answer,
        // so denying it would not prevent the write. A request that cannot be answered safely must
        // never be shown at all.
        if validate_safe_token(&request.id).is_err() {
            // pi's escaping-response-path warning (v0.8.0 `index.ts:1158`). Upstream folds both
            // guards into `resolvePathWithinDirectory` returning `null`; cyrup keeps them as two
            // checks (see above) and reports the same message from each.
            audit.forwarding_warning(
                &format!(
                    "Ignoring forwarded permission request '{}' because its response path would escape '{}'",
                    request.id,
                    location.responses_dir.display()
                ),
                None,
            );
            let _ = std::fs::remove_file(&request_path);
            continue;
        }
        let response_path = location.responses_dir.join(format!("{}.json", request.id));
        if validate_contains_root(&location.responses_dir, &response_path).is_err() {
            audit.forwarding_warning(
                &format!(
                    "Ignoring forwarded permission request '{}' because its response path would escape '{}'",
                    request.id,
                    location.responses_dir.display()
                ),
                None,
            );
            let _ = std::fs::remove_file(&request_path);
            continue;
        }

        // pi `forwardedPermissionLogDetails` (v0.8.0 `index.ts:1164-1167`) =
        // `createForwardedPermissionLogDetails(request, location)` (`:1091-1108`) plus
        // `requestPath`. `source` is `location.label`, which is the constant `"primary"` in the
        // single-location model both sides use.
        let log_details = serde_json::json!({
            "requestId": request.id,
            "source": "primary",
            "requesterAgentName": request.requester_agent_name,
            "requesterSessionId": request.requester_session_id,
            "targetSessionId": request.target_session_id,
            "requestPath": request_path.display().to_string(),
        });

        let decision =
            resolve_forwarded_decision(&request, services, config, audit, &log_details).await;

        // pi `writeReviewEntry(decision.approved ? ".approved" : ".denied", …)`
        // (v0.8.0 `index.ts:1225-1230`). Note the details here are
        // `createForwardedPermissionLogDetails(...)` WITHOUT `requestPath` and WITH `responsePath`
        // — a different shape from `forwardedPermissionLogDetails` above, deliberately.
        audit.review(
            if decision.approved {
                "forwarded_permission.approved"
            } else {
                "forwarded_permission.denied"
            },
            &serde_json::json!({
                "requestId": request.id,
                "source": "primary",
                "requesterAgentName": request.requester_agent_name,
                "requesterSessionId": request.requester_session_id,
                "targetSessionId": request.target_session_id,
                "responsePath": response_path.display().to_string(),
                "resolution": decision.state,
                "denialReasonMetadata": sensitive_log_metadata(decision.denial_reason.as_deref()),
            }),
        );

        let response = ForwardedPermissionResponse {
            request_id: request.id.clone(),
            response_nonce: request.response_nonce.clone(), // echo the child's nonce (pi `:1486`).
            approved: decision.approved,
            state: decision.state,
            denial_reason: decision.denial_reason.clone(),
            responder_session_id: current.clone(),
            responded_at: now_millis(),
        };
        if let Err(err) = write_json_atomic(&response_path, &response, Some(audit)) {
            // pi response-write failure: report it and leave the request for a retry
            // (v0.8.0 `index.ts:1240-1243`).
            audit.forwarding_error(
                &format!(
                    "Failed to write primary forwarded permission response '{}'",
                    response_path.display()
                ),
                Some(&err.to_string()),
            );
            continue;
        }
        let _ = std::fs::remove_file(&request_path); // pi `:1498`.
    }

    // pi `if (!options.preserveLocation) { cleanupPermissionForwardingLocationIfEmpty(location); }`
    // (`index.ts:1501-1503`).
    if !options.preserve_location {
        cleanup_location_if_empty(&location, Some(audit));
    }
}

/// The per-request decision (v0.8.0 pi `index.ts:1170-1230`): expired → deny; yolo → approve; else surface
/// the SAME `select`/`input` dialog a local ask uses ([`LocalAskChannel`]) UNDER the one host-owned,
/// session-scoped human-interaction lock (C3), with the configured auto-deny prompt timeout.
async fn resolve_forwarded_decision(
    request: &ForwardedPermissionRequest,
    services: &Arc<dyn HostServices>,
    config: &ExtensionConfig,
    audit: &AuditTrail,
    log_details: &serde_json::Value,
) -> PermissionPromptDecision {
    /// Spread `log_details` (pi's `...forwardedPermissionLogDetails`) then the extra keys, matching
    /// JS object-literal ordering where the later keys win.
    fn with_details(base: &serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        let mut record = base.clone();
        if let (serde_json::Value::Object(target), serde_json::Value::Object(source)) =
            (&mut record, &extra)
        {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        record
    }

    let timeout_ms = i64::try_from(PERMISSION_FORWARDING_TIMEOUT.as_millis()).unwrap_or(i64::MAX);
    let age_ms = now_millis().saturating_sub(request.created_at);
    if age_ms >= timeout_ms {
        // pi `writeReviewEntry("forwarded_permission.expired", {...details, requestAgeMs,
        // timeoutMs})` (v0.8.0 `index.ts:1173-1177`).
        audit.review(
            "forwarded_permission.expired",
            &with_details(
                log_details,
                serde_json::json!({ "requestAgeMs": age_ms, "timeoutMs": timeout_ms }),
            ),
        );
        // pi expired-on-read (v0.8.0 `index.ts:1178-1182`).
        return PermissionPromptDecision {
            approved: false,
            state: PermissionDecisionState::Denied,
            denial_reason: Some(
                "permission_timeout: forwarded permission request expired before it could be displayed."
                    .to_string(),
            ),
        };
    }

    // pi `shouldAutoApprovePermissionState("ask", extensionConfig)` (v0.8.0 `index.ts:1183-1185`):
    // yolo auto-approves.
    if config.yolo_mode {
        // pi `writeReviewEntry("forwarded_permission.auto_approved", forwardedPermissionLogDetails)`
        // (v0.8.0 `index.ts:1184`).
        audit.review("forwarded_permission.auto_approved", log_details);
        return PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Approved,
            denial_reason: None,
        };
    }

    // pi `writeReviewEntry("forwarded_permission.prompted", forwardedPermissionLogDetails)`
    // (v0.8.0 `index.ts:1187`) — written BEFORE the debug notify and before the dialog opens, so a
    // parent killed mid-prompt still leaves evidence the request reached a human.
    audit.review("forwarded_permission.prompted", log_details);

    // pi debug notify (v0.8.0 `index.ts:1188-1197`).
    if config.debug {
        let who = if request.requester_agent_name.is_empty() {
            "unknown"
        } else {
            request.requester_agent_name.as_str()
        };
        services.notify(
            &format!("Subagent '{who}' is waiting for permission approval."),
            NotifyKind::Warning,
        );
    }

    // pi optional auto-deny timeout (v0.8.0 `index.ts:1199-1208`): `forwardedPromptTimeoutSeconds`
    // non-null and > 0 → the select auto-rejects after that long; else it waits indefinitely.
    let positive_timeout_secs = config.forwarded_prompt_timeout_seconds.filter(|s| *s > 0.0);
    // pi `timeoutMs = forwardedPromptTimeoutSeconds * 1000` (`index.ts:1201`) — a plain multiply, so a
    // fractional `45.5` yields 45500 ms, NOT 45000. `try_from_secs_f64` rather than
    // `Duration::from_secs_f64` because the latter panics on a non-finite/out-of-range input and this
    // crate denies `clippy::panic`; `normalize` has already excluded NaN/infinity/non-positive, so the
    // only reachable error is an absurd overflow, which saturates to the longest wait expressible.
    let timeout = positive_timeout_secs
        .map(|secs| Duration::try_from_secs_f64(secs).unwrap_or(Duration::MAX));
    // pi `timeoutDenialReason` (v0.8.0 `index.ts:1203-1205`): only set when a positive timeout is
    // configured;
    // `requestPermissionDecisionFromUi` only reaches the reason-attaching fallback branch when the
    // caller passed a `timeoutDenialReason` at all (`permission-dialog.ts:155-158`), so `None` here
    // reproduces pi's plain (no `denialReason`) result for the "wait indefinitely" case.
    // `{secs}` is `Display for f64`, which omits a zero fraction — `30` and `45.5`, exactly what JS
    // interpolates for the same two values (`${30}` / `${45.5}`).
    let timeout_denial_reason = positive_timeout_secs.map(|secs| {
        format!(
            "permission_timeout: forwarded permission prompt was not answered within {secs} seconds."
        )
    });
    let prompt_message = match positive_timeout_secs {
        Some(secs) => format!("This forwarded prompt auto-denies after {secs} seconds if unanswered."),
        None => "This forwarded prompt will wait indefinitely until answered.".to_string(),
    };
    let body = format!("{}\n\n{}", format_forwarded_prompt(request), prompt_message);

    // C3 (reconciliation §1): hold the ONE host-owned human-interaction lock across the dialog so a
    // forwarded prompt and an in-session ask / intercom clarify never surface to the same human at
    // once. Absent (default host) ⇒ nothing to serialize.
    let _human_guard = match services.human_interaction_lock() {
        Some(lock) => Some(lock.acquire().await),
        None => None,
    };
    let channel = LocalAskChannel::new(services.clone());
    match channel
        .confirm(
            "Permission Required (Subagent)",
            &body,
            PromptOpts { timeout, timeout_denial_reason },
        )
        .await
    {
        AskOutcome::Decided(decision) => decision,
        // No live dialog reachable ⇒ fail-CLOSED deny (a headless parent cannot surface it).
        AskOutcome::NoLiveChannel => denied(),
    }
}

// ---------------------------------------------------------------------------- parent watcher task

/// A LIVE handle on the extension's `config.json` snapshot, shared between the extension and the
/// spawned forwarding watcher.
///
/// PERM-005: the watcher used to capture an `ExtensionConfig` BY VALUE at spawn time, so a
/// mid-session `yoloMode` / `forwardedPromptTimeoutSeconds` change (pi `refreshExtensionConfig`,
/// `index.ts:1600-1608`, re-run from `refreshSessionRuntimeState` and `before_agent_start`) never
/// reached the running task. Upstream has no such staleness: its polling closure reads the module
/// scope's `extensionConfig` binding on every scan (`index.ts:1427`, `:1443-1452`), which
/// `refreshExtensionConfig` reassigns in place. Sharing the mutex reproduces that — the watcher
/// snapshots it once per poll iteration (`snapshot_config`) instead of once per spawn.
pub type SharedExtensionConfig = Arc<Mutex<ExtensionConfig>>;

/// PERM-031 — a LIVE handle on `ctx.has_ui`, shared between the extension's event arms and the
/// spawned watcher, threaded exactly the way PERM-005 threaded [`SharedExtensionConfig`].
///
/// This is the cyrup form of pi's `permissionForwardingContext` (`index.ts:1666`): upstream keeps a
/// reference to the live `ExtensionContext` and `processForwardedPermissionRequests` re-reads
/// `ctx.hasUI` off it on every scan (`:1114`), so a UI that detaches mid-session stops the spool
/// being serviced WITHOUT any hook having to fire. cyrup's `HostCtx` is passed by reference per
/// dispatch and cannot be held, so the one field the watcher needs is mirrored into an atomic that
/// every ctx-bearing event arm refreshes.
pub type SharedHasUi = Arc<AtomicBool>;

/// Read the current [`ExtensionConfig`] out of a [`SharedExtensionConfig`], never holding the lock
/// across an `await` (the clone is taken and the guard dropped inside this call).
fn snapshot_config(config: &SharedExtensionConfig) -> ExtensionConfig {
    config.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The parent-side forwarding watcher (pi `startForwardedPermissionPolling`, `index.ts:1983-2031`): a
/// detached `tokio` task the parent session installs on `SessionStart` (running OUTSIDE any live
/// `HostCtx`, reachable via the captured `HostServices` Arc). It watches its OWN request inbox with a
/// `notify::PollWatcher` (mirroring `control.rs::watch_control_inbox`) PLUS a
/// [`CONTROL_INBOX_POLL_INTERVAL`] fallback tick, and runs [`process_forwarded_requests`] on every
/// wake (with a mandatory startup scan). Returns the [`tokio::task::JoinHandle`] so the extension can
/// `abort()` it on `SessionShutdown` (pi teardown, `index.ts:2131`).
///
/// PERM-005 — the ATTACH phase RETRIES instead of terminating. Upstream is re-entered on four hooks
/// (`refreshSessionRuntimeState`/`session_start` `:2084`, `before_agent_start` `:2137`, `input`
/// `:2194`, `tool_call` `:2210`), so its `if (!location) return;` (`index.ts:1991-1993`) is a
/// *retry on the next hook*, not a permanent give-up. Cyrup spawns ONE long-lived task instead of a
/// per-hook timer, so the equivalent of that re-entry is an in-task retry loop: an unresolvable
/// session id or an underivable forwarding location parks the watcher on the
/// [`CONTROL_INBOX_POLL_INTERVAL`] tick and it tries again, rather than returning and leaving the
/// spool unserviced for the whole session (the `is_finished()` idempotence check in
/// `PermissionSystemExtension::maybe_start_forwarding_watcher` would respawn it, but only if some
/// later hook called in — which, pre-fix, none did).
#[must_use]
pub fn spawn_forwarding_watcher(
    agent_dir: PathBuf,
    services: Arc<dyn HostServices>,
    config: SharedExtensionConfig,
    audit: Arc<AuditTrail>,
    has_ui: SharedHasUi,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CONTROL_INBOX_POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // ATTACH, with retry (pi's per-hook re-entry). The session id is late-bound — the host
        // publishes it after the extension's `SessionStart` hook returns in some wirings — and the
        // location can fail to derive transiently; neither is terminal.
        let (session_id, location) = loop {
            ticker.tick().await;
            let Some(sid) = services.session_id().and_then(|s| normalize_session_id(&s)) else {
                continue; // no live session id YET ⇒ nothing to address; try again next tick.
            };
            match forwarding_location(&agent_dir, &sid) {
                Ok(loc) => break (sid, loc),
                // pi `if (!location) return;` — a per-hook give-up, re-entered on the next hook.
                Err(_) => continue,
            }
        };

        // Create the inbox so the watch target exists for the run's lifetime (pi ensures on demand).
        let _ = ensure_location(&location, Some(&*audit));

        // Mandatory startup re-scan (a request may have landed before the watcher attached).
        // `preserve_location` — pi `index.ts:1935`'s `{ preserveLocation: true }`: this watcher owns
        // the spool for the whole session (it `ensure_location`d it just above and its
        // `notify::PollWatcher` below is attached to `requests_dir`), so a scan that finds the inbox
        // empty must NOT delete it out from under a child that is mid-write.
        process_forwarded_requests(
            &agent_dir,
            &session_id,
            &services,
            &snapshot_config(&config),
            ProcessForwardedOptions::preserve_location(),
            &audit,
            has_ui.load(Ordering::Relaxed),
        )
        .await;

        let mut watcher = match watch_dir(&location.requests_dir) {
            Ok(w) => Some(w),
            Err(err) => {
                // pi `logPermissionForwardingWarning("Unable to watch permission forwarding
                // requests at '…'; using reduced-frequency polling fallback", error)`
                // (v0.8.0 `index.ts:1763-1768`) — the PARENT-side watch-setup failure. It is a
                // `warning` review entry, not a debug entry: the debug pair at `:641`/`:660` is
                // the CHILD's response-dir watcher, and is written below in
                // `wait_for_forwarded_approval`.
                audit.forwarding_warning(
                    &format!(
                        "Unable to watch permission forwarding requests at '{}'; using reduced-frequency polling fallback",
                        location.requests_dir.display()
                    ),
                    Some(&err.to_string()),
                );
                None
            }
        };
        loop {
            match watcher.as_mut() {
                Some((_w, rx)) => {
                    tokio::select! {
                        _ = rx.recv() => {}
                        _ = ticker.tick() => {}
                    }
                }
                None => {
                    ticker.tick().await;
                }
            }
            // Re-read the LIVE config every iteration (pi reads the reassigned module binding on
            // every scan), so a mid-session yolo / prompt-timeout change takes effect here.
            process_forwarded_requests(
                &agent_dir,
                &session_id,
                &services,
                &snapshot_config(&config),
                ProcessForwardedOptions::preserve_location(),
                &audit,
                // PERM-031: re-read on EVERY scan, not captured at spawn — pi's `ctx.hasUI`.
                has_ui.load(Ordering::Relaxed),
            )
            .await;
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    #[test]
    fn encode_session_id_matches_encode_uri_component() {
        // Unreserved chars pass through; reserved/others percent-encode (uppercase, UTF-8 bytes).
        assert_eq!(encode_session_id_for_path("abc-DEF_123.~"), "abc-DEF_123.~");
        assert_eq!(encode_session_id_for_path("a/b c"), "a%2Fb%20c");
        assert_eq!(encode_session_id_for_path("a:b@c"), "a%3Ab%40c");
    }

    #[test]
    fn normalize_rejects_empty_and_unknown() {
        assert_eq!(normalize_session_id("  x "), Some("x".to_string()));
        assert!(normalize_session_id("").is_none());
        assert!(normalize_session_id("  ").is_none());
        assert!(normalize_session_id("Unknown").is_none());
    }

    #[test]
    fn location_rejects_traversal_session_id_before_join() {
        // A `..` session id encodes to `..` (dot is unreserved), which `validate_safe_token` rejects —
        // it never becomes a `Path::join` component (R-PERM-040).
        let dir = tempfile::tempdir().unwrap();
        let err = forwarding_location(dir.path(), "..");
        assert!(matches!(err, Err(PermissionError::UnsafeToken(_))));
    }

    #[test]
    fn location_layout_is_pi_faithful() {
        let dir = tempfile::tempdir().unwrap();
        let loc = forwarding_location(dir.path(), "sess-1").unwrap();
        assert!(loc.session_root.ends_with("sessions/permission-forwarding/sessions/sess-1"));
        assert!(loc.requests_dir.ends_with("requests"));
        assert!(loc.responses_dir.ends_with("responses"));
    }

    #[test]
    fn nonce_is_256_bits_and_binding_is_constant_time_exact() {
        let n = create_nonce();
        // 32 bytes → 43 base64url chars (no pad).
        assert_eq!(n.len(), 43);
        assert!(safe_equal_string(&n, &n));
        assert!(!safe_equal_string(&n, "short"));
        assert!(!safe_equal_string("abc", "abd"));
    }

    #[test]
    fn atomic_write_is_wx_0600_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.json");
        let req = ForwardedPermissionRequest {
            id: "id1".into(),
            response_nonce: "nonce".into(),
            created_at: 123,
            requester_session_id: "child".into(),
            target_session_id: "parent".into(),
            requester_agent_name: "worker".into(),
            message: "run bash?".into(),
        };
        write_json_atomic(&path, &req, None).unwrap();
        let back = read_request(&path).unwrap();
        assert_eq!(back.id, "id1");
        assert_eq!(back.target_session_id, "parent");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "request file must be 0o600");
        }
    }

    #[test]
    fn forged_nonce_response_does_not_bind() {
        let req = ForwardedPermissionRequest {
            id: "id1".into(),
            response_nonce: "real-nonce".into(),
            created_at: 0,
            requester_session_id: "child".into(),
            target_session_id: "parent".into(),
            requester_agent_name: "w".into(),
            message: "m".into(),
        };
        let forged = ForwardedPermissionResponse {
            request_id: "id1".into(),
            response_nonce: "WRONG".into(), // hostile sibling guessing
            approved: true,
            state: PermissionDecisionState::Once,
            denial_reason: None,
            responder_session_id: "parent".into(),
            responded_at: 0,
        };
        assert!(!response_is_bound(&forged, &req, "parent"), "a forged nonce must NOT bind");
        let mut good = forged.clone();
        good.response_nonce = "real-nonce".into();
        assert!(response_is_bound(&good, &req, "parent"));
        // Wrong responder session also fails to bind.
        assert!(!response_is_bound(&good, &req, "someone-else"));
    }

    #[test]
    fn decision_state_wire_strings_match_pi() {
        // The on-disk `state` must serialize to pi's EXACT strings.
        let cases = [
            (PermissionDecisionState::Approved, "approved"),
            (PermissionDecisionState::Denied, "denied"),
            (PermissionDecisionState::DeniedWithReason, "denied_with_reason"),
            (PermissionDecisionState::Once, "once"),
            (PermissionDecisionState::Always, "always"),
            (PermissionDecisionState::Reject, "reject"),
        ];
        for (state, wire) in cases {
            let js = serde_json::to_string(&state).unwrap();
            assert_eq!(js, format!("\"{wire}\""));
        }
    }

    // =============================================================================================
    // PERM-005 — the watcher's ATTACH phase retries, and it reads the LIVE config every iteration.
    // =============================================================================================

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A `HostServices` whose `session_id()` is `None` for the first `blank_for` calls and
    /// `Some(id)` afterwards — the "session id not resolved yet at SessionStart" case. `select()`
    /// counts calls so a test can prove no human was consulted.
    struct LateSessionHost {
        id: String,
        blank_for: usize,
        calls: AtomicUsize,
        selects: Arc<AtomicUsize>,
    }

    impl HostServices for LateSessionHost {
        fn session_id(&self) -> Option<String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.blank_for {
                None
            } else {
                Some(self.id.clone())
            }
        }
        fn select(
            &self,
            _prompt: &str,
            _options: &serde_json::Value,
            _opts: &cyrup_ext::DialogOptions,
        ) -> Option<String> {
            self.selects.fetch_add(1, Ordering::SeqCst);
            Some("Reject".to_string())
        }
    }

    /// Drop a well-formed forwarded request into `parent`'s inbox and return its id.
    fn seed_request(agent_dir: &Path, parent: &str) -> String {
        let loc = forwarding_location(agent_dir, parent).unwrap();
        assert!(ensure_location(&loc, None), "the spool dirs must be creatable");
        let req = ForwardedPermissionRequest {
            id: "req-perm005".to_string(),
            response_nonce: create_nonce(),
            created_at: now_millis(),
            requester_session_id: "child-1".to_string(),
            target_session_id: parent.to_string(),
            requester_agent_name: "worker".to_string(),
            message: "run `bash rm -rf /`?".to_string(),
        };
        write_json_atomic(&loc.requests_dir.join("req-perm005.json"), &req, None).unwrap();
        req.id
    }

    /// Wait up to `bound` for `f` to hold, polling on the watcher's own tick granularity.
    async fn eventually(bound: Duration, mut f: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + bound;
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        f()
    }

    /// PERM-005 failure mode (1): the session id is not resolvable when the watcher spawns.
    ///
    /// Pre-fix this was a TERMINAL `return` — the task ended immediately and, because nothing but
    /// `SessionStart` ever called `maybe_start_forwarding_watcher`, the session ran its whole life
    /// with no watcher and every forwarded child ask sat in the spool until it failed closed.
    /// Upstream's `if (!location) return;` (`index.ts:1991-1993`) is a give-up for THAT hook only —
    /// the next `before_agent_start`/`input`/`tool_call` re-enters. The port's equivalent is an
    /// in-task retry on the poll tick.
    #[tokio::test]
    async fn watcher_retries_attach_until_the_session_id_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let parent = "perm005-late-session";
        let selects = Arc::new(AtomicUsize::new(0));

        let services: Arc<dyn HostServices> = Arc::new(LateSessionHost {
            id: parent.to_string(),
            // Blank for the first three attach attempts (~2 poll intervals of retry).
            blank_for: 3,
            calls: AtomicUsize::new(0),
            selects: Arc::clone(&selects),
        });

        let config = Arc::new(Mutex::new(ExtensionConfig::default()));
        let watcher =
            spawn_forwarding_watcher(
                agent_dir.clone(),
                Arc::clone(&services),
                Arc::clone(&config),
                Arc::new(AuditTrail::detached(agent_dir.join("logs"))),
                // PERM-031: a UI is present, which is the precondition for the spool being scanned.
                Arc::new(AtomicBool::new(true)),
            );

        let request_path = forwarding_location(&agent_dir, parent)
            .unwrap()
            .requests_dir
            .join("req-perm005.json");
        seed_request(&agent_dir, parent);

        let serviced =
            eventually(Duration::from_secs(10), || selects.load(Ordering::SeqCst) > 0).await;
        watcher.abort();

        assert!(
            serviced,
            "the watcher must retry its attach and eventually service the spool; pre-fix it \
             returned on the first blank session id and the request at {} was never read",
            request_path.display()
        );
    }

    /// A `HostServices` with a fixed session id that FAILS the test if a dialog is ever surfaced —
    /// under yolo mode pi auto-approves without prompting (`index.ts:1427-1429`).
    struct NoDialogHost {
        id: String,
        selects: Arc<AtomicUsize>,
    }

    impl HostServices for NoDialogHost {
        fn session_id(&self) -> Option<String> {
            Some(self.id.clone())
        }
        fn select(
            &self,
            _prompt: &str,
            _options: &serde_json::Value,
            _opts: &cyrup_ext::DialogOptions,
        ) -> Option<String> {
            self.selects.fetch_add(1, Ordering::SeqCst);
            None
        }
    }

    /// PERM-005 failure mode (4): a mid-session config change must reach the RUNNING watcher.
    ///
    /// The watcher used to take `config: ExtensionConfig` BY VALUE, so `refresh_config_and_manager`
    /// (pi `refreshExtensionConfig`, `index.ts:1600-1608`) could flip `yoloMode` and the running
    /// task would keep prompting. Here the watcher starts non-yolo, `yolo_mode` is flipped through
    /// the shared handle BEFORE any request exists, and the request that lands afterwards must be
    /// auto-approved with NO dialog.
    #[tokio::test]
    async fn watcher_reads_the_live_config_on_every_poll() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let parent = "perm005-live-config";
        let selects = Arc::new(AtomicUsize::new(0));

        let services: Arc<dyn HostServices> =
            Arc::new(NoDialogHost { id: parent.to_string(), selects: Arc::clone(&selects) });
        let config = Arc::new(Mutex::new(ExtensionConfig::default()));
        assert!(!snapshot_config(&config).yolo_mode, "the watcher starts in non-yolo mode");

        let watcher =
            spawn_forwarding_watcher(
                agent_dir.clone(),
                Arc::clone(&services),
                Arc::clone(&config),
                Arc::new(AuditTrail::detached(agent_dir.join("logs"))),
                // PERM-031: a UI is present, which is the precondition for the spool being scanned.
                Arc::new(AtomicBool::new(true)),
            );

        // The mid-session `refreshExtensionConfig`. This lands strictly AFTER the pre-fix code took
        // its by-value snapshot (that snapshot was the `config: ExtensionConfig` argument itself,
        // frozen at the call above, before the task's first poll), so a by-value port cannot see it.
        config.lock().unwrap_or_else(PoisonError::into_inner).yolo_mode = true;

        seed_request(&agent_dir, parent);
        let loc = forwarding_location(&agent_dir, parent).unwrap();
        let response_path = loc.responses_dir.join("req-perm005.json");

        let answered = eventually(Duration::from_secs(10), || response_path.exists()).await;
        watcher.abort();

        assert!(answered, "the watcher must have written a response for the seeded request");
        let body = std::fs::read_to_string(&response_path).unwrap();
        let response: ForwardedPermissionResponse = serde_json::from_str(&body).unwrap();
        assert!(
            response.approved,
            "yolo flipped mid-session must auto-APPROVE (pi `shouldAutoApprovePermissionState`, \
             `index.ts:1427-1429`); a by-value config snapshot would have kept the old mode"
        );
        assert_eq!(
            selects.load(Ordering::SeqCst),
            0,
            "yolo auto-approval surfaces no dialog at all"
        );
    }
}
