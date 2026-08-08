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

/// pi `PERMISSION_FORWARDING_TIMEOUT_MS = 10 * 60 * 1000` (`permission-forwarding.ts:7`): the CHILD's
/// blocking-wait deadline AND the parent's expired-on-read cutoff.
pub const PERMISSION_FORWARDING_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The ops/test override for the child wait bound (defaults to the pi-faithful
/// [`PERMISSION_FORWARDING_TIMEOUT`]). A finite positive milliseconds value shortens the child's
/// deadline — the seam the fail-closed timeout proof (`tests/forwarding_subprocess.rs`) drives so the
/// 10-minute production default never has to elapse in a test.
pub const CHILD_WAIT_TIMEOUT_ENV: &str = "CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS";

/// pi `PERMISSION_FORWARDING_AGENT_DIR_ENV_KEY` (`permission-forwarding.ts:10`) — the cyrup analog of
/// the one non-subagent-scoped agent-dir override (the explicit, always-consulted level of pi's
/// 5-level precedence). The 3 subagent-only middle levels (`PI_DELEGATED_AUTH_RUNTIME_DIR` etc.) are
/// N/A on cyrup, whose subagents share the parent's agent dir (port doc §12 open-Q 5); the default
/// (the passed `agent_dir`, pi `defaultAgentDir = PI_AGENT_DIR`) is the last level.
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
    let agent_dir = std::env::var(FORWARDING_AGENT_DIR_ENV)
        .ok()
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

#[cfg(unix)]
fn set_restrictive_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_restrictive_mode(_path: &Path, _mode: u32) {}

/// pi `ensureDirectoryExists` (`index.ts:1030-1039`): mkdir recursive + chmod `0o700`. Returns `true`
/// only if all three dirs are ready.
#[must_use]
pub fn ensure_location(location: &ForwardingLocation) -> bool {
    [&location.session_root, &location.requests_dir, &location.responses_dir]
        .into_iter()
        .all(|dir| {
            if std::fs::create_dir_all(dir).is_err() {
                return false;
            }
            set_restrictive_mode(dir, 0o700);
            true
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

/// pi `writeJsonFileAtomic` (`index.ts:1166-1178`): write a unique `<path>.<pid>.<uuid>.tmp` with
/// `O_EXCL` (`wx`, no-clobber) + `0o600`, chmod, then `rename` into place, then chmod again. On any
/// failure the temp is removed. The no-clobber flag on the (unique) temp is load-bearing for
/// at-most-once creation.
///
/// # Errors
/// Returns [`PermissionError::Io`] if serialization, the exclusive create/write, or the rename fails.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), PermissionError> {
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
        set_restrictive_mode(&temp, 0o600);
        std::fs::rename(&temp, path)?;
        set_restrictive_mode(path, 0o600);
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(PermissionError::Io(e.to_string()));
    }
    Ok(())
}

/// pi `readForwardedPermissionRequest` (`index.ts:1180-1211`): parse + strict-schema-validate; any
/// malformed/missing field → `None` (serde enforces the required fields).
#[must_use]
pub fn read_request(path: &Path) -> Option<ForwardedPermissionRequest> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// pi `readForwardedPermissionResponse` (`index.ts:1213-1242`).
#[must_use]
pub fn read_response(path: &Path) -> Option<ForwardedPermissionResponse> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// pi `tryRemoveDirectoryIfEmpty` × 3 (`cleanupPermissionForwardingLocationIfEmpty`,
/// `index.ts:1089-1121`): best-effort removal of now-empty spool dirs (leaf → root).
fn cleanup_location_if_empty(location: &ForwardingLocation) {
    for dir in [&location.requests_dir, &location.responses_dir, &location.session_root] {
        let is_empty = std::fs::read_dir(dir).map(|mut it| it.next().is_none()).unwrap_or(false);
        if is_empty {
            let _ = std::fs::remove_dir(dir);
        }
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
    std::env::var(CHILD_WAIT_TIMEOUT_ENV)
        .ok()
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
) -> PermissionPromptDecision {
    let Some(target) = normalize_session_id(target_session_id) else {
        return denied(); // pi null-target deny (`index.ts:1267-1272`).
    };
    let location = match forwarding_location(default_agent_dir, &target) {
        Ok(l) => l,
        Err(_) => return denied(),
    };
    if !ensure_location(&location) {
        return denied(); // pi unavailable-dirs deny (`index.ts:1275-1280`).
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

    if write_json_atomic(&request_path, &request).is_err() {
        return denied(); // pi request-write failure deny (`index.ts:1309-1312`).
    }

    // pi poll-to-deadline (`index.ts:1314-1343`). Reuse ONE `notify::PollWatcher` on the responses dir
    // for low-latency wakeups (fed by the parent's response write), with the poll interval as the
    // fallback tick — the pi `fs.watch` + `min(POLL, remaining)` structure.
    let mut watcher = watch_dir(&location.responses_dir).ok();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if response_path.exists() {
            let bound = read_response(&response_path)
                .filter(|resp| response_is_bound(resp, &request, &target));
            let _ = std::fs::remove_file(&response_path); // consume-on-read (pi `:1333`).
            if let Some(resp) = bound {
                let _ = std::fs::remove_file(&request_path);
                cleanup_location_if_empty(&location);
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

    // pi timeout deny (`index.ts:1345-1354`).
    let _ = std::fs::remove_file(&request_path);
    cleanup_location_if_empty(&location);
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
) {
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
        Err(_) => return,
    };
    request_files.sort(); // pi `.sort()` (`index.ts:1375`) — deterministic order.

    for request_path in request_files {
        let request = match read_request(&request_path) {
            Some(r) => r,
            None => {
                let _ = std::fs::remove_file(&request_path); // invalid → delete (pi `:1392-1395`).
                continue;
            }
        };

        // pi `isForwardedPermissionRequestForSession` (`:1397-1403`): drop a request for another
        // session (a hostile/stale spool entry cannot steer THIS parent's human).
        if normalize_session_id(&request.target_session_id).as_deref() != Some(current.as_str()) {
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
            let _ = std::fs::remove_file(&request_path);
            continue;
        }
        let response_path = location.responses_dir.join(format!("{}.json", request.id));
        if validate_contains_root(&location.responses_dir, &response_path).is_err() {
            let _ = std::fs::remove_file(&request_path);
            continue;
        }

        let decision = resolve_forwarded_decision(&request, services, config).await;

        let response = ForwardedPermissionResponse {
            request_id: request.id.clone(),
            response_nonce: request.response_nonce.clone(), // echo the child's nonce (pi `:1486`).
            approved: decision.approved,
            state: decision.state,
            denial_reason: decision.denial_reason.clone(),
            responder_session_id: current.clone(),
            responded_at: now_millis(),
        };
        if write_json_atomic(&response_path, &response).is_err() {
            continue; // pi response-write failure: leave the request for a retry (`:1493-1496`).
        }
        let _ = std::fs::remove_file(&request_path); // pi `:1498`.
    }

    // pi `if (!options.preserveLocation) { cleanupPermissionForwardingLocationIfEmpty(location); }`
    // (`index.ts:1501-1503`).
    if !options.preserve_location {
        cleanup_location_if_empty(&location);
    }
}

/// The per-request decision (v0.8.0 pi `index.ts:1170-1230`): expired → deny; yolo → approve; else surface
/// the SAME `select`/`input` dialog a local ask uses ([`LocalAskChannel`]) UNDER the one host-owned,
/// session-scoped human-interaction lock (C3), with the configured auto-deny prompt timeout.
async fn resolve_forwarded_decision(
    request: &ForwardedPermissionRequest,
    services: &Arc<dyn HostServices>,
    config: &ExtensionConfig,
) -> PermissionPromptDecision {
    let age_ms = now_millis().saturating_sub(request.created_at);
    if age_ms >= i64::try_from(PERMISSION_FORWARDING_TIMEOUT.as_millis()).unwrap_or(i64::MAX) {
        // pi expired-on-read (v0.8.0 `index.ts:1172-1182`).
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
        return PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Approved,
            denial_reason: None,
        };
    }

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
        let _ = ensure_location(&location);

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
        )
        .await;

        let mut watcher = watch_dir(&location.requests_dir).ok();
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
        write_json_atomic(&path, &req).unwrap();
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
        assert!(ensure_location(&loc), "the spool dirs must be creatable");
        let req = ForwardedPermissionRequest {
            id: "req-perm005".to_string(),
            response_nonce: create_nonce(),
            created_at: now_millis(),
            requester_session_id: "child-1".to_string(),
            target_session_id: parent.to_string(),
            requester_agent_name: "worker".to_string(),
            message: "run `bash rm -rf /`?".to_string(),
        };
        write_json_atomic(&loc.requests_dir.join("req-perm005.json"), &req).unwrap();
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
            spawn_forwarding_watcher(agent_dir.clone(), Arc::clone(&services), Arc::clone(&config));

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
            spawn_forwarding_watcher(agent_dir.clone(), Arc::clone(&services), Arc::clone(&config));

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
