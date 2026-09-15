//! The reply side: the wire shape, its bounding, and the composition that fills it in.
//!
//! This is pi `inspect-rpc.ts:33-57`, `:82-106`, `:290-320` and `:322-443`.

use std::path::{Path, PathBuf};

use crate::background::child_identity::{self, AsyncStatusChildResolution};
use crate::background::delivery::SessionGate;
use crate::background::fleet_view::{
    SessionMessageKind, SessionTranscriptMessage, read_session_messages_tail,
};
use crate::background::{RunState, resolve_async_run_id, run_status};
use crate::identity::SessionId;
use crate::workflows::{sanitize_display_text, truncate_display};

use super::read_output::{ResultOutputRequest, read_result_output, step_agent_of};
use super::request::{InspectErrorCode, InspectRequest, InspectRequestEcho, parse_inspect_request};
use super::{
    DEFAULT_MESSAGE_LINES, INSPECT_REPLY_KIND, INSPECT_REPLY_VERSION, INSPECT_WIDGET_PREFIX,
    MAX_FINAL_OUTPUT_LENGTH, MAX_ID_LENGTH, MAX_LABEL_LENGTH, MAX_MESSAGE_LINES,
    MAX_MESSAGE_TEXT_LENGTH, MAX_SERIALIZED_BYTES, MAX_TASK_LENGTH, is_valid_request_id,
};

/// pi's single `internal` sentence (`inspect-rpc.ts:399`, `:425`). Deliberately ONE string for
/// every internal failure: the reply crosses a trust boundary to a host widget, and the
/// distinguishing detail belongs in the log, not in it.
const INTERNAL_MESSAGE: &str = "Inspection could not read the async run artifacts.";
/// pi's `foreign_session` sentence (`:355`, `:364`).
const FOREIGN_SESSION_MESSAGE: &str =
    "Inspection is only available for async runs owned by the current session.";

/// The literal [`INSPECT_REPLY_KIND`], as a type — the same device
/// [`crate::background::completion_replay::ReplayVersion`] uses, and for the same reason: the
/// field is a discriminant of the on-disk/on-wire format, so a value that is not it must fail to
/// deserialize rather than round-trip as data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InspectReplyKind;

impl serde::Serialize for InspectReplyKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(INSPECT_REPLY_KIND)
    }
}

impl<'de> serde::Deserialize<'de> for InspectReplyKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw == INSPECT_REPLY_KIND {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unknown inspect reply kind {raw}"
            )))
        }
    }
}

/// The literal [`INSPECT_REPLY_VERSION`], as a type. [`InspectReplyKind`]'s sibling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InspectReplyVersion;

impl serde::Serialize for InspectReplyVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(INSPECT_REPLY_VERSION)
    }
}

impl<'de> serde::Deserialize<'de> for InspectReplyVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == INSPECT_REPLY_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unknown inspect reply version {raw}"
            )))
        }
    }
}

/// pi `InspectReplyMessage` (`inspect-rpc.ts:34-40`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectReplyMessage {
    /// The speaker, sanitized and cut to 32 units (pi `:294`).
    pub role: String,
    /// Which shape of content part this is.
    pub kind: SessionMessageKind,
    /// The part's text, bounded to [`MAX_MESSAGE_TEXT_LENGTH`] with newlines KEPT.
    pub text: String,
    /// The tool name, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `Some(true)` only — pi writes `...(message.isError === true ? { isError: true } : {})`
    /// (`:298`), i.e. the key is absent rather than `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// pi `InspectReply["truncated"]` (`inspect-rpc.ts:55`) — what the byte budget had to give up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectTruncation {
    /// The delegated task was cut.
    pub task: bool,
    /// How many of the OLDEST messages were dropped.
    pub messages: usize,
    /// The final output was cut.
    pub final_output: bool,
}

impl InspectTruncation {
    /// pi `:319`/`:421` — the key is present only when something actually was truncated.
    fn any(self) -> bool {
        self.task || self.final_output || self.messages > 0
    }
}

/// pi `InspectReply["error"]` (`inspect-rpc.ts:56`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InspectReplyError {
    /// The machine-readable code the host branches on.
    pub code: InspectErrorCode,
    /// The sentence a human reads. Upstream's, verbatim.
    pub message: String,
}

