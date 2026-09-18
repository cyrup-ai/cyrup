//! PB-8 — the RPC wire envelopes and their parser: pi `src/extension/rpc.ts:37-63` (the two
//! envelope types), `:77-85` (`SubagentRpcErrorCode`), `:322-330` (`SubagentRpcError`),
//! `:342-347` (`assertRequestId`), `:771-787` (`parseRequest`), `:789-795`
//! (`safeReplyRequestId`) and `:797-815` (`errorReply`), all @v0.68.0.
//!
//! # The three parse checks are ORDERED, and the order is the whole error taxonomy
//!
//! `parseRequest` (`:771-779`) validates the `requestId` FIRST (`:773`), the protocol version
//! SECOND (`:774-776`) and the method THIRD (`:777-779`). That order is not stylistic: the reply
//! TOPIC is built out of the `requestId`, so a `requestId` carrying a `\r`/`\n` could forge a
//! second reply topic and answer a request the caller never made. It is rejected before anything
//! else is even looked at.

use serde_json::{Map, Value};

use super::{SUBAGENT_RPC_PROTOCOL_VERSION, SubagentRpcMethod, subagent_rpc_reply_event};

/// pi `SubagentRpcErrorCode` (`rpc.ts:77-85`).
///
/// # `[CYRUP-DELTA, mechanism]` — `not_found` and `invalid_state` are NOT emitted, and are absent
///
/// Upstream's eight codes are `invalid_request`, `invalid_params`, `unsupported_version`,
/// `unsupported_method`, `no_active_session`, `execution_failed`, `not_found` and
/// `invalid_state`. cyrup emits the first six.
///
/// The last two are producible upstream only because `stopAsyncRun` (`rpc.ts:561-701`)
/// re-implements the whole stop path INLINE and is therefore in a position to classify its own
/// refusals. cyrup does not re-implement it: that body is already ported one level down behind
/// `route_control_action`'s `"stop"` arm (`extension/tool/routing.rs:1804-1809` →
/// [`crate::extension::SubagentExecutor::control_stop`]), which answers with untyped
/// `Err(ToolError)` strings carrying upstream's own sentences. Every tool `Err` therefore maps to
/// [`Self::ExecutionFailed`] with that sentence verbatim, and the bridge deliberately does NOT
/// string-match the message to re-derive a code — that would be a second copy of a classification
/// that already exists in prose, free to drift from it on the next copy-edit.
///
/// The two variants are left OUT of this enum rather than carried as unreachable members: a
/// variant nothing can construct is a promise to a client that this surface will one day answer
/// with it, and the honest statement is this paragraph. A client written against upstream's
/// vocabulary must treat `execution_failed` as covering both, and the message it carries is the
/// same sentence upstream would have attached to the narrower code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubagentRpcErrorCode {
    /// The request was not an object, or its `requestId` was unusable.
    InvalidRequest,
    /// A normalizer refused the `params` (`rpc.ts:349-353,486-559`).
    InvalidParams,
    /// `version !== 1` (`rpc.ts:774-776`).
    UnsupportedVersion,
    /// `method` absent or not one of the eight (`rpc.ts:777-779`).
    UnsupportedMethod,
    /// No live capability backend (upstream's null `ExtensionContext`, `rpc.ts:710`).
    NoActiveSession,
    /// The dispatch itself failed (`rpc.ts:380-383`), or a refusal upstream would have classified
    /// as `not_found`/`invalid_state` — see this type's own doc.
    ExecutionFailed,
}

impl SubagentRpcErrorCode {
    /// The exact wire spelling (`rpc.ts:77-85`).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidParams => "invalid_params",
            Self::UnsupportedVersion => "unsupported_version",
            Self::UnsupportedMethod => "unsupported_method",
            Self::NoActiveSession => "no_active_session",
            Self::ExecutionFailed => "execution_failed",
        }
    }
}

/// pi `class SubagentRpcError` (`rpc.ts:322-330`) — a code plus the sentence the caller reads.
#[derive(Clone, Debug)]
pub(crate) struct SubagentRpcError {
    pub(crate) code: SubagentRpcErrorCode,
    pub(crate) message: String,
}

impl SubagentRpcError {
    pub(crate) fn new(code: SubagentRpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// `throw new SubagentRpcError("invalid_params", …)` — the normalizers' single constructor.
    pub(crate) fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(SubagentRpcErrorCode::InvalidParams, message)
    }
}

/// pi `SubagentRpcRequestEnvelope` (`rpc.ts:37-46`), after `parseRequest` has validated it.
#[derive(Clone, Debug)]
pub(crate) struct SubagentRpcRequest {
    pub(crate) request_id: String,
    pub(crate) method: SubagentRpcMethod,
    pub(crate) params: Option<Value>,
    /// pi `source?: { extension?: string; … }` (`rpc.ts:42-45`) — who asked. Upstream carries it
    /// on the envelope and never reads it; cyrup traces it, so an operator can tell which sibling
    /// extension drove a given dispatch without the field becoming dead weight.
    pub(crate) source: Option<Value>,
}

