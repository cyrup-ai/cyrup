//! The NATIVE supervisor channel — a port of `pi-subagents/src/intercom/native-supervisor-channel.ts`.
//!
//! # Why this exists
//!
//! Upstream deleted `extension/companion-suggestions.ts` and, in the SAME commit (`3ac0ef5`, "Make
//! supervisor coordination native", 2026-07-03 — three days before v0.34.0), added this file. The
//! change is one idea: **supervisor coordination stops depending on an installed intercom package.**
//! `intercom-bridge.ts` lost its on-disk `pi-intercom` extension-directory probe and now reports a
//! constant `NATIVE_INTERCOM_EXTENSION_DIR = "native:pi-subagents-supervisor-channel"` with
//! `supervisorChannelAvailable: true`, because the channel is nothing but a directory of JSON files
//! under the shared temp root.
//!
//! cyrup's supervisor coordination is broker-backed: a child's `contact_supervisor`
//! (`cyrup-intercom`) asks over a Unix socket, and the supervisor must have a broker PRESENCE for
//! the ask to reach anyone. But `cyrup_intercom::is_installed` gates a plain (non-child) session on
//! `CYRUP_INTERCOM` being truthy or `<agent dir>/intercom/config.json` existing, so an orchestrator
//! that never opted in registers no presence at all — a child's ask then addresses a supervisor the
//! broker has never heard of. That is exactly the state upstream stopped tolerating: the file
//! channel needs no broker, no socket and no opt-in.
//!
//! # The protocol (`native-supervisor-channel.ts:18-26, 84-101`)
//!
//! ```text
//! <temp root>/supervisor-channels/<runId>-<agent>-<childIndex>/
//!   requests/<requestId>.json   written by the CHILD, deleted once resolved/expired/inactive
//!   replies/<requestId>.json    written by the PARENT
//! ```
//!
//! Both segment names go through [`safe_segment`] (upstream's `safeSegment`), so a hostile run id or
//! persona name can never escape the channel root.
//!
//! # Mechanism divergences (stated, with the reason)
//!
//! * Upstream guards both registrations with `hasTool(pi, name)` (`:296-300, 331-333, 634-637`) —
//!   "register only if pi-intercom did not already". cyrup's `InitApi` has no tool-registry query at
//!   `init` time, so the same precedence is expressed by the CALLER: the parent tool is registered
//!   under [`NATIVE_SUPERVISOR_TOOL_NAME`] (upstream's own non-colliding alias, `:21`), and the
//!   child-side tool is gated on the intercom bridge being unable to attach at all
//!   ([`native_child_client_should_register`]).
//! * Upstream polls with `setInterval`; cyrup's poller is a `tokio` task the same
//!   [`crate::background::watch`] machinery already uses, and surfaces a request through
//!   [`cyrup_ext::host::HostServices::inject_message`] rather than `pi.sendMessage` — the identical
//!   hand-off `HostServicesCompletionSink`/`HostServicesControlNoticeSink` already make.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use cyrup_core::{CancelToken, Content, ExecMode, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

/// `NATIVE_INTERCOM_EXTENSION_DIR` (`intercom/intercom-bridge.ts:8`) — the constant the doctor report's
/// intercom section renders in place of the old on-disk `pi-intercom` extension-directory probe.
/// It is not a path: it is a sentinel meaning "the supervisor channel is this process's own
/// filesystem channel", which is why `diagnoseIntercomBridge` reports
/// `supervisorChannelAvailable: true` unconditionally (`intercom/intercom-bridge.ts:141`).
pub const NATIVE_SUPERVISOR_EXTENSION_DIR: &str = "native:cyrup-subagents-supervisor-channel";

/// `NATIVE_SUPERVISOR_TOOL_NAME` (`native-supervisor-channel.ts:21`): the PARENT-side tool name.
/// Deliberately not `intercom` — upstream registers this alias so the native channel never
/// overrides a real pi-intercom `intercom` tool (`:640`, and the tool's own description says so).
pub const NATIVE_SUPERVISOR_TOOL_NAME: &str = "subagent_supervisor";

/// The bare tool name upstream's SECOND registration takes on each side (`:637` on the parent,
/// `:307` on the child) — always guarded by `!hasTool(pi, "intercom")`, i.e. "take this name only
/// if nothing else owns it".
pub const INTERCOM_TOOL_NAME: &str = "intercom";

/// `MAX_MESSAGE_BYTES` (`:22`).
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// `DEFAULT_ASK_TIMEOUT_MS` (`:23`).
const DEFAULT_ASK_TIMEOUT_MS: u64 = 10 * 60 * 1000;
/// `CHANNEL_POLL_MS = Math.min(POLL_INTERVAL_MS, 500)` (`:25`).
const CHANNEL_POLL_MS: u64 = 500;
/// `STALE_EMPTY_CHANNEL_AGE_MS` (`:26`).
const STALE_EMPTY_CHANNEL_AGE_MS: u64 = 60 * 1000;
/// `STALE_EMPTY_CHANNEL_CLEANUP_INTERVAL_MS` (`:27`).
const STALE_EMPTY_CHANNEL_CLEANUP_INTERVAL_MS: u64 = 60 * 1000;

const REQUESTS_DIR: &str = "requests";
const REPLIES_DIR: &str = "replies";

/// `PI_INTERCOM_ASK_TIMEOUT_MS` (`:180`) under cyrup's `CYRUP_` prefix — the SAME var
/// `cyrup_intercom::identity::ENV_INTERCOM_ASK_TIMEOUT_MS` reads, so a user who tunes the broker
/// ask timeout tunes the file channel identically.
pub const ENV_ASK_TIMEOUT_MS: &str = "CYRUP_INTERCOM_ASK_TIMEOUT_MS";

// =================================================================================================
// Paths (`native-supervisor-channel.ts:18-20, 74-101`)
// =================================================================================================

/// `safeSegment` (`:74-76`): trim, collapse every run of characters outside `[A-Za-z0-9._-]` to a
/// single `-`, strip leading/trailing `-`, and fall back to `"unknown"` when nothing survives.
#[must_use]
pub fn safe_segment(value: &str) -> String {
    let trimmed = value.trim();
    let mut out = String::with_capacity(trimmed.len());
    let mut last_dash = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let cleaned = out.trim_matches('-');
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned.to_string()
    }
}

/// `SUPERVISOR_CHANNEL_ROOT = path.join(TEMP_ROOT_DIR, "supervisor-channels")` (`:18`).
#[must_use]
pub fn supervisor_channel_root() -> PathBuf {
    crate::spawn::nested_events::temp_root_dir().join("supervisor-channels")
}

/// `resolveSupervisorChannelDir` (`:78-80`).
#[must_use]
pub fn resolve_supervisor_channel_dir(run_id: &str, agent: &str, child_index: usize) -> PathBuf {
    supervisor_channel_root().join(format!(
        "{}-{}-{child_index}",
        safe_segment(run_id),
        safe_segment(agent)
    ))
}

/// `ensureSupervisorChannelDir` (`:82-85`): create `requests/` and `replies/` (mode `0o700`).
///
/// # Errors
///
/// Propagates the underlying `create_dir_all` failure.
pub fn ensure_supervisor_channel_dir(channel_dir: &Path) -> std::io::Result<()> {
    for sub in [REQUESTS_DIR, REPLIES_DIR] {
        let dir = channel_dir.join(sub);
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    Ok(())
}

fn request_path(channel_dir: &Path, request_id: &str) -> PathBuf {
    channel_dir
        .join(REQUESTS_DIR)
        .join(format!("{}.json", safe_segment(request_id)))
}

fn reply_path(channel_dir: &Path, request_id: &str) -> PathBuf {
    channel_dir
        .join(REPLIES_DIR)
        .join(format!("{}.json", safe_segment(request_id)))
}

/// `Date.now()` (the crate's one clock, [`crate::time::now_epoch_millis`]) narrowed to the `u64`
/// milliseconds this module's wire records (`SupervisorRequest::created_at`,
/// `SupervisorReply::created_at`) carry. A pre-epoch clock reads as `0`, the same floor the shared
/// helper uses.
fn now_millis_u64() -> u64 {
    u64::try_from(crate::time::now_epoch_millis()).unwrap_or(0)
}

/// `askTimeoutMs` (`:178-181`): a finite, positive `CYRUP_INTERCOM_ASK_TIMEOUT_MS`, else 10 minutes.
#[must_use]
pub fn ask_timeout_ms() -> u64 {
    ask_timeout_ms_from(&|k| std::env::var(k).ok())
}

/// The env-injected form of [`ask_timeout_ms`], so the parse is testable without mutating
/// process-global environment state (this crate is `#![forbid(unsafe_code)]`).
#[must_use]
pub fn ask_timeout_ms_from(get: &dyn Fn(&str) -> Option<String>) -> u64 {
    get(ENV_ASK_TIMEOUT_MS)
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .map_or(DEFAULT_ASK_TIMEOUT_MS, |v| v as u64)
}

// =================================================================================================
// Wire types (`native-supervisor-channel.ts:29-55`)
// =================================================================================================

/// `SupervisorReason` (`:28`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorReason {
    /// A blocking decision request.
    NeedDecision,
    /// A blocking, structured-reply request.
    InterviewRequest,
    /// Fire-and-forget progress.
    ProgressUpdate,
}

impl SupervisorReason {
    /// The literal wire token (`"need_decision"` etc.).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SupervisorReason::NeedDecision => "need_decision",
            SupervisorReason::InterviewRequest => "interview_request",
            SupervisorReason::ProgressUpdate => "progress_update",
        }
    }

    /// Parse a wire token, or `None` for anything else (upstream's `parseRequestFile` reason guard,
    /// `:349`).
    #[must_use]
    pub fn from_str_exact(value: &str) -> Option<Self> {
        match value {
            "need_decision" => Some(SupervisorReason::NeedDecision),
            "interview_request" => Some(SupervisorReason::InterviewRequest),
            "progress_update" => Some(SupervisorReason::ProgressUpdate),
            _ => None,
        }
    }

    /// `expectsReply = params.reason !== "progress_update"` (`:200`).
    #[must_use]
    pub const fn expects_reply(self) -> bool {
        !matches!(self, SupervisorReason::ProgressUpdate)
    }

    /// `reasonHeading` (`:133-137`).
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            SupervisorReason::InterviewRequest => {
                "Subagent requests a structured supervisor interview."
            }
            SupervisorReason::ProgressUpdate => "Subagent progress update.",
            SupervisorReason::NeedDecision => "Subagent needs a supervisor decision.",
        }
    }
}