/// pi `InspectReply` (`inspect-rpc.ts:42-57`) — the whole wire shape, in upstream's own key order.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectReply {
    /// Always [`INSPECT_REPLY_KIND`].
    pub kind: InspectReplyKind,
    /// Always [`INSPECT_REPLY_VERSION`].
    pub version: InspectReplyVersion,
    /// The caller's correlation token, echoed — or the literal `invalid` when it was not one.
    pub request_id: String,
    /// Canonical run id of the inspected node. Absent on error replies that could not resolve a
    /// run (pi `:46-48`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub async_id: Option<String>,
    /// The child the request named, echoed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_id: Option<String>,
    /// The run's lifecycle state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RunState>,
    /// A short display name for the node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The delegated task, when it is attributable — see [`build_inspect_reply`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The transcript tail, newest last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<InspectReplyMessage>>,
    /// The child's delivered text (or, failing that, its failure text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_output: Option<String>,
    /// Present only when something was cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<InspectTruncation>,
    /// Present only on a refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<InspectReplyError>,
}

impl InspectReply {
    /// The reply's error code, when it is a refusal — the one thing a caller that does not render
    /// the whole reply still needs.
    #[must_use]
    pub fn error_code(&self) -> Option<InspectErrorCode> {
        self.error.as_ref().map(|error| error.code)
    }
}

/// What [`build_inspect_reply`] reads the world through — pi `InspectDeps` (`inspect-rpc.ts:70-76`),
/// with upstream's `state` object replaced by the two facts this port actually takes from it.
pub struct InspectDeps<'a> {
    /// The per-cwd async root the run id resolves against.
    pub async_root: &'a Path,
    /// The per-cwd results directory.
    pub results_dir: &'a Path,
    /// The caller's session. `None` is `no_active_session`, upstream's own first refusal.
    pub current_session: Option<&'a SessionId>,
    /// The roots a recorded `sessionFile` may be dereferenced under.
    ///
    /// [CYRUP-DELTA, unrepresentable] upstream unions `deps.state.trustedSessionRoots` with
    /// `node.status.sessionRoot` (`:373-376`); [`crate::background::RunStatus`] has no
    /// `session_root` field, so only the first half of that union exists to be supplied. The
    /// caller decides WHICH list (the executor passes its `view: "transcript"` roots, so inspect
    /// and the transcript view are confined identically); the containment MECHANISM is the same
    /// single gate either way.
    pub trusted_roots: &'a [PathBuf],
    /// Epoch millis, read ONCE and threaded — see [`ResultOutputRequest::now`].
    pub now: i64,
}

/// pi `publicText` (`inspect-rpc.ts:82-86`) — a short, single-line display value: sanitize a
/// generous prefix, then cut to the real bound. `None` for a value that sanitizes to nothing.
fn public_text(value: &str, max_length: usize) -> Option<String> {
    // pi `value.slice(0, maxLength * 4)` — a bound on the SANITIZER's input, since sanitizing can
    // only shrink; without it a megabyte of escape sequences would be scanned to produce 160
    // characters.
    let normalized = sanitize_display_text(&truncate_display(value, max_length.saturating_mul(4)));
    if normalized.is_empty() {
        None
    } else {
        Some(truncate_display(&normalized, max_length))
    }
}

/// pi `boundContent` (`inspect-rpc.ts:88-95`) — bound long-form content WITHOUT flattening it.
///
/// Upstream's own comment is the whole rationale and is carried verbatim: task/finalOutput/message
/// text must keep newlines readable, so only the two unicode line separators are normalized, since
/// `JSON.stringify` does not escape them and hosts split the widget payload on line boundaries.
fn bound_content(value: &str, max_length: usize) -> String {
    truncate_display(&value.replace(['\u{2028}', '\u{2029}'], "\n"), max_length)
}

/// pi `errorReply` (`inspect-rpc.ts:97-106`).
fn error_reply(
    echo: &InspectRequestEcho,
    code: InspectErrorCode,
    message: impl Into<String>,
) -> InspectReply {
    InspectReply {
        kind: InspectReplyKind,
        version: InspectReplyVersion,
        // pi `:101` — an unusable correlation token is replaced by the literal `invalid` rather
        // than echoed, so the host never sees a token it could not have sent.
        request_id: echo
            .request_id
            .as_deref()
            .filter(|id| is_valid_request_id(id))
            .unwrap_or("invalid")
            .to_string(),
        async_id: echo
            .async_id
            .as_deref()
            .map(|id| truncate_display(id, MAX_ID_LENGTH)),
        child_id: echo
            .child_id
            .as_deref()
            .map(|id| truncate_display(id, MAX_ID_LENGTH)),
        status: None,
        label: None,
        task: None,
        messages: None,
        final_output: None,
        truncated: None,
        error: Some(InspectReplyError {
            code,
            message: message.into(),
        }),
    }
}

