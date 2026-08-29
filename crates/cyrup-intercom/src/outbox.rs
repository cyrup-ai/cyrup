//! The extension outbox (`v0.12.0 extension-api.ts`, driven from `v0.12.0 index.ts:1047-1183`) —
//! the surface through which an extension OTHER than the agent sends an intercom message, and the
//! ten-code result contract it switches on.
//!
//! An extension emits [`crate::outbox::INTERCOM_OUTBOX_REQUEST_EVENT`] on the inter-extension bus
//! and receives exactly one [`crate::outbox::INTERCOM_OUTBOX_RESULT_EVENT`] back per `requestId`.
//! The `code` on that result is the contract: an extension switches on it to decide whether to
//! retry, re-prompt, or give up, so every variant is reachable from its own condition and none is
//! a catch-all.

use std::sync::Arc;

use serde_json::json;

use crate::session_state::SharedIntercomState;
use crate::transport::client::SendOptions;
use crate::transport::protocol::{MessageProvenance, ProvenanceKind, SessionInfo, now_ms};

/// `INTERCOM_EXTENSION_REGISTER_EVENT` (`v0.12.0 extension-api.ts:3`).
pub const INTERCOM_EXTENSION_REGISTER_EVENT: &str = "intercom:extension-register";
/// `INTERCOM_EXTENSION_REGISTRY_READY_EVENT` (`v0.12.0 extension-api.ts:4`).
pub const INTERCOM_EXTENSION_REGISTRY_READY_EVENT: &str = "intercom:extension-registry-ready";
/// `INTERCOM_OUTBOX_REQUEST_EVENT` (`v0.12.0 extension-api.ts:5`).
pub const INTERCOM_OUTBOX_REQUEST_EVENT: &str = "intercom:outbox-request";
/// `INTERCOM_OUTBOX_RESULT_EVENT` (`v0.12.0 extension-api.ts:6`).
pub const INTERCOM_OUTBOX_RESULT_EVENT: &str = "intercom:outbox-result";

/// `IntercomOutboxResultStatus` (`v0.12.0 extension-api.ts:8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxResultStatus {
    /// The broker confirmed delivery.
    Sent,
    /// The request was well-formed but refused (bad payload, duplicate, user declined).
    Rejected,
    /// Policy stopped it before it could be attempted (confirmation, addressing).
    Blocked,
    /// It was attempted and the session or the transport failed under it.
    Failed,
}

/// `IntercomOutboxResultCode` (`v0.12.0 extension-api.ts:10-20`) — the CONTRACT. Every variant is
/// reachable from its own condition; none is a catch-all, and no condition produces a bare `failed`
/// with no code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxResultCode {
    /// The human declined the `confirmSend` dialog.
    UserCancelled,
    /// `confirmSend` is on but this session cannot ask (no UI, or no bound host services).
    ConfirmationUnavailable,
    /// No live intercom runtime, or the connect / roster lookup failed.
    SessionUnavailable,
    /// The runtime went away mid-flight (generation bumped, shutdown, client replaced).
    SessionEnded,
    /// The payload failed [`parse_outbox_request`].
    InvalidRequest,
    /// This `requestId` was already handled in this runtime.
    DuplicateRequest,
    /// `to` matched no connected session.
    TargetNotFound,
    /// `to` matched more than one connected session.
    TargetAmbiguous,
    /// `to` resolved to this very session.
    SelfTarget,
    /// The broker accepted the send and reported `delivered: false`.
    DeliveryFailed,
}

/// `IntercomOutboxRequestV1` (`v0.12.0 extension-api.ts:22-29`). EVERY field is required and
/// `version` must be exactly `1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntercomOutboxRequestV1 {
    /// Caller-minted correlation id; also becomes the delivered `Message.id` (`index.ts:1150`).
    pub request_id: String,
    /// The originating extension's id.
    pub extension_id: String,
    /// The originating extension's display name (rendered on the recipient's card).
    pub extension_name: String,
    /// Session name or id to deliver to, stored TRIMMED (`index.ts:502`).
    pub to: String,
    /// The message body, stored VERBATIM (`index.ts:503`).
    pub message: String,
}