/// `SupervisorRequest` (`:29-45`) — the child→parent request file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorRequest {
    /// Always `"subagent.supervisor.request"` — the discriminator `parseRequestFile` checks first.
    #[serde(rename = "type")]
    pub kind: String,
    /// The request id (a UUID), also the request/reply file stem.
    pub id: String,
    /// Creation time, ms since the epoch.
    pub created_at: u64,
    /// Reply deadline for a blocking request; absent for `progress_update`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Why the child is contacting its supervisor.
    pub reason: SupervisorReason,
    /// The already-formatted, human-facing message body ([`format_child_message`]).
    pub message: String,
    /// Whether the child is blocked waiting for a reply.
    pub expects_reply: bool,
    /// The supervisor's addressable presence target, when the spawn site knew one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator_target: Option<String>,
    /// The supervisor's stable session id — the field the parent's context match keys on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator_session_id: Option<String>,
    /// The run this child belongs to.
    pub run_id: String,
    /// The child's persona name.
    pub agent: String,
    /// The child's flat index within its run.
    pub child_index: usize,
    /// The child's own deterministic presence label, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_target: Option<String>,
    /// The raw interview shape for an `interview_request`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interview: Option<serde_json::Value>,
}

/// `SupervisorReply` (`:52-57`) — the parent→child reply file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorReply {
    /// Always `"subagent.supervisor.reply"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The request this answers.
    pub request_id: String,
    /// Creation time, ms since the epoch.
    pub created_at: u64,
    /// The supervisor's answer text.
    pub message: String,
}

/// `PendingSupervisorRequest` (`:47-50`): a parsed request plus where it was read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSupervisorRequest {
    /// The parsed request.
    pub request: SupervisorRequest,
    /// The channel directory it was found under.
    pub channel_dir: PathBuf,
    /// The concrete request file.
    pub request_file: PathBuf,
}

// =================================================================================================
// Child side (`native-supervisor-channel.ts:103-131, 139-262`)
// =================================================================================================

/// `readChildMetadata`'s resolved shape (`:105-131`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildChannelMetadata {
    /// `PI_SUBAGENT_SUPERVISOR_CHANNEL_DIR`.
    pub channel_dir: PathBuf,
    /// `PI_SUBAGENT_RUN_ID`.
    pub run_id: String,
    /// `PI_SUBAGENT_CHILD_AGENT`.
    pub agent: String,
    /// `PI_SUBAGENT_CHILD_INDEX` (digits only).
    pub child_index: usize,
    /// `PI_SUBAGENT_ORCHESTRATOR_TARGET`, optional.
    pub orchestrator_target: Option<String>,
    /// `PI_SUBAGENT_ORCHESTRATOR_SESSION_ID` — REQUIRED (`:120`).
    pub orchestrator_session_id: String,
    /// `PI_SUBAGENT_INTERCOM_SESSION_NAME`, optional.
    pub child_target: Option<String>,
}

/// `readChildMetadata` (`:105-131`), env-injected so it is testable without process-global mutation.
///
/// Returns `None` unless channel dir, run id, agent, orchestrator session id are all non-blank AND
/// the child index is a run of ASCII digits — upstream's `/^\d+$/` test (`:120`), which rejects
/// `"-1"`/`"1.5"`/`""` rather than coercing them.
#[must_use]
pub fn read_child_metadata_from(
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<ChildChannelMetadata> {
    let text = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());

    let channel_dir = text(crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR)?;
    let run_id = text(crate::spawn::nested_events::RUN_ID_ENV)?;
    let agent = text(crate::spawn::intercom_target::ENV_CHILD_AGENT)?;
    let orchestrator_session_id =
        text(crate::spawn::intercom_target::ENV_ORCHESTRATOR_SESSION_ID)?;
    let raw_index = text(crate::spawn::nested_events::CHILD_INDEX_ENV)?;
    if !raw_index.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let child_index = raw_index.parse::<usize>().ok()?;

    Some(ChildChannelMetadata {
        channel_dir: PathBuf::from(channel_dir),
        run_id,
        agent,
        child_index,
        orchestrator_target: text(crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET),
        orchestrator_session_id,
        child_target: text(crate::spawn::intercom_target::ENV_INTERCOM_SESSION_NAME),
    })
}

/// [`read_child_metadata_from`] against this process's real environment.
#[must_use]
pub fn read_child_metadata() -> Option<ChildChannelMetadata> {
    read_child_metadata_from(&|k| std::env::var(k).ok())
}

/// `formatChildMessage` (`:139-166`): the human-facing body the PARENT sees, assembled from the
/// child's identity plus its own message. An `interview_request` additionally appends the
/// structured-reply instruction and the serialized interview shape.
#[must_use]
pub fn format_child_message(
    metadata: &ChildChannelMetadata,
    reason: SupervisorReason,
    message: Option<&str>,
    interview: Option<&serde_json::Value>,
) -> String {
    let mut lines = vec![
        reason.heading().to_string(),
        format!("Run: {}", metadata.run_id),
        format!("Agent: {}", metadata.agent),
        format!("Child index: {}", metadata.child_index),
    ];
    if let Some(target) = &metadata.child_target {
        lines.push(format!("Child intercom target: {target}"));
    }
    lines.push(String::new());
    if let Some(msg) = message.map(str::trim).filter(|m| !m.is_empty()) {
        lines.push(msg.to_string());
    }
    if reason == SupervisorReason::InterviewRequest {
        lines.push(String::new());
        lines.push(
            "Structured response requested. Reply with JSON, optionally fenced in ```json, \
             matching the requested interview shape."
                .to_string(),
        );
        if let Some(interview) = interview {
            lines.push(serde_json::to_string_pretty(interview).unwrap_or_else(|_| "{}".to_string()));
        }
    }
    lines.join("\n").trim_end().to_string()
}

/// `parseStructuredReply` (`:168-176`): parse the reply as JSON, unwrapping a whole-body
/// ```` ```json ```` fence first. `Err` carries the parse error text upstream puts in
/// `details.structuredReplyParseError`.
///
/// # Errors
///
/// Returns the JSON parse error rendered as a string when neither the fenced body nor the raw body
/// is valid JSON.
pub fn parse_structured_reply(message: &str) -> Result<serde_json::Value, String> {
    let trimmed = message.trim();
    let candidate = strip_json_fence(trimmed).unwrap_or(trimmed);
    serde_json::from_str(candidate).map_err(|e| e.to_string())
}

/// The whole-body ```` ``` ````/```` ```json ```` unwrap upstream expresses as
/// `/^```(?:json)?\s*([\s\S]*?)\s*```$/i`.
fn strip_json_fence(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("```")?;
    let rest = rest.strip_suffix("```")?;
    let rest = match rest.get(..4) {
        Some(tag) if tag.eq_ignore_ascii_case("json") => rest.get(4..)?,
        _ => rest,
    };
    Some(rest.trim())
}

/// Build the on-disk request for one child ask (`:194-224`), without writing it.
///
/// # Errors
///
/// Returns the caller-facing error text when a decision request carries no message
/// (`:197-199`), or when the serialized request exceeds [`MAX_MESSAGE_BYTES`] (`:220`).
pub fn build_supervisor_request(
    metadata: &ChildChannelMetadata,
    reason: SupervisorReason,
    message: Option<&str>,
    interview: Option<serde_json::Value>,
    request_id: String,
    created_at: u64,
    ask_timeout: u64,
) -> Result<SupervisorRequest, String> {
    let has_message = message.map(str::trim).is_some_and(|m| !m.is_empty());
    if reason == SupervisorReason::NeedDecision && !has_message {
        return Err("message is required for supervisor decisions.".to_string());
    }
    let expects_reply = reason.expects_reply();
    let request = SupervisorRequest {
        kind: "subagent.supervisor.request".to_string(),
        id: request_id,
        created_at,
        expires_at: expects_reply.then(|| created_at.saturating_add(ask_timeout)),
        reason,
        message: format_child_message(metadata, reason, message, interview.as_ref()),
        expects_reply,
        orchestrator_target: metadata.orchestrator_target.clone(),
        orchestrator_session_id: Some(metadata.orchestrator_session_id.clone()),
        run_id: metadata.run_id.clone(),
        agent: metadata.agent.clone(),
        child_index: metadata.child_index,
        child_target: metadata.child_target.clone(),
        interview,
    };
    let serialized = serde_json::to_string_pretty(&request)
        .map_err(|e| format!("could not serialize the supervisor request: {e}"))?;
    if serialized.len() > MAX_MESSAGE_BYTES {
        return Err("Supervisor request is too large.".to_string());
    }
    Ok(request)
}

/// `sendSupervisorRequest` (`:194-262`): write the request, then (for a blocking reason) poll the
/// reply file until it lands, the deadline passes, or the caller cancels — deleting the request file
/// on every failure path so a dead ask never lingers in the parent's pending list.
///
/// # Errors
///
/// Returns the caller-facing text for: a decision with no message, an over-size request, an I/O
/// failure creating the channel directory or writing the request, a cancelled wait, or the deadline
/// elapsing with no reply.
pub async fn send_supervisor_request(
    metadata: &ChildChannelMetadata,
    reason: SupervisorReason,
    message: Option<&str>,
    interview: Option<serde_json::Value>,
    cancel: &CancelToken,
) -> Result<(SupervisorRequest, Option<SupervisorReply>), String> {
    let created_at = now_millis_u64();
    let ask_timeout = ask_timeout_ms();
    let request = build_supervisor_request(
        metadata,
        reason,
        message,
        interview,
        uuid::Uuid::new_v4().to_string(),
        created_at,
        ask_timeout,
    )?;

    ensure_supervisor_channel_dir(&metadata.channel_dir)
        .map_err(|e| format!("could not open the supervisor channel: {e}"))?;
    let file = request_path(&metadata.channel_dir, &request.id);
    crate::background::atomic::write_atomic_json(&file, &request)
        .await
        .map_err(|e| format!("could not write the supervisor request: {e}"))?;

    if !request.expects_reply {
        return Ok((request, None));
    }

    let deadline = created_at.saturating_add(ask_timeout);
    match wait_for_reply(&metadata.channel_dir, &request.id, deadline, cancel).await {
        Ok(reply) => Ok((request, Some(reply))),
        Err(err) => {
            remove_request_file(&file);
            Err(err)
        }
    }
}