/// pi `toReplyMessage` (`inspect-rpc.ts:292-300`).
fn to_reply_message(message: &SessionTranscriptMessage) -> InspectReplyMessage {
    InspectReplyMessage {
        role: truncate_display(&sanitize_display_text(&message.role), 32),
        kind: message.kind,
        text: bound_content(&message.text, MAX_MESSAGE_TEXT_LENGTH),
        name: message
            .name
            .as_deref()
            // pi `:297`'s `message.name ? … : {}` — an EMPTY name is falsy and dropped.
            .filter(|name| !name.is_empty())
            .map(|name| truncate_display(&sanitize_display_text(name), 96)),
        is_error: message.is_error.then_some(true),
    }
}

/// The reply's serialized size in bytes — pi's `Buffer.byteLength(JSON.stringify(reply), "utf-8")`.
///
/// A serialization failure is unreachable for this type (every field is a `String`, a number, a
/// `bool` or a `Vec` of those), and reporting `0` for it is the safe direction: it ends
/// [`enforce_byte_budget`]'s loop immediately instead of shifting messages until the list is empty.
fn serialized_bytes(reply: &InspectReply) -> usize {
    serde_json::to_vec(reply).map_or(0, |bytes| bytes.len())
}

/// pi `enforceByteBudget` (`inspect-rpc.ts:302-321`) — fit the reply under
/// [`MAX_SERIALIZED_BYTES`]: drop OLDEST messages first, then shrink `finalOutput`, then `task`.
/// The envelope always survives.
fn enforce_byte_budget(mut reply: InspectReply) -> InspectReply {
    let mut truncated = reply.truncated.unwrap_or_default();
    while serialized_bytes(&reply) > MAX_SERIALIZED_BYTES {
        let Some(messages) = reply.messages.as_mut() else {
            break;
        };
        if messages.is_empty() {
            break;
        }
        messages.remove(0);
        truncated.messages = truncated.messages.saturating_add(1);
    }
    // pi `:312`/`:316` shrink by `length - overshoot - 256`, in UTF-16 units, which is what
    // `truncate_display` counts.
    if let Some(final_output) = reply.final_output.as_ref() {
        let overshoot = serialized_bytes(&reply).saturating_sub(MAX_SERIALIZED_BYTES);
        if overshoot > 0 {
            let keep = utf16_len(final_output)
                .saturating_sub(overshoot)
                .saturating_sub(256);
            reply.final_output = Some(truncate_display(final_output, keep));
            truncated.final_output = true;
        }
    }
    if let Some(task) = reply.task.as_ref() {
        let overshoot = serialized_bytes(&reply).saturating_sub(MAX_SERIALIZED_BYTES);
        if overshoot > 0 {
            let keep = utf16_len(task)
                .saturating_sub(overshoot)
                .saturating_sub(256);
            reply.task = Some(truncate_display(task, keep));
            truncated.task = true;
        }
    }
    if truncated.any() {
        reply.truncated = Some(truncated);
    }
    reply
}

/// `String.prototype.length` — UTF-16 code units, the unit every bound in this module counts in.
fn utf16_len(value: &str) -> usize {
    value.chars().map(char::len_utf16).sum()
}