/// `OutboxRequestTrace` (`v0.12.0 index.ts:78-94`) — what a rejection can still echo back when the
/// payload only parsed far enough to carry it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutboxRequestTrace {
    /// The only field a trace cannot exist without.
    pub request_id: String,
    /// Echoed onto the result when recoverable.
    pub extension_id: Option<String>,
    /// Echoed onto the result when recoverable.
    pub extension_name: Option<String>,
    /// Carried for the `intercom_outbox_result` audit entry.
    pub to: Option<String>,
    /// Carried for the `intercom_outbox_result` audit entry.
    pub message: Option<String>,
}

/// `PendingOutboxRequest` (`v0.12.0 index.ts:94`) — an in-flight request plus the generation it
/// started under, so a runtime change settles exactly the ones it orphaned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingOutboxRequest {
    /// The [`crate::connect`] generation live when the request was accepted.
    pub generation: u64,
    /// What the result can echo.
    pub request: OutboxRequestTrace,
}

/// The outcome of [`parse_outbox_request`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedOutboxRequest {
    /// A fully valid V1 request.
    Ok(Box<IntercomOutboxRequestV1>),
    /// Rejected, with whatever could still be recovered for correlation. A `None` trace means not
    /// even a `requestId` was recoverable, and the caller must emit NOTHING (`index.ts:1050-1059`).
    Invalid {
        /// What could be echoed back, if anything.
        trace: Option<OutboxRequestTrace>,
        /// Human-readable reason.
        detail: String,
    },
}

/// A resolved delivery target (`OutboxTarget`, `v0.12.0 index.ts:78`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxTarget {
    /// The peer's session id.
    pub id: String,
    /// Its display name, falling back to the id (`index.ts:1045`).
    pub label: String,
}

/// Read a required non-blank string field. Upstream tests `typeof x !== "string" || x.trim() === ""`
/// (`v0.12.0 index.ts:487-499`), which `serde` alone would not reject.
fn non_blank(value: &serde_json::Value, key: &str) -> Option<String> {
    let s = value.get(key)?.as_str()?;
    if s.trim().is_empty() { None } else { Some(s.to_string()) }
}

/// `parseOutboxRequestPayload` (`v0.12.0 index.ts:471-507`), statement for statement.
///
/// Hand-written against [`serde_json::Value`] rather than `serde_json::from_value` for two reasons
/// upstream depends on: a rejection must still echo the partially-recovered
/// `requestId`/`extensionId`/`extensionName` so it can be correlated (`:475-477`), and a
/// blank-after-trim string must be invalid where serde would happily accept it.
#[must_use]
pub fn parse_outbox_request(payload: &serde_json::Value) -> ParsedOutboxRequest {
    let Some(obj) = payload.as_object() else {
        return ParsedOutboxRequest::Invalid {
            trace: None,
            detail: "Outbox request payload is not an object".to_string(),
        };
    };
    // The trace is recovered FIRST and independently of validity, so a rejection can still be
    // correlated by the extension that sent it (`index.ts:475-477`).
    let request_id = obj.get("requestId").and_then(|v| v.as_str()).map(str::to_string);
    let trace = request_id.as_ref().map(|id| OutboxRequestTrace {
        request_id: id.clone(),
        extension_id: obj.get("extensionId").and_then(|v| v.as_str()).map(str::to_string),
        extension_name: obj.get("extensionName").and_then(|v| v.as_str()).map(str::to_string),
        to: obj.get("to").and_then(|v| v.as_str()).map(str::to_string),
        message: obj.get("message").and_then(|v| v.as_str()).map(str::to_string),
    });
    let invalid = |detail: &str| ParsedOutboxRequest::Invalid {
        trace: trace.clone(),
        detail: detail.to_string(),
    };

    if payload.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return invalid("Outbox request version must be 1");
    }
    let Some(request_id) = request_id.filter(|s| !s.trim().is_empty()) else {
        return invalid("Outbox request requestId must be a non-empty string");
    };
    let Some(extension_id) = non_blank(payload, "extensionId") else {
        return invalid("Outbox request extensionId must be a non-empty string");
    };
    let Some(extension_name) = non_blank(payload, "extensionName") else {
        return invalid("Outbox request extensionName must be a non-empty string");
    };
    let Some(to) = non_blank(payload, "to") else {
        return invalid("Outbox request to must be a non-empty string");
    };
    let Some(message) = non_blank(payload, "message") else {
        return invalid("Outbox request message must be a non-empty string");
    };

    ParsedOutboxRequest::Ok(Box::new(IntercomOutboxRequestV1 {
        request_id,
        extension_id,
        extension_name,
        // `to` is trimmed, `message` is verbatim (`v0.12.0 index.ts:502-503`).
        to: to.trim().to_string(),
        message,
    }))
}