/// `waitForReply` (`:183-192`).
async fn wait_for_reply(
    channel_dir: &Path,
    request_id: &str,
    deadline: u64,
    cancel: &CancelToken,
) -> Result<SupervisorReply, String> {
    let file = reply_path(channel_dir, request_id);
    while now_millis_u64() <= deadline {
        if cancel.is_cancelled() {
            return Err("Supervisor request cancelled.".to_string());
        }
        if let Some(reply) = read_reply_file(&file, request_id) {
            return Ok(reply);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("Timed out waiting for supervisor reply.".to_string())
}

/// The reply-file validity check upstream inlines at `:187-189`: right discriminator, right
/// `requestId`, and a `message` that is actually a string.
fn read_reply_file(file: &Path, request_id: &str) -> Option<SupervisorReply> {
    let bytes = std::fs::read(file).ok()?;
    let reply: SupervisorReply = serde_json::from_slice(&bytes).ok()?;
    (reply.kind == "subagent.supervisor.reply" && reply.request_id == request_id).then_some(reply)
}

/// `removeRequestFile` (`:459-465`): best-effort; a failure leaves the timeout/reply files
/// authoritative.
fn remove_request_file(file: &Path) {
    let _ = std::fs::remove_file(file);
}

// =================================================================================================
// Parent side (`native-supervisor-channel.ts:335-437, 467-520, 553-596`)
// =================================================================================================

/// `parseRequestFile` (`:335-347`): every field guard upstream applies, so a truncated or foreign
/// JSON file is skipped rather than surfaced.
#[must_use]
pub fn parse_request_file(file: &Path, channel_dir: &Path) -> Option<PendingSupervisorRequest> {
    let bytes = std::fs::read(file).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("subagent.supervisor.request") {
        return None;
    }
    if !value.get("id").and_then(serde_json::Value::as_str).is_some_and(|s| !s.is_empty()) {
        return None;
    }
    value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .and_then(SupervisorReason::from_str_exact)?;
    if !value.get("message").and_then(serde_json::Value::as_str).is_some_and(|s| !s.is_empty()) {
        return None;
    }
    if value.get("runId").and_then(serde_json::Value::as_str).is_none()
        || value.get("agent").and_then(serde_json::Value::as_str).is_none()
        || value.get("childIndex").and_then(serde_json::Value::as_u64).is_none()
    {
        return None;
    }
    let request: SupervisorRequest = serde_json::from_value(value).ok()?;
    Some(PendingSupervisorRequest {
        request,
        channel_dir: channel_dir.to_path_buf(),
        request_file: file.to_path_buf(),
    })
}

/// `listRequestFiles` (`:349-373`): every `requests/*.json` under every channel directory. A missing
/// root is an empty list, never an error (`:353-356`).
#[must_use]
pub fn list_request_files() -> Vec<(PathBuf, PathBuf)> {
    let root = supervisor_channel_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let channel_dir = entry.path();
        let Ok(requests) = std::fs::read_dir(channel_dir.join(REQUESTS_DIR)) else {
            continue;
        };
        for request in requests.flatten() {
            let path = request.path();
            if request.file_type().is_ok_and(|t| t.is_file())
                && path.extension().is_some_and(|e| e == "json")
            {
                files.push((channel_dir.clone(), path));
            }
        }
    }
    files
}

/// `SupervisorRequestLifecycle` (`:479`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisorRequestLifecycle {
    /// Still waiting on the supervisor.
    Pending,
    /// A reply file exists.
    Resolved,
    /// Past its deadline.
    Expired,
    /// The request file is gone.
    Missing,
    /// Raised by a different orchestrator session than this one.
    WrongSession,
}

/// `requestExpiresAt` (`:481-485`).
fn request_expires_at(request: &SupervisorRequest, ask_timeout: u64) -> u64 {
    request
        .expires_at
        .unwrap_or_else(|| request.created_at.saturating_add(ask_timeout))
}

/// `requestLifecycle` (`:504-511`), minus the `inactive` arm — that arm reads pi's
/// `state.foregroundRuns`/`asyncJobs` run registries (`:487-502`), which is part 2/3 of this port
/// (the run-loop and acceptance/state batches) and has no ported analogue here yet. Its omission is
/// conservative: a request whose run has already finished simply expires on its deadline rather than
/// being reaped early, so nothing is surfaced that upstream would have hidden.
#[must_use]
pub fn request_lifecycle(
    pending: &PendingSupervisorRequest,
    current_session_id: Option<&str>,
    now: u64,
    ask_timeout: u64,
) -> SupervisorRequestLifecycle {
    if let Some(session_id) = current_session_id
        && pending.request.orchestrator_session_id.as_deref() != Some(session_id)
    {
        return SupervisorRequestLifecycle::WrongSession;
    }
    if !pending.request_file.exists() {
        return SupervisorRequestLifecycle::Missing;
    }
    if pending.request.expects_reply
        && reply_path(&pending.channel_dir, &pending.request.id).exists()
    {
        return SupervisorRequestLifecycle::Resolved;
    }
    if pending.request.expects_reply && now > request_expires_at(&pending.request, ask_timeout) {
        return SupervisorRequestLifecycle::Expired;
    }
    SupervisorRequestLifecycle::Pending
}

/// `formatPendingLine` (`:553-556`).
#[must_use]
pub fn format_pending_line(pending: &PendingSupervisorRequest) -> String {
    let r = &pending.request;
    let reply_hint = if r.expects_reply {
        format!(
            " Reply: {NATIVE_SUPERVISOR_TOOL_NAME}({{ action: \"reply\", replyTo: \"{}\", message: \"...\" }})",
            r.id
        )
    } else {
        String::new()
    };
    format!(
        "- {}: {} [{}#{}] {}.{reply_hint}",
        r.id,
        r.agent,
        r.run_id,
        r.child_index,
        r.reason.as_str()
    )
}

/// `requestVisibleText` (`:558-564`): the body injected into the parent's transcript.
#[must_use]
pub fn request_visible_text(pending: &PendingSupervisorRequest) -> String {
    let r = &pending.request;
    if r.expects_reply {
        format!(
            "{}\n\nReply with: {NATIVE_SUPERVISOR_TOOL_NAME}({{ action: \"reply\", replyTo: \"{}\", message: \"...\" }})",
            r.message, r.id
        )
    } else {
        r.message.clone()
    }
}

/// `writeReply` (`:566-576`): write the reply file, then delete the request file so no later poll
/// re-surfaces it.
///
/// # Errors
///
/// Returns the caller-facing text for a blank message (upstream's own guard, `:567`) or a failed
/// write.
pub async fn write_reply(pending: &PendingSupervisorRequest, message: &str) -> Result<(), String> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err("message is required for supervisor replies.".to_string());
    }
    let reply = SupervisorReply {
        kind: "subagent.supervisor.reply".to_string(),
        request_id: pending.request.id.clone(),
        created_at: now_millis_u64(),
        message: trimmed.to_string(),
    };
    let path = reply_path(&pending.channel_dir, &pending.request.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not open the supervisor channel: {e}"))?;
    }
    crate::background::atomic::write_atomic_json(&path, &reply)
        .await
        .map_err(|e| format!("could not write the supervisor reply: {e}"))?;
    remove_request_file(&pending.request_file);
    Ok(())
}

/// `removeStaleEmptySupervisorChannel`/`cleanupStaleEmptySupervisorChannels` (`:398-437`): drop
/// channel directories that hold no requests and no replies and have not been touched for
/// [`STALE_EMPTY_CHANNEL_AGE_MS`]. Opportunistic — a racing writer just gets picked up next pass.
/// Returns how many were removed.
pub fn cleanup_stale_empty_supervisor_channels(now: u64) -> usize {
    let root = supervisor_channel_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if remove_stale_empty_channel(&entry.path(), now) {
            removed += 1;
        }
    }
    removed
}

fn dir_mtime_ms(dir: &Path) -> u64 {
    std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn dir_is_empty(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        // ENOENT reads as empty (`readDirectoryEntries` returns `[]` for it, `:389`).
        Err(_) => true,
    }
}

fn remove_stale_empty_channel(channel_dir: &Path, now: u64) -> bool {
    let requests = channel_dir.join(REQUESTS_DIR);
    let replies = channel_dir.join(REPLIES_DIR);
    let newest = dir_mtime_ms(channel_dir)
        .max(dir_mtime_ms(&requests))
        .max(dir_mtime_ms(&replies));
    if now.saturating_sub(newest) < STALE_EMPTY_CHANNEL_AGE_MS {
        return false;
    }
    if !dir_is_empty(&requests) || !dir_is_empty(&replies) {
        return false;
    }
    let _ = std::fs::remove_dir(&requests);
    let _ = std::fs::remove_dir(&replies);
    std::fs::remove_dir(channel_dir).is_ok()
}

// =================================================================================================
// The parent channel + its tool (`native-supervisor-channel.ts:598-668`)
// =================================================================================================

/// The parent's live view of the channel — `createNativeSupervisorChannel`'s closure state
/// (`:598-604`): the pending map, the seen-file set, and the last stale-cleanup stamp.
#[derive(Default)]
struct ChannelState {
    pending: HashMap<String, PendingSupervisorRequest>,
    seen_files: std::collections::HashSet<PathBuf>,
    last_stale_cleanup_at: u64,
}

/// `createNativeSupervisorChannel` (`:598-668`) — the parent-side supervisor channel.
///
/// Reached by: `SubagentsExtension::init` registers [`SubagentSupervisorTool`] over this handle
/// (Full mode only, matching upstream's `registerParentTools`), and the `SessionStart` handler calls
/// [`Self::start`], which is where upstream calls `supervisorChannel.start()`
/// (`extension/index.ts:757`).
pub struct NativeSupervisorChannel {
    state: Mutex<ChannelState>,
    /// The live capability backend messages are injected through, when one is bound.
    services: Mutex<Option<Arc<dyn cyrup_ext::host::HostServices>>>,
    /// The running poll task, so `dispose` can stop it (upstream's `clearInterval`).
    poller: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for NativeSupervisorChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeSupervisorChannel")
    }
}