/// pi `buildInspectReply` (`inspect-rpc.ts:323-427`) — resolve, reconcile, gate, read, bound.
///
/// # Resolution and the session gate are SEPARATE steps, deliberately
///
/// [`resolve_async_run_id`] applies a session filter INTERNALLY (`run_id_resolver.rs:192-197`, via
/// `location_belongs_to`), which upstream's resolver does not. Passing the current session straight
/// through would make the `foreign_session` code UNREACHABLE — every foreign run would report
/// `not_found` instead, collapsing "someone else's run" into "no such run" at exactly the surface
/// whose job is to tell a user which of the two happened. So the id is resolved with `None` and
/// [`SessionGate::Strict`] is applied explicitly afterwards, against the RECONCILED status.
///
/// `Strict` and not `Permissive`: upstream refuses outright with `no_active_session` when there is
/// no current session (`:327-329`), so by the time the gate runs a session always exists and the
/// two classes agree — but the class is ported, not chosen, and `view: "transcript"`'s
/// `Permissive` (`extension/executor/status.rs`) is the one that must not be copied here.
///
/// # Task attribution
///
/// [CYRUP-DELTA, unrepresentable] upstream suppresses `task` for a FORKED child, because a fork's
/// session begins with inherited parent history and its first user message is therefore the
/// parent's, not the delegation (`:389`). cyrup's `context: "fresh" | "fork"` is a LAUNCH
/// parameter (`extension/tool/params.rs`) recorded on neither [`crate::background::RunStatus`] nor
/// [`crate::background::StepStatus`], so there is no field to read. The OTHER half of upstream's
/// guard — `!tail.truncated`, i.e. the session file is fully inside the read window, so the first
/// visible user message really is the first — IS representable and is kept. The residual is that a
/// forked child may report its inherited first user message as its task.
pub async fn build_inspect_reply(request: &InspectRequest, deps: &InspectDeps<'_>) -> InspectReply {
    let echo = InspectRequestEcho::from(request);
    let Some(current_session) = deps.current_session else {
        return error_reply(
            &echo,
            InspectErrorCode::NoActiveSession,
            "Inspection requires an active session; the request could not be attributed to a \
             session.",
        );
    };

    let resolved =
        match resolve_async_run_id(&request.async_id, deps.async_root, deps.results_dir, None) {
            Ok(resolved) => resolved,
            // pi `:334-336` — an unsafe token or an ambiguous prefix is the CALLER's problem.
            Err(error) => {
                tracing::debug!(%error, "inspect could not resolve the requested run id");
                return error_reply(
                    &echo,
                    InspectErrorCode::InvalidRequest,
                    "The requested async run could not be resolved.",
                );
            }
        };
    let Some(location) = resolved else {
        return error_reply(
            &echo,
            InspectErrorCode::NotFound,
            format!("Async run '{}' was not found.", request.async_id),
        );
    };
    // pi `:349`. [CYRUP-DELTA] upstream's `resolved.kind === "nested"` branch (`:344-347`) has no
    // port — see this module's parent doc — so the non-nested arm is the only one.
    let Some(async_dir) = location.async_dir.as_deref() else {
        return error_reply(
            &echo,
            InspectErrorCode::Stale,
            format!(
                "Async run '{}' has no remaining artifacts.",
                request.async_id
            ),
        );
    };
    // pi `:351` — `reconcileAsyncRun(...).status ?? readStatus(asyncDir)`. `reconcile_by_dir` folds
    // both: it runs the same R-SA-079 gate `view: "transcript"` runs and maps a NotFound to
    // `Ok(None)`.
    let status = match run_status::reconcile_by_dir(async_dir, deps.results_dir).await {
        Ok(Some((status, _paths))) => status,
        Ok(None) => {
            return error_reply(
                &echo,
                InspectErrorCode::Stale,
                format!(
                    "Async run '{}' artifacts are no longer available.",
                    request.async_id
                ),
            );
        }
        Err(error) => {
            tracing::debug!(%error, "inspect reconciliation failed");
            return error_reply(&echo, InspectErrorCode::Internal, INTERNAL_MESSAGE);
        }
    };
    if !SessionGate::Strict.admits(Some(current_session), status.session_id.as_ref()) {
        return error_reply(
            &echo,
            InspectErrorCode::ForeignSession,
            FOREIGN_SESSION_MESSAGE,
        );
    }

    // pi `findChildNode`'s step arm (`:144-152`), through the crate's own resolver — whose
    // `NotFound` sentence is already upstream's verbatim `Child '<id>' was not found under async
    // run '<run>'.` Its `Ambiguous` arm has no upstream twin (upstream's `ids.includes` takes the
    // FIRST match) and is reported as `not_found` too, since upstream's `{ error }` union carries
    // no code.
    let step_index = match request.child_id.as_deref() {
        None => None,
        Some(child_id) => match child_identity::resolve_async_status_child(&status, child_id) {
            AsyncStatusChildResolution::Resolved(child) => Some(child.index),
            failure => {
                return error_reply(
                    &echo,
                    InspectErrorCode::NotFound,
                    failure.failure_message().unwrap_or(INTERNAL_MESSAGE),
                );
            }
        },
    };
    let step = step_index.and_then(|index| status.steps.get(index));

    // pi `:371` — `max(1, min(MAX, trunc(lines ?? DEFAULT)))`. A JSON integer already arrives
    // truncated, so only the clamp survives, exactly as `fleet_view::transcript_line_limit` does.
    let line_limit = request.lines.map_or(DEFAULT_MESSAGE_LINES, |lines| {
        usize::try_from(lines.clamp(1, MAX_MESSAGE_LINES as i64)).unwrap_or(DEFAULT_MESSAGE_LINES)
    });
    let session_file = step
        .and_then(|step| step.session_file.clone())
        .or_else(|| status.session_file.clone());

    let mut messages: Option<Vec<InspectReplyMessage>> = None;
    let mut task: Option<String> = None;
    if let Some(session_file) = session_file.as_deref()
        && !deps.trusted_roots.is_empty()
    {
        let tail = read_session_messages_tail(session_file, line_limit, deps.trusted_roots);
        if !tail.messages.is_empty() {
            messages = Some(tail.messages.iter().map(to_reply_message).collect());
            // pi `:385-392`, minus the unrepresentable fork half — see this function's doc.
            if !tail.truncated
                && let Some(first_user) = tail.messages.iter().find(|message| {
                    message.role == "user" && message.kind == SessionMessageKind::Text
                })
            {
                task = Some(first_user.text.clone());
            }
        }
    }

    let result = match read_result_output(&ResultOutputRequest {
        results_dir: deps.results_dir,
        session_id: current_session,
        run_id: &status.run_id,
        step_index,
        trusted_roots: deps.trusted_roots,
        step_agent: step_agent_of(&status.steps, step_index),
        now: deps.now,
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            tracing::debug!(%error, run_id = %status.run_id, "inspect could not read the run artifacts");
            return error_reply(&echo, InspectErrorCode::Internal, INTERNAL_MESSAGE);
        }
    };
    // pi `:397-400` — a node that RECORDED a failure and yet produced neither output nor error
    // text means the artifacts that should hold the failure are unreadable. Reporting that as a
    // successful inspection with no `finalOutput` would show a failed child as a silent one.
    let failed_text = step
        .and_then(|step| step.error.as_deref())
        .or(status.error.as_deref());
    if result.is_empty() && failed_text.is_some() {
        return error_reply(&echo, InspectErrorCode::Internal, INTERNAL_MESSAGE);
    }
    let final_output_raw = result.output.or(result.error_text);

    let mut truncated = InspectTruncation::default();
    let bounded_task = task
        .as_deref()
        .map(|task| bound_content(task, MAX_TASK_LENGTH));
    if bounded_task.as_deref() != task.as_deref() {
        truncated.task = true;
    }
    let bounded_final = final_output_raw
        .as_deref()
        .map(|output| bound_content(output, MAX_FINAL_OUTPUT_LENGTH));
    if bounded_final.as_deref() != final_output_raw.as_deref() {
        truncated.final_output = true;
    }
    // pi `:408` — `node.label ?? step?.label ?? step?.agent ?? node.status.runId`.
    // [CYRUP-DELTA] neither `node.label` nor `step.label` exists (see the parent module doc), so
    // the ladder starts at the agent.
    let label = public_text(
        step.map_or_else(|| status.run_id.as_str(), |step| step.agent.as_str()),
        MAX_LABEL_LENGTH,
    );

    enforce_byte_budget(InspectReply {
        kind: InspectReplyKind,
        version: InspectReplyVersion,
        request_id: request.request_id.clone(),
        async_id: Some(status.run_id.as_str().to_string()),
        child_id: request.child_id.clone(),
        status: Some(status.state),
        label,
        task: bounded_task,
        // pi `:419` — an EMPTY message list is omitted, not sent as `[]`.
        messages: messages.filter(|messages| !messages.is_empty()),
        final_output: bounded_final,
        truncated: truncated.any().then_some(truncated),
        error: None,
    })
}