/// `resolveOutboxTarget` (`v0.12.0 index.ts:1029-1046`).
///
/// Built on [`crate::broker::routing::find_session_ids`], which already implements upstream's exact
/// precedence — exact id, then case-insensitive exact name (possibly many), then id prefix. It
/// deliberately does NOT reuse [`SharedIntercomState::resolve_target`], which collapses both
/// ambiguity classes into one error and so cannot distinguish `target_not_found` from
/// `target_ambiguous`; upstream made the same split for the same reason.
///
/// # Errors
///
/// Returns the `target_not_found` / `target_ambiguous` code and upstream's detail string verbatim.
pub fn resolve_outbox_target(
    sessions: &[SessionInfo],
    to: &str,
) -> Result<OutboxTarget, (OutboxResultCode, String)> {
    let entries: Vec<(String, Option<String>)> =
        sessions.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
    let matches = crate::broker::routing::find_session_ids(&entries, to);
    if matches.is_empty() {
        return Err((
            OutboxResultCode::TargetNotFound,
            format!("Session \"{to}\" is not currently connected."),
        ));
    }
    if matches.len() > 1 {
        return Err((
            OutboxResultCode::TargetAmbiguous,
            format!("Multiple sessions match \"{to}\"."),
        ));
    }
    let Some(id) = matches.into_iter().next() else {
        // Unreachable: the emptiness case returned above. Handled without indexing because the
        // workspace denies `indexing_slicing`, and a panic here would take down the fan-out.
        return Err((
            OutboxResultCode::TargetNotFound,
            format!("Session \"{to}\" is not currently connected."),
        ));
    };
    let label = sessions
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| id.clone());
    Ok(OutboxTarget { id, label })
}

/// `buildOutboxResult` (`v0.12.0 index.ts:984-998`) — the conditional-spread result builder.
///
/// Every optional key is OMITTED when absent rather than emitted as `null`; upstream builds this
/// with object spreads, so a `null` here would be a wire change.
fn build_outbox_result(
    request_id: &str,
    status: OutboxResultStatus,
    code: Option<OutboxResultCode>,
    trace: Option<&OutboxRequestTrace>,
    message_id: Option<&str>,
    detail: Option<&str>,
) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    out.insert("version".to_string(), json!(1));
    out.insert("requestId".to_string(), json!(request_id));
    out.insert(
        "status".to_string(),
        serde_json::to_value(status).unwrap_or_else(|_| json!("failed")),
    );
    if let Some(code) = code
        && let Ok(v) = serde_json::to_value(code)
    {
        out.insert("code".to_string(), v);
    }
    if let Some(id) = trace.and_then(|t| t.extension_id.as_deref()) {
        out.insert("extensionId".to_string(), json!(id));
    }
    if let Some(name) = trace.and_then(|t| t.extension_name.as_deref()) {
        out.insert("extensionName".to_string(), json!(name));
    }
    if let Some(mid) = message_id {
        out.insert("messageId".to_string(), json!(mid));
    }
    if let Some(detail) = detail {
        out.insert("detail".to_string(), json!(detail));
    }
    serde_json::Value::Object(out)
}

/// `emitOutboxResult` (`v0.12.0 index.ts:1000-1008`) — append the audit entry FIRST, then emit the
/// bus event, in that order.
fn emit_outbox_result(
    state: &SharedIntercomState,
    result: &serde_json::Value,
    trace: Option<&OutboxRequestTrace>,
) {
    let Some(services) = state.host_services() else { return };
    let mut entry = result.clone();
    if let Some(obj) = entry.as_object_mut() {
        if let Some(to) = trace.and_then(|t| t.to.as_deref()) {
            obj.insert("to".to_string(), json!(to));
        }
        if let Some(text) = trace.and_then(|t| t.message.as_deref()) {
            obj.insert("message".to_string(), json!({ "text": text }));
        }
        obj.insert("timestamp".to_string(), json!(now_ms()));
    }
    if let Err(e) = services.append_entry("intercom_outbox_result", &entry) {
        tracing::warn!(error = %e, kind = "intercom_outbox_result", "append_entry failed");
    }
    services.emit_event(INTERCOM_OUTBOX_RESULT_EVENT, result);
}