impl Default for NativeSupervisorChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeSupervisorChannel {
    /// A channel with no live host backend bound yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ChannelState::default()),
            services: Mutex::new(None),
            poller: Mutex::new(None),
        }
    }

    /// Bind (or rebind) the P-1 capability backend requests are injected through and whose
    /// `session_id` decides which requests belong to THIS orchestrator session.
    pub fn bind_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        *self.services.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(services);
    }

    fn services(&self) -> Option<Arc<dyn cyrup_ext::host::HostServices>> {
        self.services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn current_session_id(&self) -> Option<String> {
        self.services().and_then(|s| s.session_id())
    }

    /// `poll` (`:648-668`), one pass: reap resolved/expired/wrong-session entries, then adopt every
    /// newly-seen request file that belongs to this orchestrator session, injecting its body into the
    /// transcript. Returns the requests adopted this pass (their visible text), so a caller — and the
    /// tests — can drive the sequence deterministically instead of racing a timer.
    pub fn poll_once(&self) -> Vec<String> {
        let now = now_millis_u64();
        let ask_timeout = ask_timeout_ms();
        self.cleanup_stale_channels_if_due(now);

        // `if (!ctx) return;` (`:650-651`): with no live session there is no identity to match a
        // request against, so nothing is adopted (and nothing is reaped as "wrong session").
        let Some(session_id) = self.current_session_id() else {
            return Vec::new();
        };

        let mut adopted = Vec::new();
        let mut to_inject = Vec::new();
        {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            refresh_pending(&mut state.pending, Some(session_id.as_str()), now, ask_timeout);

            for (channel_dir, file) in list_request_files() {
                if state.seen_files.contains(&file) {
                    continue;
                }
                let Some(pending) = parse_request_file(&file, &channel_dir) else {
                    continue;
                };
                if pending.request.orchestrator_session_id.as_deref() != Some(session_id.as_str()) {
                    continue;
                }
                let lifecycle = request_lifecycle(&pending, None, now, ask_timeout);
                state.seen_files.insert(file.clone());
                if lifecycle != SupervisorRequestLifecycle::Pending {
                    cleanup_lifecycle(&pending, lifecycle);
                    continue;
                }
                let text = request_visible_text(&pending);
                if pending.request.expects_reply {
                    state.pending.insert(pending.request.id.clone(), pending);
                } else {
                    remove_request_file(&pending.request_file);
                }
                adopted.push(text.clone());
                to_inject.push(text);
            }
        }

        if let Some(services) = self.services() {
            for content in to_inject {
                let _ = services.inject_message(
                    &content,
                    Some(SUPERVISOR_REQUEST_MESSAGE_TYPE),
                    true,
                    // `{ triggerTurn: true }` (`:687` @v0.43.0) — a supervisor request is exactly the
                    // case where the orchestrator must act, so it starts a turn rather than sitting
                    // in the transcript until the human happens to type.
                    true,
                );
            }
        }
        adopted
    }

    fn cleanup_stale_channels_if_due(&self, now: u64) {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if now.saturating_sub(state.last_stale_cleanup_at) < STALE_EMPTY_CHANNEL_CLEANUP_INTERVAL_MS
        {
            return;
        }
        state.last_stale_cleanup_at = now;
        drop(state);
        cleanup_stale_empty_supervisor_channels(now);
    }

    /// `start` (`:655-661`): one immediate pass, then a [`CHANNEL_POLL_MS`] loop. Idempotent — a
    /// second call while a poller is already running is a no-op, matching upstream's `if (poller)
    /// return`.
    pub fn start(self: &Arc<Self>) {
        let mut slot = self.poller.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        drop(self.poll_once());
        let this = Arc::clone(self);
        *slot = Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(CHANNEL_POLL_MS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                drop(this.poll_once());
            }
        }));
    }

    /// `dispose` (`:662-667`): stop the poller and drop all in-memory state.
    pub fn dispose(&self) {
        if let Some(handle) = self
            .poller
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            handle.abort();
        }
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending.clear();
        state.seen_files.clear();
    }

    /// The pending requests, newest-id order irrelevant (upstream iterates the Map).
    #[must_use]
    pub fn pending(&self) -> Vec<PendingSupervisorRequest> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .values()
            .cloned()
            .collect()
    }

    /// `resolvePendingRequest` (`:578-594`) then `writeReply` (`:566-576`): answer one pending
    /// request and drop it from the pending map.
    ///
    /// # Errors
    ///
    /// Returns the caller-facing text when `reply_to` names nothing pending, when `to` matches
    /// several requests, when nothing needs a reply, when several do and no `reply_to` was given, or
    /// when the write fails.
    pub async fn reply(
        &self,
        reply_to: Option<&str>,
        to: Option<&str>,
        message: &str,
    ) -> Result<PendingSupervisorRequest, String> {
        let chosen = self.resolve_pending(reply_to, to)?;
        write_reply(&chosen, message).await?;
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .remove(&chosen.request.id);
        Ok(chosen)
    }

    /// `resolvePendingRequest` (`:578-594`).
    fn resolve_pending(
        &self,
        reply_to: Option<&str>,
        to: Option<&str>,
    ) -> Result<PendingSupervisorRequest, String> {
        let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(id) = reply_to {
            return state.pending.get(id).cloned().ok_or_else(|| {
                format!("No pending supervisor request found for replyTo '{id}'.")
            });
        }
        let requests: Vec<PendingSupervisorRequest> = state
            .pending
            .values()
            .filter(|p| p.request.expects_reply)
            .cloned()
            .collect();
        if let Some(to) = to {
            let normalized = to.to_lowercase();
            let matches: Vec<&PendingSupervisorRequest> = requests
                .iter()
                .filter(|p| {
                    p.request.id.to_lowercase().starts_with(&normalized)
                        || p.request.agent.to_lowercase() == normalized
                        || p.request
                            .child_target
                            .as_deref()
                            .is_some_and(|t| t.to_lowercase() == normalized)
                })
                .collect();
            match matches.len() {
                1 => return matches.first().map(|p| (*p).clone()).ok_or_default_err(),
                n if n > 1 => {
                    return Err(format!(
                        "Multiple pending supervisor requests match '{to}'. Use replyTo."
                    ));
                }
                _ => {}
            }
        }
        match requests.len() {
            1 => requests.into_iter().next().ok_or_default_err(),
            0 => Err("No pending supervisor requests need a reply.".to_string()),
            _ => Err("Multiple pending supervisor requests need replies. Use replyTo.".to_string()),
        }
    }
}

/// A tiny helper so the single-match arms above stay `unwrap`-free under the no-panic policy.
trait OkOrDefaultErr<T> {
    fn ok_or_default_err(self) -> Result<T, String>;
}

impl<T> OkOrDefaultErr<T> for Option<T> {
    fn ok_or_default_err(self) -> Result<T, String> {
        self.ok_or_else(|| "No pending supervisor requests need a reply.".to_string())
    }
}

/// The custom message type a surfaced supervisor request is injected under — upstream's
/// `customType: "subagent_supervisor_request"` (`:672`).
pub const SUPERVISOR_REQUEST_MESSAGE_TYPE: &str = "subagent_supervisor_request";

/// `refreshPendingRequests` (`:521-529`).
fn refresh_pending(
    pending: &mut HashMap<String, PendingSupervisorRequest>,
    session_id: Option<&str>,
    now: u64,
    ask_timeout: u64,
) {
    let stale: Vec<(String, SupervisorRequestLifecycle)> = pending
        .values()
        .map(|p| (p.request.id.clone(), request_lifecycle(p, session_id, now, ask_timeout)))
        .filter(|(_, lifecycle)| *lifecycle != SupervisorRequestLifecycle::Pending)
        .collect();
    for (id, lifecycle) in stale {
        if let Some(p) = pending.remove(&id) {
            cleanup_lifecycle(&p, lifecycle);
        }
    }
}

/// `cleanupRequestLifecycle` (`:531-533`).
fn cleanup_lifecycle(pending: &PendingSupervisorRequest, lifecycle: SupervisorRequestLifecycle) {
    if matches!(
        lifecycle,
        SupervisorRequestLifecycle::Resolved | SupervisorRequestLifecycle::Expired
    ) {
        remove_request_file(&pending.request_file);
    }
}

// =================================================================================================
// The parent tool (`buildParentIntercomTool`, `native-supervisor-channel.ts:596-632`)
// =================================================================================================

/// The PARENT-side `subagent_supervisor` tool — `buildParentIntercomTool` (`:596-632`).
///
/// Reached by: a child writes a blocking request into its channel directory; the poller injects it
/// into the orchestrator's transcript with `triggerTurn`; the orchestrator (model or human) calls
/// `subagent_supervisor({ action: "reply", replyTo, message })`, which writes the reply file the
/// still-blocked child is polling for.
///
/// Every value the `action` enum advertises has a dispatch arm below — `status`, `pending`, `list`
/// and `reply` do work; `send` and `ask` return upstream's exact refusal text (`:625-627`), which is
/// a real arm, not a fallthrough: a child initiates asks with `contact_supervisor`, never the parent.
pub struct SubagentSupervisorTool {
    channel: Arc<NativeSupervisorChannel>,
    parameters: serde_json::Value,
    /// Which of upstream's TWO parent registrations this instance is.
    ///
    /// `buildParentIntercomTool(pending, state, name = "intercom")` (`:596-627`) is ONE builder
    /// called twice — once under [`NATIVE_SUPERVISOR_TOOL_NAME`] and once under the bare name
    /// `intercom` (`:636-637`) — with identical parameters and identical dispatch, differing only
    /// in name, label and description. Same here: one type, one dispatch, two identities.
    alias: bool,
}

impl SubagentSupervisorTool {
    /// Build the tool over a live parent channel, under upstream's non-colliding
    /// [`NATIVE_SUPERVISOR_TOOL_NAME`] (`:636`).
    #[must_use]
    pub fn new(channel: Arc<NativeSupervisorChannel>) -> Self {
        Self::build(channel, false)
    }

