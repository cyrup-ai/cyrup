//! The failure value the ported `try` block returns, and its structured diagnostics
//! (pi `appendBedrockFailureDiagnostic`, v0.84.1 `ai/src/api/bedrock-converse-stream.ts:398-421`).

use cyrup_core::{
    AssistantMessage, StopReason, diagnostics::create_assistant_message_diagnostic_from,
};
use serde_json::{Map, Value, json};

/// A failure inside the ported `try` block: the partial snapshot to attach plus the composed
/// `errorMessage` (already run through [`format_bedrock_error`](super::errors::format_bedrock_error)).
///
/// `status`/`error_code` are the parts of upstream's thrown SDK exception that survive into the
/// structured diagnostic (`error.$metadata.httpStatusCode` and `error.name`); they are `None` for
/// the failure paths whose upstream counterpart throws a plain `Error` carrying neither.
pub(super) struct BedrockFailure {
    pub(super) partial: AssistantMessage,
    pub(super) stop_reason: StopReason,
    pub(super) message: String,
    pub(super) status: Option<u16>,
    pub(super) error_code: Option<String>,
    /// Upstream's hoisted `responseRequestId` (pi `:225`, assigned at `:254`), carried on the
    /// failure so the catch can still correlate a mid-stream throw that has no metadata of its own.
    pub(super) request_id: Option<String>,
}

impl BedrockFailure {
    pub(super) fn errored(partial: AssistantMessage, message: String) -> Self {
        BedrockFailure {
            partial,
            stop_reason: StopReason::Error,
            message,
            status: None,
            error_code: None,
            request_id: None,
        }
    }

    /// Attach the hoisted response request id (pi `:254`) to a failure raised after the response
    /// headers were seen.
    pub(super) fn with_request_id(mut self, request_id: Option<&str>) -> Self {
        self.request_id = request_id.map(str::to_string);
        self
    }

    /// The `client.send()` rejection path: upstream's throw is a `BedrockRuntimeServiceException`,
    /// so `$metadata.httpStatusCode` and the modeled `.name` are both present.
    pub(super) fn service_exception(
        partial: AssistantMessage,
        message: String,
        status: u16,
        name: &str,
    ) -> Self {
        BedrockFailure {
            partial,
            stop_reason: StopReason::Error,
            message,
            status: Some(status),
            error_code: extract_bedrock_error_code(name),
            request_id: None,
        }
    }
}

/// Over-long values are DROPPED rather than truncated (pi `MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS`,
/// v0.84.1 `ai/src/api/bedrock-converse-stream.ts:379`): a truncated request id is not a request id.
pub(super) const MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS: usize = 200;

/// pi `normalizeDiagnosticValue` (v0.84.1 `ai/src/api/bedrock-converse-stream.ts:381-386`).
///
/// **Unit**: pi's guard is `trimmed.length > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS` (`:384`), and JS
/// `String.prototype.length` counts UTF-16 CODE UNITS, not scalar values. The exact Rust analog is
/// [`str::encode_utf16`]`().count()`; `chars().count()` (scalars, what this was) and `len()`
/// (UTF-8 bytes) agree with it only for ASCII. Astral-plane characters are two UTF-16 units each,
/// so a 150-emoji request id is 300 units to pi — dropped — and 150 scalars to a `chars()`-based
/// cyrup — kept, emitting a `requestId` diagnostic pi never emits. Same reasoning, and the same
/// fix, as `cyrup-permission-system/src/wildcard.rs:21-23,81`.
pub(super) fn normalize_diagnostic_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.encode_utf16().count() > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS {
        return None;
    }
    Some(trimmed.to_string())
}

/// pi `extractBedrockErrorCode` (v0.84.1 `ai/src/api/bedrock-converse-stream.ts:388-396`): modeled
/// Bedrock errors all end in `Exception`, unlike transport names such as `TimeoutError`, so a name
/// that does not is not a code.
pub(super) fn extract_bedrock_error_code(name: &str) -> Option<String> {
    if !name.ends_with("Exception") {
        return None;
    }
    normalize_diagnostic_value(name)
}

/// pi `appendBedrockFailureDiagnostic` (v0.84.1 `ai/src/api/bedrock-converse-stream.ts:398-421`),
/// called from the catch at `:318-320` whenever the terminal reason settled on `"error"`.
///
/// VERSION LAG (v0.83.0 → v0.84.1): neither `appendBedrockFailureDiagnostic`,
/// `normalizeDiagnosticValue` nor the hoisted `responseRequestId` (`:225`) exists at v0.83.0 — the
/// whole structured-diagnostic path is new in v0.84.1.
///
/// Structured metadata sits ALONGSIDE `error_message`, which stays byte-identical because the
/// turn-level retry classifier matches against it. Unknown fields are omitted, never guessed: a
/// modeled mid-stream exception reaches upstream as a bare object literal (not an `Error`, no
/// `$metadata`), leaving only the fallback request id — which is why `error_code`/`status` are
/// passed `None` on that path here too. When nothing is known the diagnostic is not appended at all.
pub(super) fn append_bedrock_failure_diagnostic(
    output: &mut AssistantMessage,
    status: Option<u16>,
    error_code: Option<&str>,
    fallback_request_id: Option<&str>,
) {
    let mut details = Map::new();
    if let Some(status) = status {
        details.insert("status".to_string(), json!(status));
    }
    if let Some(code) = error_code {
        details.insert("errorCode".to_string(), json!(code));
    }
    if let Some(id) = fallback_request_id.and_then(normalize_diagnostic_value) {
        details.insert("requestId".to_string(), json!(id));
    }
    if details.is_empty() {
        return;
    }
    output.append_diagnostic(create_assistant_message_diagnostic_from(
        "bedrock_response_failure",
        None,
        Some(Value::Object(details)),
    ));
}