/// pi `encodeInspectReply` (`inspect-rpc.ts:429-431`) — the ONE line the host widget reads.
///
/// A `Vec` of exactly one line, as upstream returns: the caller splices it into a line stream.
/// Serialization cannot fail for this type; an empty vector would be the visible symptom if it
/// ever did, rather than a malformed half-line on the widget channel.
#[must_use]
pub fn encode_inspect_reply(reply: &InspectReply) -> Vec<String> {
    serde_json::to_string(reply)
        .map(|json| vec![format!("{INSPECT_WIDGET_PREFIX}{json}")])
        .unwrap_or_default()
}

/// pi `handleInspectRpcArgs` (`inspect-rpc.ts:433-443`) — parse the slash-command args and ALWAYS
/// return a correlated inspect reply.
///
/// "Always correlated" is the contract: on a parse failure the first token is echoed as the
/// `requestId` when it is a well-formed one, so the host can match the failure to the request it
/// sent rather than dropping it.
pub async fn handle_inspect_rpc_args(args: &str, deps: &InspectDeps<'_>) -> InspectReply {
    match parse_inspect_request(args) {
        Ok(request) => build_inspect_reply(&request, deps).await,
        Err(message) => {
            let echo = InspectRequestEcho {
                request_id: args
                    .split_whitespace()
                    .next()
                    .filter(|token| is_valid_request_id(token))
                    .map(str::to_string),
                ..InspectRequestEcho::default()
            };
            error_reply(&echo, InspectErrorCode::InvalidRequest, message)
        }
    }
}