    /// Upstream's SECOND parent registration (`:637`): the same tool under the bare name
    /// `intercom`.
    ///
    /// Why it exists at all: upstream guards it with `if (!hasTool(pi, "intercom"))`, so it takes
    /// the name only when nothing else owns it. A model that has learned to reach a supervisor via
    /// `intercom` — the name pi-intercom uses, the name the child-side bridge instruction names,
    /// and the name every existing prompt/skill that predates the native channel uses — otherwise
    /// finds no such tool at all on an orchestrator that never installed intercom. That
    /// orchestrator is exactly the one the native channel was built for.
    ///
    /// `InitApi` has no `hasTool`, so the caller decides the precedence from the one signal that
    /// determines whether `cyrup-intercom` will own the name — see
    /// [`native_intercom_alias_should_register`].
    #[must_use]
    pub fn new_intercom_alias(channel: Arc<NativeSupervisorChannel>) -> Self {
        Self::build(channel, true)
    }

    fn build(channel: Arc<NativeSupervisorChannel>, alias: bool) -> Self {
        Self {
            channel,
            alias,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "send", "ask", "reply", "pending", "status"],
                    },
                    "to": { "type": "string" },
                    "message": { "type": "string" },
                    "replyTo": { "type": "string" },
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        }
    }
}

#[async_trait]
impl Tool for SubagentSupervisorTool {
    fn name(&self) -> &str {
        if self.alias {
            INTERCOM_TOOL_NAME
        } else {
            NATIVE_SUPERVISOR_TOOL_NAME
        }
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    /// Upstream's two descriptions, verbatim (`:601-603`). The alias drops the "without overriding
    /// pi-intercom" clause, which only makes sense on the tool whose whole reason for a
    /// non-colliding name is that it does not.
    fn description(&self) -> &str {
        if self.alias {
            "Native cyrup-subagents supervisor channel. Use reply/pending/status to answer child \
             subagent requests."
        } else {
            "Native cyrup-subagents supervisor channel. Use reply/pending/status to answer child \
             subagent requests without overriding pi-intercom."
        }
    }

    fn label(&self) -> Option<&str> {
        if self.alias {
            Some("Intercom")
        } else {
            Some("Subagent Supervisor")
        }
    }

    fn prompt_snippet(&self) -> Option<&str> {
        if self.alias {
            Some("intercom: answer a blocked subagent's supervisor request")
        } else {
            Some("subagent_supervisor: answer a blocked subagent's supervisor request")
        }
    }

    fn prompt_guidelines(&self) -> Vec<&str> {
        const GUIDELINES: &[&str] = &[
            "When a subagent supervisor request appears, answer it with \
             `subagent_supervisor({ action: \"reply\", replyTo: \"<id>\", message: \"...\" })` — \
             the child is blocked until the reply lands.",
        ];
        const ALIAS_GUIDELINES: &[&str] = &[
            "When a subagent supervisor request appears, answer it with \
             `intercom({ action: \"reply\", replyTo: \"<id>\", message: \"...\" })` — the child \
             is blocked until the reply lands.",
        ];
        if self.alias { ALIAS_GUIDELINES.to_vec() } else { GUIDELINES.to_vec() }
    }

    /// Sequential: a reply mutates the shared pending map and writes a file the blocked child is
    /// polling, so two concurrent replies must not interleave.
    fn execution_mode(&self) -> ExecMode {
        ExecMode::Sequential
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // `refreshPendingRequests(pending, state, state.lastUiContext)` (`:602`) — every action
        // starts from a reaped view, so a resolved/expired request never shows up in `pending`.
        drop(self.channel.poll_once());

        let action = params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolError::new(format!("{} requires an `action`", self.name())))?;
        let message = params.get("message").and_then(serde_json::Value::as_str);
        let reply_to = params.get("replyTo").and_then(serde_json::Value::as_str);
        let to = params.get("to").and_then(serde_json::Value::as_str);

        match action {
            // `:603-605`
            "status" => {
                let pending = self.channel.pending();
                Ok(json_result(
                    format!(
                        "Native supervisor channel active. Pending replies: {}.",
                        pending.len()
                    ),
                    serde_json::json!({
                        "active": true,
                        "pending": pending.len(),
                        "root": supervisor_channel_root().display().to_string(),
                    }),
                ))
            }
            // `:606-609`
            "pending" | "list" => {
                let pending = self.channel.pending();
                let lines: Vec<String> = pending
                    .iter()
                    .filter(|p| p.request.expects_reply)
                    .map(format_pending_line)
                    .collect();
                let text = if lines.is_empty() {
                    "No pending supervisor requests.".to_string()
                } else {
                    lines.join("\n")
                };
                Ok(json_result(
                    text,
                    serde_json::json!({ "pending": public_pending(&pending) }),
                ))
            }
            // `:610-616`
            "reply" => {
                let answered = self
                    .channel
                    .reply(reply_to, to, message.unwrap_or_default())
                    .await
                    .map_err(ToolError::new)?;
                Ok(json_result(
                    format!("Replied to supervisor request {}.", answered.request.id),
                    serde_json::json!({
                        "replyTo": answered.request.id,
                        "runId": answered.request.run_id,
                        "agent": answered.request.agent,
                    }),
                ))
            }
            // `:617-619` — a real arm with upstream's verbatim refusal, not a fallthrough.
            "send" | "ask" => Err(ToolError::new(
                "Native cyrup-subagents intercom currently handles supervisor replies. Child \
                 agents initiate asks with contact_supervisor.",
            )),
            other => Err(ToolError::new(format!(
                "Unsupported intercom action: {other}"
            ))),
        }
    }
}

/// `publicPendingRequests` (`:586-594`).
fn public_pending(pending: &[PendingSupervisorRequest]) -> serde_json::Value {
    serde_json::Value::Array(
        pending
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.request.id,
                    "runId": p.request.run_id,
                    "agent": p.request.agent,
                    "childIndex": p.request.child_index,
                    "reason": p.request.reason.as_str(),
                    "expectsReply": p.request.expects_reply,
                })
            })
            .collect(),
    )
}

fn json_result(text: String, details: serde_json::Value) -> ToolResult {
    ToolResult {
        content: vec![Content::text(text)],
        details: Some(details),
        terminate: false,
        ..Default::default()
    }
}

// =================================================================================================
// The child tool (`registerNativeSupervisorClient`, `native-supervisor-channel.ts:294-333`)
// =================================================================================================

/// The CHILD-side `contact_supervisor` over the file channel — upstream's
/// `registerNativeSupervisorClient`'s first tool (`:296-311`).
pub struct NativeContactSupervisorTool {
    metadata: ChildChannelMetadata,
    parameters: serde_json::Value,
}

impl NativeContactSupervisorTool {
    /// Build the tool for a child whose channel metadata has already resolved.
    #[must_use]
    pub fn new(metadata: ChildChannelMetadata) -> Self {
        Self {
            metadata,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "enum": ["need_decision", "interview_request", "progress_update"],
                    },
                    "message": { "type": "string" },
                    "interview": { "type": "object", "additionalProperties": true },
                },
                "required": ["reason"],
                "additionalProperties": false,
            }),
        }
    }
}

#[async_trait]
impl Tool for NativeContactSupervisorTool {
    fn name(&self) -> &str {
        "contact_supervisor"
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        "Contact the parent/supervisor session for a blocking decision, structured interview, or \
         progress update."
    }

    fn label(&self) -> Option<&str> {
        Some("Contact Supervisor")
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("contact_supervisor: ask this run's supervisor for a decision, an interview, or send a progress update")
    }

    fn execution_mode(&self) -> ExecMode {
        ExecMode::Sequential
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let reason = params
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .and_then(SupervisorReason::from_str_exact)
            .ok_or_else(|| {
                ToolError::new(
                    "contact_supervisor requires reason = need_decision | interview_request | \
                     progress_update",
                )
            })?;
        let message = params.get("message").and_then(serde_json::Value::as_str);
        let interview = params.get("interview").cloned();

        let (request, reply) =
            send_supervisor_request(&self.metadata, reason, message, interview, &cancel)
                .await
                .map_err(ToolError::new)?;

        // `:225-231`: a fire-and-forget update reports queued delivery and does not block.
        let Some(reply) = reply else {
            return Ok(json_result(
                "Supervisor progress update queued.".to_string(),
                serde_json::json!({
                    "delivered": true,
                    "requestId": request.id,
                    "reason": reason.as_str(),
                }),
            ));
        };

        // `:234-244`: an interview reply is additionally parsed as structured JSON, with the parse
        // failure surfaced as `structuredReplyParseError` rather than failing the call.
        let mut details = serde_json::json!({ "requestId": request.id, "reason": reason.as_str() });
        if reason == SupervisorReason::InterviewRequest
            && let Some(map) = details.as_object_mut()
        {
            match parse_structured_reply(&reply.message) {
                Ok(value) => {
                    map.insert("structuredReply".to_string(), value);
                }
                Err(err) => {
                    map.insert(
                        "structuredReplyParseError".to_string(),
                        serde_json::Value::String(err),
                    );
                }
            }
        }
        Ok(json_result(
            format!("**Reply from supervisor:**\n{}", reply.message),
            details,
        ))
    }
}

/// The child-side `intercom` FALLBACK (`native-supervisor-channel.ts:305-321`) — upstream's SECOND
/// child registration, and the one cyrup had never ported.
///
/// It is NOT an alias of `contact_supervisor`: it has a different schema (the parent-shaped
/// `IntercomParamsSchema`) and its own dispatch, mapping four of that schema's actions onto the
/// same file channel (`:311-319`):
///
/// * `status` → a local, non-blocking "the channel is up" answer;
/// * `list` → a local "the supervisor is reachable through contact_supervisor" answer;
/// * `send` → a fire-and-forget `progress_update` request;
/// * `ask` → a BLOCKING `need_decision` request;
/// * anything else → upstream's verbatim refusal pointing at the parent's own `reply`.
///
/// Its whole purpose is name compatibility: an agent whose declared `tools:` list asks for
/// `intercom` (and whose orchestrator never installed the intercom extension, so nothing else owns
/// the name) would otherwise be launched with a tool it declared and does not have — the
/// missing-required-tool state `tool-availability.ts` exists to diagnose.
pub struct NativeChildIntercomTool {
    metadata: ChildChannelMetadata,
    parameters: serde_json::Value,
}