/// pi `isRecord` (`rpc.ts:338-340`). A JSON array is an object in JS and is excluded there; in
/// `serde_json` an array is not a `Value::Object` at all, so `as_object` is the whole test.
fn as_record(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// pi `assertRequestId` (`rpc.ts:342-347`): a non-empty-after-trim string with NO `\r` or `\n`.
///
/// The newline rule is a SECURITY property, not tidiness: [`subagent_rpc_reply_event`] pastes this
/// value straight into a bus topic, and cyrup's [`crate::extension::rpc`] module doc explains that
/// a client subscribes to that exact topic before emitting. A newline would let a caller name a
/// topic it was never given.
fn assert_request_id(value: Option<&Value>) -> Result<String, SubagentRpcError> {
    match value.and_then(Value::as_str) {
        Some(id) if !id.trim().is_empty() && !id.contains(['\r', '\n']) => Ok(id.to_string()),
        _ => Err(SubagentRpcError::new(
            SubagentRpcErrorCode::InvalidRequest,
            "RPC requestId must be a non-empty string without newlines.",
        )),
    }
}

/// pi `safeReplyRequestId` (`rpc.ts:789-795`) — the SAME three tests as [`assert_request_id`], but
/// answering with the literal `"unknown"` instead of throwing, so a malformed request still gets a
/// reply envelope rather than silence.
pub(crate) fn safe_reply_request_id(raw: &Value) -> String {
    as_record(raw)
        .and_then(|record| record.get("requestId"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && !id.contains(['\r', '\n']))
        .map_or_else(|| "unknown".to_string(), str::to_string)
}

/// pi `parseRequest` (`rpc.ts:771-787`). See this module's doc for why the three checks are
/// ordered the way they are.
pub(crate) fn parse_request(raw: &Value) -> Result<SubagentRpcRequest, SubagentRpcError> {
    // `:772`.
    let Some(record) = as_record(raw) else {
        return Err(SubagentRpcError::new(
            SubagentRpcErrorCode::InvalidRequest,
            "Subagent RPC request must be an object.",
        ));
    };
    // `:773` — FIRST.
    let request_id = assert_request_id(record.get("requestId"))?;
    // `:774-776`.
    let version = record.get("version");
    if version.and_then(Value::as_u64) != Some(u64::from(SUBAGENT_RPC_PROTOCOL_VERSION)) {
        return Err(SubagentRpcError::new(
            SubagentRpcErrorCode::UnsupportedVersion,
            format!(
                "Unsupported subagent RPC version: {}.",
                version.map_or_else(|| "undefined".to_string(), ToString::to_string)
            ),
        ));
    }
    // `:777-779`.
    let raw_method = record.get("method");
    let Some(method) = raw_method
        .and_then(Value::as_str)
        .and_then(SubagentRpcMethod::from_wire)
    else {
        return Err(SubagentRpcError::new(
            SubagentRpcErrorCode::UnsupportedMethod,
            format!(
                "Unsupported subagent RPC method: {}.",
                raw_method.map_or_else(|| "undefined".to_string(), ToString::to_string)
            ),
        ));
    };
    Ok(SubagentRpcRequest {
        request_id,
        method,
        // `:784` — carried through only when present, so an absent `params` stays absent rather
        // than becoming an explicit `null` a normalizer would then have to special-case.
        params: record.get("params").cloned(),
        // `:785` — a non-object `source` is DROPPED, not rejected.
        source: record.get("source").filter(|v| v.is_object()).cloned(),
    })
}

/// pi's success envelope (`rpc.ts:827-833`).
pub(crate) fn success_reply(request: &SubagentRpcRequest, data: Value) -> Value {
    serde_json::json!({
        "version": SUBAGENT_RPC_PROTOCOL_VERSION,
        "requestId": request.request_id,
        "method": request.method.as_str(),
        "success": true,
        "data": data,
    })
}

/// pi `errorReply` (`rpc.ts:797-815`).
///
/// `raw` is the ORIGINAL request value, because this is reached both when parsing failed (so there
/// is no envelope) and when handling failed (where upstream passes `request ?? raw`, `:835`). The
/// `method` key rides along only when the raw value names one of the eight (`:799-801`), so a
/// reply to `method: "teleport"` carries no `method` at all.
pub(crate) fn error_reply(raw: &Value, error: &SubagentRpcError) -> Value {
    let request_id = safe_reply_request_id(raw);
    let method = as_record(raw)
        .and_then(|record| record.get("method"))
        .and_then(Value::as_str)
        .and_then(SubagentRpcMethod::from_wire);
    let mut reply = serde_json::Map::new();
    reply.insert(
        "version".to_string(),
        Value::from(SUBAGENT_RPC_PROTOCOL_VERSION),
    );
    reply.insert("requestId".to_string(), Value::from(request_id));
    if let Some(method) = method {
        reply.insert("method".to_string(), Value::from(method.as_str()));
    }
    reply.insert("success".to_string(), Value::Bool(false));
    reply.insert(
        "error".to_string(),
        serde_json::json!({ "code": error.code.as_str(), "message": error.message }),
    );
    Value::Object(reply)
}

/// The bus topic an error reply goes out on, when there may be no parsed request to name it: pi
/// `options.events.emit(subagentRpcReplyEvent(reply.requestId), reply)` (`rpc.ts:836`), where
/// `reply.requestId` is already `safeReplyRequestId`'s answer.
pub(crate) fn error_reply_topic(raw: &Value) -> String {
    subagent_rpc_reply_event(&safe_reply_request_id(raw))
}