/// `settleOutboxRequest` (`v0.12.0 index.ts:1009-1021`) — pop the pending request and emit exactly
/// one result for it. A second call for the same `requestId` finds nothing pending and emits
/// NOTHING, which is what makes the contract exactly-once.
pub fn settle_outbox_request(
    state: &SharedIntercomState,
    request_id: &str,
    status: OutboxResultStatus,
    code: Option<OutboxResultCode>,
    message_id: Option<&str>,
    detail: Option<&str>,
) -> bool {
    let Some(pending) = state.take_pending_outbox(request_id) else {
        return false;
    };
    let result = build_outbox_result(
        request_id,
        status,
        code,
        Some(&pending.request),
        message_id,
        detail,
    );
    emit_outbox_result(state, &result, Some(&pending.request));
    true
}

/// `failPendingOutboxRequests(generation, code, detail)` (`v0.12.0 index.ts:1022-1028`) — settle
/// every request orphaned by a runtime change, leaving any started under `generation` alone.
pub fn fail_pending_outbox_requests(
    state: &SharedIntercomState,
    generation: u64,
    code: OutboxResultCode,
    detail: &str,
) {
    for pending in state.drain_pending_outbox_upto(generation) {
        let result = build_outbox_result(
            &pending.request.request_id,
            OutboxResultStatus::Failed,
            Some(code),
            Some(&pending.request),
            None,
            Some(detail),
        );
        emit_outbox_result(state, &result, Some(&pending.request));
    }
}

/// Emit a result for a request that was never tracked (a parse or dedupe rejection). Upstream emits
/// these inline in the synchronous prelude, before anything is added to `pendingOutboxRequests`
/// (`v0.12.0 index.ts:1050-1070`).
fn emit_untracked_result(
    state: &SharedIntercomState,
    request_id: &str,
    status: OutboxResultStatus,
    code: OutboxResultCode,
    trace: Option<&OutboxRequestTrace>,
    detail: &str,
) {
    let result = build_outbox_result(request_id, status, Some(code), trace, None, Some(detail));
    emit_outbox_result(state, &result, trace);
}

/// `handleOutboxRequest` (`v0.12.0 index.ts:1047-1183`).
///
/// The synchronous prelude — parse, dedupe, track — runs INLINE so `invalid_request` and
/// `duplicate_request` are ordered against the emit exactly as upstream orders them; the delivery
/// leg is spawned, matching upstream's `void (async () => …)()`.
pub fn handle_outbox_request(state: Arc<SharedIntercomState>, payload: serde_json::Value) {
    let parsed = parse_outbox_request(&payload);
    let request = match parsed {
        ParsedOutboxRequest::Invalid { trace, detail } => {
            // No recoverable requestId ⇒ emit NOTHING: an uncorrelatable result is noise on the bus
            // (`v0.12.0 index.ts:1050-1059`).
            if let Some(trace) = trace {
                emit_untracked_result(
                    &state,
                    &trace.request_id.clone(),
                    OutboxResultStatus::Rejected,
                    OutboxResultCode::InvalidRequest,
                    Some(&trace),
                    &detail,
                );
            }
            return;
        }
        ParsedOutboxRequest::Ok(request) => *request,
    };

    let trace = OutboxRequestTrace {
        request_id: request.request_id.clone(),
        extension_id: Some(request.extension_id.clone()),
        extension_name: Some(request.extension_name.clone()),
        to: Some(request.to.clone()),
        message: Some(request.message.clone()),
    };

    // `outboxRequestIds` test-and-insert (`index.ts:1063`).
    if state.outbox_request_seen(&request.request_id) {
        emit_untracked_result(
            &state,
            &request.request_id,
            OutboxResultStatus::Rejected,
            OutboxResultCode::DuplicateRequest,
            Some(&trace),
            "Outbox request has already been handled",
        );
        return;
    }

    let generation = state.connect.generation();
    state.track_pending_outbox(
        request.request_id.clone(),
        PendingOutboxRequest { generation, request: trace },
    );

    tokio::spawn(async move {
        deliver_outbox_request(state, request, generation).await;
    });
}