impl NativeChildIntercomTool {
    #[must_use]
    pub fn new(metadata: ChildChannelMetadata) -> Self {
        Self {
            metadata,
            // The PARENT-shaped schema, verbatim (`:71-76`) — upstream reuses
            // `IntercomParamsSchema` on both sides, which is why the child tool advertises
            // `reply`/`pending` it does not service and answers them with the refusal below.
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "send", "ask", "reply", "pending", "status"],
                    },
                    "to": { "type": "string" },
                    "message": { "type": "string" },
                    "replyTo": { "type": "string" },
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        }
    }
}

#[async_trait]
impl Tool for NativeChildIntercomTool {
    fn name(&self) -> &str {
        INTERCOM_TOOL_NAME
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    /// `:306`, verbatim.
    fn description(&self) -> &str {
        "Native supervisor-channel intercom fallback for subagents. Prefer contact_supervisor when \
         available."
    }

    fn label(&self) -> Option<&str> {
        Some("Intercom")
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("intercom: supervisor-channel fallback; prefer contact_supervisor")
    }

    /// Sequential for the same reason `contact_supervisor` is: `ask` blocks on a reply file and
    /// two concurrent asks from one child would interleave on the same channel directory.
    fn execution_mode(&self) -> ExecMode {
        ExecMode::Sequential
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let action = params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolError::new("intercom requires an `action`"))?;
        let message = params
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        match action {
            // `:311`
            "status" => Ok(json_result(
                "Native supervisor channel is active.".to_string(),
                serde_json::json!({ "active": true }),
            )),
            // `:312`
            "list" => Ok(json_result(
                "Supervisor session available through contact_supervisor.".to_string(),
                serde_json::json!({ "sessions": [] }),
            )),
            // `:313-314`: `send` is a fire-and-forget progress update, `ask` a blocking decision.
            "send" | "ask" => {
                let reason = if action == "send" {
                    SupervisorReason::ProgressUpdate
                } else {
                    SupervisorReason::NeedDecision
                };
                let (request, reply) = send_supervisor_request(
                    &self.metadata,
                    reason,
                    Some(message),
                    None,
                    &cancel,
                )
                .await
                .map_err(ToolError::new)?;
                match reply {
                    None => Ok(json_result(
                        "Supervisor progress update queued.".to_string(),
                        serde_json::json!({
                            "delivered": true,
                            "requestId": request.id,
                            "reason": reason.as_str(),
                        }),
                    )),
                    Some(reply) => Ok(json_result(
                        format!("**Reply from supervisor:**\n{}", reply.message),
                        serde_json::json!({
                            "requestId": request.id,
                            "reason": reason.as_str(),
                        }),
                    )),
                }
            }
            // `:315`, verbatim — a real arm, not a fallthrough.
            _ => Err(ToolError::new(
                "Native child intercom supports status, list, send, and ask. Use parent intercom \
                 reply from the supervisor session.",
            )),
        }
    }
}

/// The env var carrying this child's REQUIRED tool list — pi `REQUIRED_CHILD_TOOLS_ENV`
/// (`runs/shared/tool-availability.ts:4`, value `PI_SUBAGENT_REQUIRED_TOOLS`), written by the spawn
/// plan as a JSON array whenever the agent declared an explicit `tools:` allowlist
/// (`runs/shared/pi-args.ts:611-616`).
///
/// Read here for one purpose, upstream's own (`subagent-prompt-runtime.ts:513`): the child-side
/// `intercom` FALLBACK registers only when the agent's declared tool list actually asks for a tool
/// by that name.
pub const ENV_REQUIRED_CHILD_TOOLS: &str = "CYRUP_SUBAGENT_REQUIRED_TOOLS";

/// pi `readRequiredChildTools` (`subagent-prompt-runtime.ts:76-84`): decode
/// [`ENV_REQUIRED_CHILD_TOOLS`]. A blank/absent/malformed value is `None` — upstream throws on a
/// malformed payload, which here would take down a child over a list the parent already built, so
/// this degrades to "no explicit allowlist was declared" (the same state an agent without `tools:`
/// is in).
#[must_use]
pub fn read_required_child_tools(get: &dyn Fn(&str) -> Option<String>) -> Option<Vec<String>> {
    let raw = get(ENV_REQUIRED_CHILD_TOOLS)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str::<Vec<String>>(trimmed).ok()
}

/// Will `cyrup-intercom` supply a WORKING supervisor channel to this process?
///
/// This is cyrup's stand-in for upstream's `!hasTool(pi, name)` guards (`:296`, `:305`, `:634-637`)
/// — `InitApi` exposes no tool-registry query at `init` time, so the precedence has to be decided
/// from the signals that determine whether the intercom extension attaches at all.
/// `cyrup_intercom::intercom_extension_for_env_concrete` attaches iff BOTH:
///
/// * the intercom config's `enabled` is not the literal `false`
///   (`if !config.enabled { return Ok(None) }`); AND
/// * `cyrup_intercom::is_installed` holds — `CYRUP_INTERCOM` truthy, or
///   `<agent dir>/intercom/config.json` present — **or** the process is a metadata-carrying child.
///
/// # Why the second term is read for the ORCHESTRATOR, not for this child
///
/// The previous gate here asked only "is `enabled` literally `false`", and so returned `true` in
/// exactly one exotic configuration: an `intercom/config.json` that exists and disables itself. It
/// returned FALSE for the configuration this whole module's header names as the reason it exists —
/// "an orchestrator that never opted in registers no presence at all ... a child's ask then
/// addresses a supervisor the broker has never heard of". In that state a child DOES get
/// `cyrup-intercom`'s `contact_supervisor` (a child always attaches), the tool exists, the model
/// calls it, and the ask reaches nobody. The native file channel — which needs no broker, no socket
/// and no opt-in — stood down precisely where it was needed, and the stated user action hit the
/// gate that refused it.
///
/// So the question asked here is the one that actually decides whether the broker path works:
/// **is the intercom extension attached in the ORCHESTRATOR?** Both of its terms are readable from
/// inside the child, because both are shared with the parent — `CYRUP_INTERCOM` is inherited
/// through the spawn environment, and the agent dir is the same directory on the same disk.
///
/// # What happens if both sides register anyway
///
/// The registry is FIRST-WINS with a recorded conflict, not last-wins
/// (`cyrup-ext/src/registry.rs:216-233`), and `crates/cyrup/src/main.rs` attaches the child prompt
/// runtime BEFORE `cyrup-intercom` at all three session-build sites. So in the one state where the
/// two could overlap — an orchestrator that never installed intercom, whose metadata-carrying child
/// still attaches the intercom extension — the NATIVE tool wins and the broker-backed one is
/// dropped as a conflict. That is the correct precedence there and not an accident worth relying on
/// silently: the broker tool in that state addresses a supervisor with no presence, which is the
/// whole failure this gate exists to avoid.
///
/// The two crates cannot import each other (`cyrup-intercom` depends on this crate), so the config
/// path, the field name and the env var are pinned by this module's own tests, exactly as
/// [`crate::spawn::intercom_target`] pins the shared env-var names.
#[must_use]
pub fn intercom_supervisor_channel_available(
    get: &dyn Fn(&str) -> Option<String>,
    agent_dir: &Path,
) -> bool {
    let config_path = agent_dir.join("intercom").join("config.json");
    let bytes = std::fs::read(&config_path);
    // `cyrup_intercom::load_config` treats an unreadable/malformed file as the DEFAULT config,
    // whose `enabled` is `true` — so only an explicit `false` disables.
    if let Ok(bytes) = &bytes
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes)
        && value.get("enabled") == Some(&serde_json::Value::Bool(false))
    {
        return false;
    }
    // `cyrup_intercom::is_installed`: `CYRUP_INTERCOM` truthy, or the config file present.
    let env_opt_in = matches!(
        get(ENV_INTERCOM_INSTALL).as_deref().map(str::trim),
        Some("1") | Some("true") | Some("on") | Some("yes")
    );
    env_opt_in || bytes.is_ok()
}

/// `cyrup_intercom::INSTALL_ENV_VAR`. Restated rather than imported for the dependency-direction
/// reason above; pinned by this module's tests.
pub const ENV_INTERCOM_INSTALL: &str = "CYRUP_INTERCOM";

/// Whether the child-side native `contact_supervisor` should register — the complement of
/// [`intercom_supervisor_channel_available`].
#[must_use]
pub fn native_child_client_should_register_from(
    get: &dyn Fn(&str) -> Option<String>,
    agent_dir: &Path,
) -> bool {
    !intercom_supervisor_channel_available(get, agent_dir)
}

/// Whether the child-side native `intercom` FALLBACK should register.
///
/// pi `subagent-prompt-runtime.ts:513`: `if (readRequiredChildTools()?.includes("intercom"))
/// registerNativeSupervisorFallbackOnce();` — layered ON TOP of the `contact_supervisor` gate
/// (`registerNativeSupervisorFallbackOnce` calls `registerNativeSupervisorClientOnce` first,
/// `:271-277`). A plain child gets `contact_supervisor` only; the alias appears exactly when the
/// agent's own declared tool allowlist asked for a tool named `intercom`, which is what stops the
/// name being claimed on every child in the workspace.
#[must_use]
pub fn native_child_intercom_fallback_should_register(
    get: &dyn Fn(&str) -> Option<String>,
    agent_dir: &Path,
) -> bool {
    native_child_client_should_register_from(get, agent_dir)
        && read_required_child_tools(get)
            .is_some_and(|tools| tools.iter().any(|t| t == INTERCOM_TOOL_NAME))
}

/// Whether the PARENT-side `intercom` alias should register (upstream `:637`'s
/// `!hasTool(pi, "intercom")`): only when `cyrup-intercom` is not attached to own the name.
#[must_use]
pub fn native_intercom_alias_should_register(
    get: &dyn Fn(&str) -> Option<String>,
    agent_dir: &Path,
) -> bool {
    !intercom_supervisor_channel_available(get, agent_dir)
}