/// The reply as prose, for the `action: "inspect"` tool surface.
///
/// [CYRUP-DELTA] upstream has no such rendering: its only consumer is a host widget that reads the
/// JSON envelope ([`encode_inspect_reply`]). A tool result is read by a MODEL, which cannot see a
/// widget, so the same reply is rendered as text and the JSON rides along in the result's
/// `details`. The wire shape is unchanged — this is a second view of it, not a second shape.
#[must_use]
pub fn render_inspect_reply(reply: &InspectReply) -> String {
    if let Some(error) = reply.error.as_ref() {
        return format!(
            "Inspect failed ({}): {}",
            error.code.as_str(),
            error.message
        );
    }
    let mut lines: Vec<String> = Vec::new();
    if let Some(async_id) = reply.async_id.as_deref() {
        lines.push(format!("Run: {async_id}"));
    }
    if let Some(child_id) = reply.child_id.as_deref() {
        lines.push(format!("Child: {child_id}"));
    }
    if let Some(state) = reply.status {
        lines.push(format!("State: {}", run_status::run_state_label(state)));
    }
    if let Some(label) = reply.label.as_deref() {
        lines.push(format!("Label: {label}"));
    }
    if let Some(task) = reply.task.as_deref() {
        lines.push(format!("Task: {task}"));
    }
    if let Some(messages) = reply.messages.as_ref() {
        lines.push(format!("Messages ({}):", messages.len()));
        for message in messages {
            lines.push(format!("  {}: {}", message.role, message.text));
        }
    }
    if let Some(final_output) = reply.final_output.as_deref() {
        lines.push(format!("Final output:\n{final_output}"));
    }
    if let Some(truncated) = reply.truncated
        && truncated.any()
    {
        lines.push(format!(
            "(truncated: {} message(s) dropped{}{})",
            truncated.messages,
            if truncated.final_output {
                ", final output cut"
            } else {
                ""
            },
            if truncated.task { ", task cut" } else { "" }
        ));
    }
    if lines.is_empty() {
        // Reachable only for a reply with neither error nor run id, which `build_inspect_reply`
        // does not produce — but a renderer that can return "" would be a silent tool result.
        return "Inspection returned nothing for this run.".to_string();
    }
    lines.join("\n")
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
    use crate::background::result_index::{ResultWrite, write_async_result_file};
    use crate::background::watch::tests::child_result;
    use crate::background::{
        ResultFile, RunId, RunMode, RunPaths, RunStatus, StepState, StepStatus,
    };

    fn session(raw: &str) -> SessionId {
        SessionId::parse(raw).expect("non-empty")
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        async_root: PathBuf,
        results_dir: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let async_root = dir.path().join("async");
            let results_dir = dir.path().join("results");
            Self {
                _dir: dir,
                async_root,
                results_dir,
            }
        }

        fn deps<'a>(&'a self, current: Option<&'a SessionId>) -> InspectDeps<'a> {
            InspectDeps {
                async_root: &self.async_root,
                results_dir: &self.results_dir,
                current_session: current,
                trusted_roots: &[],
                now: 1_000,
            }
        }

        /// Seed a complete, single-child run owned by `owner`: its `status.json` and its payload,
        /// the latter written through the session index so nothing lands on the public path.
        async fn seed(&self, run: &RunId, owner: &SessionId, output: &str) {
            let mut step = StepStatus::pending("researcher");
            step.status = StepState::Complete;
            let mut status = RunStatus::queued(run.clone(), RunMode::Single, Some(4_242));
            status.session_id = Some(owner.clone());
            status.steps = vec![step];
            status.advance_state(RunState::Running).expect("running");
            status.advance_state(RunState::Complete).expect("complete");

            let paths = RunPaths::for_run(&self.async_root, &self.results_dir, run);
            tokio::fs::create_dir_all(&paths.run_dir)
                .await
                .expect("mkdir");
            crate::background::atomic::write_atomic_json(&paths.status, &status)
                .await
                .expect("write status");

            let file = ResultFile {
                id: run.clone(),
                run_id: run.clone(),
                agent: "researcher".to_string(),
                mode: RunMode::Single,
                state: RunState::Complete,
                success: true,
                cwd: PathBuf::from("/tmp"),
                session_file: None,
                session_id: Some(owner.clone()),
                completion_owner_id: None,
                results: vec![child_result("researcher", Some(output), 0)],
                workflow_children: None,
                workflow_receipt: None,
            };
            write_async_result_file(
                &ResultWrite {
                    results_dir: &self.results_dir,
                    session_id: owner,
                    run_id: run,
                    written_at: 1,
                    async_dir: Some(&paths.run_dir),
                    tool_call_id: None,
                },
                &serde_json::to_value(&file).expect("serializes"),
            )
            .await
            .expect("write payload");
        }
    }

    fn request(async_id: &str, child_id: Option<&str>) -> InspectRequest {
        InspectRequest {
            request_id: "req1".to_string(),
            async_id: async_id.to_string(),
            child_id: child_id.map(str::to_string),
            lines: None,
        }
    }

    /// The happy path, end to end — and the proof that the session gate is applied to the
    /// RECONCILED status rather than being folded into resolution.
    #[tokio::test]
    async fn a_run_owned_by_the_current_session_is_inspected() {
        let fixture = Fixture::new();
        let run = RunId::from_token("run0001");
        let owner = session("s1");
        fixture.seed(&run, &owner, "the answer").await;

        let reply =
            build_inspect_reply(&request("run0001", None), &fixture.deps(Some(&owner))).await;
        assert_eq!(reply.error, None, "{reply:?}");
        assert_eq!(reply.async_id.as_deref(), Some("run0001"));
        assert_eq!(reply.status, Some(RunState::Complete));
        // pi `:408`'s ladder ends at `node.status.runId` for the RUN form: `node.label` is set
        // only by `findChildNode`, and with no child there is no step to take an agent from.
        assert_eq!(reply.label.as_deref(), Some("run0001"));
        assert_eq!(reply.final_output.as_deref(), Some("the answer"));
        assert_eq!(reply.request_id, "req1");
        assert_eq!(reply.truncated, None);

        // The child form resolves by every identity spelling `child_identity` accepts.
        let reply = build_inspect_reply(
            &request("run0001", Some("step:0")),
            &fixture.deps(Some(&owner)),
        )
        .await;
        assert_eq!(reply.error, None, "{reply:?}");
        assert_eq!(reply.child_id.as_deref(), Some("step:0"));
        // …and WITH a child the ladder reaches the step's agent, one rung earlier.
        assert_eq!(reply.label.as_deref(), Some("researcher"));
        assert_eq!(reply.final_output.as_deref(), Some("the answer"));
    }

    /// SCOPE_12's partition clause AT THE RPC BOUNDARY.
    ///
    /// The code is pinned, not just the emptiness: `foreign_session` and NOT `not_found`. That
    /// second assertion is the whole test — [`resolve_async_run_id`] applies a session filter
    /// internally, so handing it the current session would report every foreign run as
    /// `not_found` and this surface would lose the ability to say WHICH thing went wrong.
    #[tokio::test]
    async fn inspect_of_a_foreign_sessions_run_returns_nothing() {
        let fixture = Fixture::new();
        let run = RunId::from_token("run0001");
        fixture.seed(&run, &session("s1"), "the answer").await;

        let intruder = session("s2");
        let reply =
            build_inspect_reply(&request("run0001", None), &fixture.deps(Some(&intruder))).await;
        assert_eq!(reply.error_code(), Some(InspectErrorCode::ForeignSession));
        assert_ne!(
            reply.error_code(),
            Some(InspectErrorCode::NotFound),
            "a foreign run must not be reported as a missing one"
        );
        assert_eq!(
            reply.error.as_ref().map(|error| error.message.as_str()),
            Some(FOREIGN_SESSION_MESSAGE)
        );
        assert_eq!(reply.final_output, None, "and no output leaked");
        assert_eq!(reply.messages, None);

        // A run id that genuinely does not exist IS `not_found`, so the two codes are live.
        let reply =
            build_inspect_reply(&request("nosuchrun", None), &fixture.deps(Some(&intruder))).await;
        assert_eq!(reply.error_code(), Some(InspectErrorCode::NotFound));
    }

    /// The four refusals that precede any artifact read, each with upstream's own sentence.
    #[tokio::test]
    async fn the_pre_read_refusals_carry_upstreams_sentences() {
        let fixture = Fixture::new();

        let reply = build_inspect_reply(&request("run0001", None), &fixture.deps(None)).await;
        assert_eq!(reply.error_code(), Some(InspectErrorCode::NoActiveSession));
        assert_eq!(reply.request_id, "req1", "still correlated");
        assert_eq!(reply.async_id.as_deref(), Some("run0001"));

        let owner = session("s1");
        let reply =
            build_inspect_reply(&request("../escape", None), &fixture.deps(Some(&owner))).await;
        assert_eq!(reply.error_code(), Some(InspectErrorCode::InvalidRequest));
        assert_eq!(
            reply.error.as_ref().map(|error| error.message.as_str()),
            Some("The requested async run could not be resolved.")
        );

        let reply =
            build_inspect_reply(&request("run0001", None), &fixture.deps(Some(&owner))).await;
        assert_eq!(reply.error_code(), Some(InspectErrorCode::NotFound));
        assert_eq!(
            reply.error.as_ref().map(|error| error.message.as_str()),
            Some("Async run 'run0001' was not found.")
        );

        // A child that is not there — `child_identity`'s sentence, which IS upstream's.
        let run = RunId::from_token("run0002");
        fixture.seed(&run, &owner, "x").await;
        let reply = build_inspect_reply(
            &request("run0002", Some("step:7")),
            &fixture.deps(Some(&owner)),
        )
        .await;
        assert_eq!(reply.error_code(), Some(InspectErrorCode::NotFound));
        assert_eq!(
            reply.error.as_ref().map(|error| error.message.as_str()),
            Some("Child 'step:7' was not found under async run 'run0002'.")
        );
    }

    /// An error reply is always correlated, and an UNUSABLE correlation token becomes the literal
    /// `invalid` rather than being echoed — the property the widget channel depends on.
    #[tokio::test]
    async fn the_slash_entry_point_always_correlates() {
        let fixture = Fixture::new();
        let owner = session("s1");
        let deps = fixture.deps(Some(&owner));

        let reply = handle_inspect_rpc_args("req-9 --verbose", &deps).await;
        assert_eq!(reply.request_id, "req-9", "the first token is echoed");
        assert_eq!(reply.error_code(), Some(InspectErrorCode::InvalidRequest));
        assert_eq!(
            reply.error.as_ref().map(|error| error.message.as_str()),
            Some("Unknown flag: --verbose. Supported: --lines N.")
        );

        // A first token that is NOT a well-formed correlation token becomes the literal `invalid`
        // rather than being echoed — `.` is outside `[A-Za-z0-9_-]`.
        let reply = handle_inspect_rpc_args("bad.token run0001", &deps).await;
        assert_eq!(reply.request_id, "invalid");
        assert_eq!(
            reply.error.as_ref().map(|error| error.message.as_str()),
            Some("requestId must match [A-Za-z0-9_-]{1,64}.")
        );

        // And the envelope is exactly one line, prefix included — a multi-line reply would be
        // unreadable to a host that splits the stream on newlines.
        let encoded = encode_inspect_reply(&reply);
        assert_eq!(encoded.len(), 1);
        assert!(encoded[0].starts_with(INSPECT_WIDGET_PREFIX), "{encoded:?}");
        assert!(!encoded[0].trim_end().contains('\n'), "{encoded:?}");
        let round_tripped: InspectReply = serde_json::from_str(
            encoded[0]
                .strip_prefix(INSPECT_WIDGET_PREFIX)
                .expect("prefix"),
        )
        .expect("round-trips");
        assert_eq!(round_tripped, reply);
    }

    /// The byte budget sheds in upstream's order — oldest messages first, then the final output —
    /// and the envelope survives whatever it has to drop.
    #[test]
    fn the_byte_budget_sheds_messages_then_final_output() {
        let message = |index: usize| InspectReplyMessage {
            role: "assistant".to_string(),
            kind: SessionMessageKind::Text,
            text: format!("{index}:{}", "x".repeat(900)),
            name: None,
            is_error: None,
        };
        let reply = InspectReply {
            kind: InspectReplyKind,
            version: InspectReplyVersion,
            request_id: "req1".to_string(),
            async_id: Some("run0001".to_string()),
            child_id: None,
            status: Some(RunState::Complete),
            label: None,
            task: None,
            messages: Some((0..200).map(message).collect()),
            final_output: Some("y".repeat(MAX_FINAL_OUTPUT_LENGTH)),
            truncated: None,
            error: None,
        };
        assert!(
            serialized_bytes(&reply) > MAX_SERIALIZED_BYTES,
            "precondition"
        );

        let bounded = enforce_byte_budget(reply);
        assert!(
            serialized_bytes(&bounded) <= MAX_SERIALIZED_BYTES,
            "{} bytes",
            serialized_bytes(&bounded)
        );
        let truncated = bounded.truncated.expect("reported");
        assert!(truncated.messages > 0, "{truncated:?}");
        let kept = bounded.messages.as_ref().expect("some survive");
        assert!(!kept.is_empty(), "the newest messages survive");
        assert!(
            kept[0]
                .text
                .starts_with(&format!("{}:", truncated.messages)),
            "the OLDEST were dropped, not the newest: {}",
            kept[0].text
        );
        // The envelope is intact whatever was shed.
        assert_eq!(bounded.request_id, "req1");
        assert_eq!(bounded.async_id.as_deref(), Some("run0001"));
    }

    /// `boundContent` keeps newlines (the reason it is not `publicText`) and normalizes exactly the
    /// two separators `JSON.stringify` leaves raw.
    #[test]
    fn bound_content_keeps_newlines_and_normalizes_the_two_separators() {
        assert_eq!(bound_content("a\nb", 100), "a\nb");
        assert_eq!(bound_content("a\u{2028}b\u{2029}c", 100), "a\nb\nc");
        assert_eq!(bound_content("abcdef", 3), "abc");

        // `public_text`, by contrast, FLATTENS — that is what makes it safe for a one-line label.
        assert_eq!(public_text("a\nb", 100).as_deref(), Some("a b"));
        assert_eq!(public_text("   ", 100), None, "sanitizes to nothing");
        assert_eq!(public_text("abcdef", 3).as_deref(), Some("abc"));
    }
}