/// The async delivery leg (`v0.12.0 index.ts:1078-1183`), fenced on the generation at six points.
async fn deliver_outbox_request(
    state: Arc<SharedIntercomState>,
    request: IntercomOutboxRequestV1,
    generation: u64,
) {
    let rid = request.request_id.as_str();
    let settle = |status, code, message_id: Option<&str>, detail: &str| {
        settle_outbox_request(&state, rid, status, Some(code), message_id, Some(detail));
    };

    // 1. Live at entry? Before the connection exists this is `session_unavailable` (`:1079`).
    if !crate::connect::is_live_at(&state, generation) {
        settle(
            OutboxResultStatus::Failed,
            OutboxResultCode::SessionUnavailable,
            None,
            "Intercom session is not active",
        );
        return;
    }

    // 2. The confirm gate's UNAVAILABLE half (`:1083-1086`). `HostServices::confirm` returns a bare
    //    bool and so cannot fail; the two ways confirmation can be unavailable here are no UI and no
    //    bound host services, which is the same class of loss upstream's "confirm threw" branch has.
    let services = state.host_services();
    if state.config.confirm_send && (!state.has_ui() || services.is_none()) {
        settle(
            OutboxResultStatus::Blocked,
            OutboxResultCode::ConfirmationUnavailable,
            None,
            "confirmSend is enabled but no UI is available",
        );
        return;
    }

    // 3. Connect. `Background` is the reason that re-arms the reconnect ladder on failure, which is
    //    what upstream's outbox uses (`ensureConnected("background")`, `:1088`).
    let Ok(client) = crate::connect::ensure_connected(&state, crate::connect::ConnectReason::Background).await
    else {
        settle(
            OutboxResultStatus::Failed,
            OutboxResultCode::SessionUnavailable,
            None,
            "Intercom session is not active",
        );
        return;
    };

    // 4. Every checkpoint past the connection is `session_ended`, never `session_unavailable`.
    if !crate::connect::is_live_at(&state, generation) {
        settle(
            OutboxResultStatus::Failed,
            OutboxResultCode::SessionEnded,
            None,
            "Session ended before target resolution",
        );
        return;
    }
    if client.session_id().is_none() {
        settle(
            OutboxResultStatus::Failed,
            OutboxResultCode::SessionUnavailable,
            None,
            "Intercom session is not active",
        );
        return;
    }

    let Ok(sessions) = client.list_sessions().await else {
        settle(
            OutboxResultStatus::Failed,
            OutboxResultCode::SessionUnavailable,
            None,
            "Intercom session is not active",
        );
        return;
    };

    // 5. Addressing. All three of these are `blocked`, not `failed` — the request was well-formed
    //    and the session healthy; it is the ADDRESSING that was refused, and an extension retries
    //    that differently from a transport failure.
    let target = match resolve_outbox_target(&sessions, &request.to) {
        Ok(target) => target,
        Err((code, detail)) => {
            settle(OutboxResultStatus::Blocked, code, None, &detail);
            return;
        }
    };
    if state.current_session_target_matches(&request.to, Some(&target.id)) {
        settle(
            OutboxResultStatus::Blocked,
            OutboxResultCode::SelfTarget,
            None,
            "Cannot message the current session.",
        );
        return;
    }

    // 6. The confirm gate's DECLINE half (`:1120-1131`).
    if state.config.confirm_send
        && let Some(services) = services.as_ref()
    {
        let prompt = format!("Send message to {}?", target.label);
        if !services.confirm("Send Message", &prompt, &cyrup_ext::DialogOptions::default()) {
            settle(
                OutboxResultStatus::Rejected,
                OutboxResultCode::UserCancelled,
                None,
                "Message cancelled by user",
            );
            return;
        }
    }

    // 7. The delivery fence (`:1145-1149`): still live, still THIS client, still connected.
    let same_client = state.client().is_some_and(|c| Arc::ptr_eq(&c, &client));
    if !crate::connect::is_live_at(&state, generation) || !same_client || !client.is_connected() {
        settle(
            OutboxResultStatus::Failed,
            OutboxResultCode::SessionEnded,
            None,
            "Session ended before delivery",
        );
        return;
    }

    // The outbox message id IS the requestId (`:1150`) — that is what makes the emitted `messageId`
    // correlatable, and a broker-level replay idempotent at the receiver's own dedupe.
    let send = client
        .send(
            &target.id,
            SendOptions {
                text: request.message.clone(),
                message_id: Some(request.request_id.clone()),
                provenance: Some(MessageProvenance {
                    kind: ProvenanceKind::ExtensionOutbox,
                    extension_id: request.extension_id.clone(),
                    extension_name: request.extension_name.clone(),
                    request_id: request.request_id.clone(),
                    extra: crate::transport::protocol::UnknownFields::default(),
                }),
                ..Default::default()
            },
        )
        .await;

    let result = match send {
        Ok(result) => result,
        Err(e) => {
            // The only place upstream CHOOSES between two codes on one failure (`:1177-1182`).
            let code = if crate::connect::is_live_at(&state, generation) {
                OutboxResultCode::SessionUnavailable
            } else {
                OutboxResultCode::SessionEnded
            };
            settle(OutboxResultStatus::Failed, code, None, &e.to_string());
            return;
        }
    };

    if !result.delivered {
        let detail = result.reason.clone().unwrap_or_else(|| "Delivery failed".to_string());
        // `SendResult.id` carries the broker's own id when a `DeliveryFailed` frame answered the
        // send (`transport/client.rs:871-878`), but it is EMPTY when the client tore down with the
        // send still in flight (`:189`, `:697`) — that teardown has no message id in scope and
        // synthesizes one. Emitting `"messageId": ""` would put a meaningless value on the field an
        // extension correlates results on, so fall back to the id we actually attempted: the
        // requestId, which `handle_outbox_request` passed as `message_id`.
        let attempted = if result.id.is_empty() { rid } else { result.id.as_str() };
        settle(
            OutboxResultStatus::Failed,
            OutboxResultCode::DeliveryFailed,
            Some(attempted),
            &detail,
        );
        return;
    }

    // The outbox's own audit entry (`:1170-1176`). Unlike the agent tool's it carries no
    // attachments and no replyTo, and it adds an `extension` key the agent's send never writes.
    if let Some(services) = services.as_ref()
        && let Err(e) = services.append_entry(
            "intercom_sent",
            &json!({
                "to": target.label,
                "message": { "text": request.message },
                "messageId": result.id,
                "timestamp": now_ms(),
                "extension": {
                    "id": request.extension_id,
                    "name": request.extension_name,
                    "requestId": request.request_id,
                },
            }),
        )
    {
        tracing::warn!(error = %e, kind = "intercom_sent", "append_entry failed");
    }

    settle_outbox_request(
        &state,
        rid,
        OutboxResultStatus::Sent,
        None,
        Some(&result.id),
        None,
    );
}