/// Process-env form of [`native_child_client_should_register_from`], kept as the name the child
/// resolver already calls.
#[must_use]
pub fn native_child_client_should_register(agent_dir: &Path) -> bool {
    native_child_client_should_register_from(&|k| std::env::var(k).ok(), agent_dir)
}

/// `$CYRUP_CODING_AGENT_DIR` (absolute verbatim, else resolved against `cwd`) if set and non-blank,
/// else `<home>/.cyrup` — byte-identical to `cyrup_intercom::paths::agent_dir_path_from`, which the
/// dependency edge (`cyrup-intercom` → this crate) forbids importing. Pinned by
/// `tests::the_agent_dir_resolution_matches_the_intercom_crates_table`.
#[must_use]
pub fn agent_dir_from(
    env: &dyn Fn(&str) -> Option<String>,
    cwd: Option<PathBuf>,
) -> PathBuf {
    if let Some(configured) = env("CYRUP_CODING_AGENT_DIR").filter(|c| !c.trim().is_empty()) {
        let configured = configured.trim().to_string();
        let p = PathBuf::from(&configured);
        return if p.is_absolute() {
            p
        } else {
            match cwd {
                Some(base) => base.join(p),
                None => p,
            }
        };
    }
    env("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(PathBuf::from))
        .or_else(std::env::home_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cyrup")
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

    fn meta(channel_dir: &Path) -> ChildChannelMetadata {
        ChildChannelMetadata {
            channel_dir: channel_dir.to_path_buf(),
            run_id: "run-XYZ".to_string(),
            agent: "reviewer".to_string(),
            child_index: 2,
            orchestrator_target: Some("subagent-chat-abcd1234".to_string()),
            orchestrator_session_id: "session-parent-1".to_string(),
            child_target: Some("subagent-reviewer-run-xyz-3".to_string()),
        }
    }

    #[test]
    fn safe_segment_matches_the_upstream_regex_behavior() {
        assert_eq!(safe_segment("  run/ID  "), "run-ID");
        assert_eq!(safe_segment("a//b__c--d"), "a-b__c--d");
        assert_eq!(safe_segment("***"), "unknown");
        assert_eq!(safe_segment(""), "unknown");
        // The traversal case the sanitiser exists for. `.` IS in upstream's allowed class
        // (`[^A-Za-z0-9._-]` is what gets replaced), so the dots survive — what does NOT survive is
        // the SEPARATOR, so the result is a single flat path component and cannot escape the root.
        assert_eq!(safe_segment("../../etc"), "..-..-etc");
        assert!(!safe_segment("../../etc").contains(std::path::MAIN_SEPARATOR));
        assert_eq!(
            resolve_supervisor_channel_dir("../../etc", "a", 0).parent(),
            Some(supervisor_channel_root().as_path()),
            "a hostile run id must still resolve to a direct child of the channel root"
        );
    }

    #[test]
    fn channel_dir_is_run_agent_index_under_the_channel_root() {
        let dir = resolve_supervisor_channel_dir("run/XYZ", "Review Bot", 2);
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some("run-XYZ-Review-Bot-2")
        );
        assert!(dir.starts_with(supervisor_channel_root()));
    }

    #[test]
    fn child_metadata_requires_all_five_vars_and_a_digit_index() {
        let base = |index: &str| {
            let index = index.to_string();
            move |k: &str| match k {
                "CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR" => Some("/tmp/ch".to_string()),
                "CYRUP_SUBAGENT_RUN_ID" => Some("run-1".to_string()),
                "CYRUP_SUBAGENT_CHILD_AGENT" => Some("worker".to_string()),
                "CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID" => Some("sess-1".to_string()),
                "CYRUP_SUBAGENT_CHILD_INDEX" => Some(index.clone()),
                _ => None,
            }
        };
        assert!(read_child_metadata_from(&base("0")).is_some());
        // pi's `/^\d+$/` rejects these rather than coercing them.
        assert!(read_child_metadata_from(&base("-1")).is_none());
        assert!(read_child_metadata_from(&base("1.5")).is_none());
        assert!(read_child_metadata_from(&base("")).is_none());
        // A missing orchestrator SESSION id (not merely the target) disables the channel — that id
        // is what the parent's context match keys on.
        let no_session = |k: &str| match k {
            "CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR" => Some("/tmp/ch".to_string()),
            "CYRUP_SUBAGENT_RUN_ID" => Some("run-1".to_string()),
            "CYRUP_SUBAGENT_CHILD_AGENT" => Some("worker".to_string()),
            "CYRUP_SUBAGENT_CHILD_INDEX" => Some("0".to_string()),
            _ => None,
        };
        assert!(read_child_metadata_from(&no_session).is_none());
    }

    #[test]
    fn ask_timeout_falls_back_on_a_non_positive_or_unparsable_value() {
        assert_eq!(ask_timeout_ms_from(&|_| None), DEFAULT_ASK_TIMEOUT_MS);
        assert_eq!(ask_timeout_ms_from(&|_| Some("1500".to_string())), 1500);
        assert_eq!(ask_timeout_ms_from(&|_| Some("0".to_string())), DEFAULT_ASK_TIMEOUT_MS);
        assert_eq!(ask_timeout_ms_from(&|_| Some("-5".to_string())), DEFAULT_ASK_TIMEOUT_MS);
        assert_eq!(ask_timeout_ms_from(&|_| Some("nope".to_string())), DEFAULT_ASK_TIMEOUT_MS);
    }

    #[test]
    fn a_decision_with_no_message_is_rejected_but_a_progress_update_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let m = meta(dir.path());
        assert!(build_supervisor_request(
            &m,
            SupervisorReason::NeedDecision,
            None,
            None,
            "id".to_string(),
            0,
            1000
        )
        .is_err());
        let update = build_supervisor_request(
            &m,
            SupervisorReason::ProgressUpdate,
            None,
            None,
            "id".to_string(),
            0,
            1000,
        )
        .expect("progress updates need no message");
        assert!(!update.expects_reply);
        assert_eq!(update.expires_at, None);
    }

    #[test]
    fn the_formatted_body_carries_the_child_identity_and_the_interview_instruction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let m = meta(dir.path());
        let body = format_child_message(
            &m,
            SupervisorReason::InterviewRequest,
            Some("need the schema"),
            Some(&serde_json::json!({ "title": "T" })),
        );
        assert!(body.starts_with("Subagent requests a structured supervisor interview."));
        assert!(body.contains("Run: run-XYZ"));
        assert!(body.contains("Agent: reviewer"));
        assert!(body.contains("Child index: 2"));
        assert!(body.contains("Child intercom target: subagent-reviewer-run-xyz-3"));
        assert!(body.contains("need the schema"));
        assert!(body.contains("Structured response requested."));
        assert!(body.contains("\"title\""));
    }

    #[test]
    fn structured_replies_unwrap_a_json_fence() {
        assert_eq!(
            parse_structured_reply("```json\n{\"a\":1}\n```").expect("fenced json parses"),
            serde_json::json!({ "a": 1 })
        );
        assert_eq!(
            parse_structured_reply("{\"a\":1}").expect("bare json parses"),
            serde_json::json!({ "a": 1 })
        );
        assert!(parse_structured_reply("not json at all").is_err());
    }

    #[test]
    fn a_foreign_or_truncated_request_file_is_skipped_not_surfaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let channel = dir.path().join("run-a-0");
        ensure_supervisor_channel_dir(&channel).expect("channel dirs");
        let bad = channel.join(REQUESTS_DIR).join("x.json");
        std::fs::write(&bad, "{\"type\":\"something.else\"}").expect("write");
        assert!(parse_request_file(&bad, &channel).is_none());
        std::fs::write(&bad, "{ truncated").expect("write");
        assert!(parse_request_file(&bad, &channel).is_none());
    }

    #[tokio::test]
    async fn a_written_request_round_trips_through_parse_and_reply() {
        let dir = tempfile::tempdir().expect("tempdir");
        let channel = dir.path().join("run-a-0");
        ensure_supervisor_channel_dir(&channel).expect("channel dirs");
        let m = meta(&channel);
        let request = build_supervisor_request(
            &m,
            SupervisorReason::NeedDecision,
            Some("which branch?"),
            None,
            "req-1".to_string(),
            now_millis_u64(),
            60_000,
        )
        .expect("request builds");
        let file = request_path(&channel, &request.id);
        crate::background::atomic::write_atomic_json(&file, &request)
            .await
            .expect("write request");

        let parsed = parse_request_file(&file, &channel).expect("the parent parses it back");
        assert_eq!(parsed.request.reason, SupervisorReason::NeedDecision);
        assert!(parsed.request.expects_reply);
        assert!(parsed.request.message.contains("which branch?"));

        // A blank reply is refused; a real one lands and deletes the request file.
        assert!(write_reply(&parsed, "   ").await.is_err());
        write_reply(&parsed, " use main ").await.expect("reply writes");
        assert!(!file.exists(), "answering must delete the request file");
        let reply = read_reply_file(&reply_path(&channel, "req-1"), "req-1")
            .expect("the child reads its reply back");
        assert_eq!(reply.message, "use main");
    }

    #[test]
    fn lifecycle_reaps_wrong_session_resolved_and_expired_requests() {
        let dir = tempfile::tempdir().expect("tempdir");
        let channel = dir.path().join("run-a-0");
        ensure_supervisor_channel_dir(&channel).expect("channel dirs");
        let m = meta(&channel);
        let request = build_supervisor_request(
            &m,
            SupervisorReason::NeedDecision,
            Some("q"),
            None,
            "req-1".to_string(),
            1_000,
            10_000,
        )
        .expect("request builds");
        let file = request_path(&channel, &request.id);
        std::fs::write(&file, serde_json::to_vec(&request).expect("json")).expect("write");
        let pending = PendingSupervisorRequest {
            request,
            channel_dir: channel.clone(),
            request_file: file.clone(),
        };

        assert_eq!(
            request_lifecycle(&pending, Some("session-parent-1"), 2_000, 10_000),
            SupervisorRequestLifecycle::Pending
        );
        assert_eq!(
            request_lifecycle(&pending, Some("some-other-session"), 2_000, 10_000),
            SupervisorRequestLifecycle::WrongSession
        );
        // expiresAt = createdAt + askTimeout = 11_000.
        assert_eq!(
            request_lifecycle(&pending, Some("session-parent-1"), 12_000, 10_000),
            SupervisorRequestLifecycle::Expired
        );
        std::fs::write(
            reply_path(&channel, "req-1"),
            serde_json::to_vec(&SupervisorReply {
                kind: "subagent.supervisor.reply".to_string(),
                request_id: "req-1".to_string(),
                created_at: 1_500,
                message: "ok".to_string(),
            })
            .expect("json"),
        )
        .expect("write reply");
        assert_eq!(
            request_lifecycle(&pending, Some("session-parent-1"), 2_000, 10_000),
            SupervisorRequestLifecycle::Resolved
        );
        std::fs::remove_file(&file).expect("remove");
        assert_eq!(
            request_lifecycle(&pending, Some("session-parent-1"), 2_000, 10_000),
            SupervisorRequestLifecycle::Missing
        );
    }

    #[test]
    fn the_pending_line_and_visible_text_carry_a_reply_recipe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let channel = dir.path().join("run-a-0");
        let m = meta(&channel);
        let request = build_supervisor_request(
            &m,
            SupervisorReason::NeedDecision,
            Some("q"),
            None,
            "req-1".to_string(),
            0,
            1000,
        )
        .expect("builds");
        let pending = PendingSupervisorRequest {
            request,
            channel_dir: channel.clone(),
            request_file: channel.join("requests").join("req-1.json"),
        };
        let line = format_pending_line(&pending);
        assert!(line.starts_with("- req-1: reviewer [run-XYZ#2] need_decision."));
        assert!(line.contains("subagent_supervisor({ action: \"reply\", replyTo: \"req-1\""));
        assert!(request_visible_text(&pending).contains("Reply with: subagent_supervisor("));
    }

    #[test]
    fn the_native_child_client_registers_only_when_intercom_is_disabled_by_config() {
        // AMENDED. This test used to assert that with NO intercom config file the native child
        // client must STAND DOWN — pinning the very hole this module's header says the native
        // channel exists to close. With no `intercom/config.json` and no `CYRUP_INTERCOM`, the
        // ORCHESTRATOR's intercom extension does not attach at all, so it registers no broker
        // presence; the child still gets `cyrup-intercom`'s `contact_supervisor` (a
        // metadata-carrying child always attaches), the model calls it, and the ask reaches
        // nobody. The gate refused to register exactly in the configuration it was built for.
        //
        // Nothing that was asserted before is weakened: `enabled: true` still stands down, and
        // `enabled: false` still registers. The no-config case is inverted, deliberately, and the
        // `CYRUP_INTERCOM` opt-in case is added.
        let dir = tempfile::tempdir().expect("tempdir");
        let no_env = |_: &str| None;

        // NO config, NO env opt-in: the orchestrator never installed intercom, so nothing is
        // listening on the broker. The file channel is the child's only working route.
        assert!(
            native_child_client_should_register_from(&no_env, dir.path()),
            "an orchestrator that never opted into intercom leaves a child's broker ask \
             unanswerable — the native file channel MUST register"
        );

        // The env opt-in alone is enough for the orchestrator to attach and hold a presence.
        let env_opt_in = |k: &str| (k == ENV_INTERCOM_INSTALL).then(|| "1".to_string());
        assert!(!native_child_client_should_register_from(&env_opt_in, dir.path()));

        // A present config file is `is_installed` on its own, and `enabled` defaults to true.
        let intercom = dir.path().join("intercom");
        std::fs::create_dir_all(&intercom).expect("mkdir");
        std::fs::write(intercom.join("config.json"), "{\"enabled\": true}").expect("write");
        assert!(!native_child_client_should_register_from(&no_env, dir.path()));

        // `enabled: false` is the state where `intercom_extension_for_env_concrete` returns `None`
        // even for a child with metadata — and it beats the env opt-in, exactly as upstream's
        // `if !config.enabled { return Ok(None) }` runs before the install check.
        std::fs::write(intercom.join("config.json"), "{\"enabled\": false}").expect("write");
        assert!(native_child_client_should_register_from(&no_env, dir.path()));
        assert!(native_child_client_should_register_from(&env_opt_in, dir.path()));
    }

    /// G106's SECOND child registration: the bare-named `intercom` fallback layers on top of the
    /// `contact_supervisor` gate and additionally requires the agent's own declared tool allowlist
    /// to name it (pi `subagent-prompt-runtime.ts:513`).
    #[test]
    fn the_child_intercom_fallback_needs_both_the_channel_and_a_declared_intercom_tool() {
        let dir = tempfile::tempdir().expect("tempdir");
        let with_tools = |tools: &str| {
            let tools = tools.to_string();
            move |k: &str| (k == ENV_REQUIRED_CHILD_TOOLS).then(|| tools.clone())
        };

        // Channel available (no intercom installed) + the agent declared `intercom`.
        assert!(native_child_intercom_fallback_should_register(
            &with_tools(r#"["read","intercom"]"#),
            dir.path()
        ));
        // Same channel, but the agent never asked for a tool by that name — a plain child gets
        // `contact_supervisor` only, and the `intercom` name stays unclaimed.
        assert!(!native_child_intercom_fallback_should_register(
            &with_tools(r#"["read","bash"]"#),
            dir.path()
        ));
        // No allowlist at all (the agent declared no `tools:`) is not a licence to claim the name.
        assert!(!native_child_intercom_fallback_should_register(&|_| None, dir.path()));
        // A malformed payload degrades to "no allowlist", never to a claim.
        assert!(!native_child_intercom_fallback_should_register(
            &with_tools("not json"),
            dir.path()
        ));

        // And when `cyrup-intercom` IS installed it owns the name, so the fallback stands down
        // even though the agent asked for it — upstream's `!hasTool(pi, "intercom")`.
        let intercom = dir.path().join("intercom");
        std::fs::create_dir_all(&intercom).expect("mkdir");
        std::fs::write(intercom.join("config.json"), "{}").expect("write");
        assert!(!native_child_intercom_fallback_should_register(
            &with_tools(r#"["intercom"]"#),
            dir.path()
        ));
    }

    /// The PARENT alias is the same tool under the bare name, with upstream's own second
    /// description — and it registers only when nothing else owns the name.
    #[test]
    fn the_parent_intercom_alias_is_the_same_tool_under_the_bare_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let channel = Arc::new(NativeSupervisorChannel::new());
        let primary = SubagentSupervisorTool::new(Arc::clone(&channel));
        let alias = SubagentSupervisorTool::new_intercom_alias(channel);

        assert_eq!(Tool::name(&primary), "subagent_supervisor");
        assert_eq!(Tool::name(&alias), "intercom");
        assert_eq!(Tool::label(&alias), Some("Intercom"));
        assert_eq!(
            Tool::description(&alias),
            "Native cyrup-subagents supervisor channel. Use reply/pending/status to answer child \
             subagent requests.",
            "upstream's SECOND description (`native-supervisor-channel.ts:602`), which drops the \
             \"without overriding pi-intercom\" clause"
        );
        assert_eq!(
            Tool::parameters(&alias),
            Tool::parameters(&primary),
            "one builder, one schema — the two registrations differ only in identity"
        );

        // Registration precedence: only when `cyrup-intercom` will not attach to own the name.
        assert!(native_intercom_alias_should_register(&|_| None, dir.path()));
        let intercom = dir.path().join("intercom");
        std::fs::create_dir_all(&intercom).expect("mkdir");
        std::fs::write(intercom.join("config.json"), "{}").expect("write");
        assert!(!native_intercom_alias_should_register(&|_| None, dir.path()));
    }

    #[test]
    fn the_agent_dir_resolution_matches_the_intercom_crates_table() {
        // Absolute `CYRUP_CODING_AGENT_DIR` wins verbatim.
        assert_eq!(
            agent_dir_from(&|k| (k == "CYRUP_CODING_AGENT_DIR").then(|| "/opt/agent".to_string()), None),
            PathBuf::from("/opt/agent")
        );
        // A relative one resolves against cwd.
        assert_eq!(
            agent_dir_from(
                &|k| (k == "CYRUP_CODING_AGENT_DIR").then(|| "rel/agent".to_string()),
                Some(PathBuf::from("/work"))
            ),
            PathBuf::from("/work/rel/agent")
        );
        // A blank one is ignored; CYRUP_HOME beats HOME; the suffix is `.cyrup`.
        assert_eq!(
            agent_dir_from(
                &|k| match k {
                    "CYRUP_CODING_AGENT_DIR" => Some("   ".to_string()),
                    "CYRUP_HOME" => Some("/h1".to_string()),
                    "HOME" => Some("/h2".to_string()),
                    _ => None,
                },
                None
            ),
            PathBuf::from("/h1/.cyrup")
        );
        assert_eq!(
            agent_dir_from(&|k| (k == "HOME").then(|| "/h2".to_string()), None),
            PathBuf::from("/h2/.cyrup")
        );
    }

    /// The cross-crate string contract: these are the SAME paths/keys
    /// `cyrup_intercom::config::load_config` reads. The two crates cannot import each other, so a
    /// rename on either side has to fail here rather than silently disable the fallback.
    #[test]
    fn the_intercom_config_contract_is_pinned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let intercom = dir.path().join("intercom");
        std::fs::create_dir_all(&intercom).expect("mkdir");
        std::fs::write(intercom.join("config.json"), "{\"enabled\": false}").expect("write");
        assert!(
            native_child_client_should_register(dir.path()),
            "the intercom config lives at <agent dir>/intercom/config.json with an `enabled` key"
        );
        // The install env var is the OTHER half of `cyrup_intercom::is_installed`, and this gate
        // now reads it. The two crates cannot import each other, so pin the spelling here —
        // a rename on the intercom side that missed this constant would silently make every
        // opted-in orchestrator register a second, competing supervisor surface.
        assert_eq!(
            ENV_INTERCOM_INSTALL, "CYRUP_INTERCOM",
            "`cyrup_intercom::INSTALL_ENV_VAR`"
        );
        assert_eq!(
            ENV_REQUIRED_CHILD_TOOLS, "CYRUP_SUBAGENT_REQUIRED_TOOLS",
            "the cyrup rename of pi's `PI_SUBAGENT_REQUIRED_TOOLS` \
             (`runs/shared/tool-availability.ts:4`)"
        );
    }
}