/// The `intercom:extension-register` front door (`v0.12.0 index.ts:1687-1698`).
///
/// This lands the shape check, the namespace rule and the already-registered refusal, and records
/// the `(namespace, owner_eligible)` pair so `currentExtensionCapabilities` has a source. The
/// channel effects behind `registration.onEvent` / `onReady` — owner election, publish fan-out, the
/// state store — are ICOM-016 and are deliberately NOT stubbed here; the broker still refuses them
/// honestly and never advertises the extension-bus feature.
pub fn handle_extension_register(state: &SharedIntercomState, payload: &serde_json::Value) {
    let Some(namespace) = payload.get("namespace").and_then(|v| v.as_str()) else {
        return;
    };
    // `^[a-z0-9][a-z0-9._/-]{0,63}$` (`v0.12.0 index.ts:863-868`), spelled without a regex crate.
    let mut chars = namespace.chars();
    let head_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let tail_ok = namespace.len() <= 64
        && namespace
            .chars()
            .skip(1)
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '-'));
    if !head_ok || !tail_ok {
        return;
    }
    let owner_eligible = payload
        .get("ownerEligible")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    // A repeat registration of the same namespace is refused, matching upstream's
    // already-registered branch (`v0.12.0 index.ts:863-868`). Nothing to emit either way: the
    // register topic is fire-and-forget, and the channel effects are ICOM-016.
    let _registered = state.record_extension_registration(namespace, owner_eligible);
}
