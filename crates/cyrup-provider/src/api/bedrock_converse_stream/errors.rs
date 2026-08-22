//! Errors (pi `formatBedrockError`, `bedrock-converse-stream.ts:326-365`).

use cyrup_core::StopReason;

/// pi `BEDROCK_DATA_RETENTION_DOCS_URL` (`bedrock-converse-stream.ts:339`).
pub(super) const BEDROCK_DATA_RETENTION_DOCS_URL: &str =
    "https://docs.aws.amazon.com/bedrock/latest/userguide/data-retention.html";

/// pi `BEDROCK_ERROR_PREFIXES` (`bedrock-converse-stream.ts:326-332`). The prefixes are legacy and
/// load-bearing: the turn-level retry classifier matches `server.?error` / `service.?unavailable`
/// against this string, so the raw SDK exception name must not be used instead.
pub(super) fn bedrock_error_prefix(name: &str) -> Option<&'static str> {
    match name {
        "InternalServerException" => Some("Internal server error"),
        "ModelStreamErrorException" => Some("Model stream error"),
        "ValidationException" => Some("Validation error"),
        "ThrottlingException" => Some("Throttling error"),
        "ServiceUnavailableException" => Some("Service unavailable"),
        _ => None,
    }
}

/// pi's data-retention hint (`bedrock-converse-stream.ts:357-359`), appended whenever the core
/// message mentions a data retention mode (case-insensitively).
pub(super) fn data_retention_hint(core: &str) -> String {
    if core.to_lowercase().contains("data retention mode") {
        format!(" See {BEDROCK_DATA_RETENTION_DOCS_URL} for supported data retention modes.")
    } else {
        String::new()
    }
}

/// pi `formatBedrockError` for a non-SDK error (`bedrock-converse-stream.ts:364`): the message plus
/// the data-retention hint, with no prefix.
pub(super) fn format_bedrock_error(message: &str) -> String {
    format!("{message}{}", data_retention_hint(message))
}

/// pi `formatBedrockError` for a `BedrockRuntimeServiceException` (`:360-363`).
///
/// The SDK folds an HTTP error into an exception whose `.name` is the modeled shape name and whose
/// `.message` is the body's `message` field. Here that arrives as the `x-amzn-errortype` header plus
/// the response body, so `core` is composed the way `normalizeProviderError` composes it when the
/// message does NOT already carry the body: `"<status>: <body>"` (`:353-356`). An unmodeled error
/// type falls back to the raw name, exactly as upstream's `?? error.name` does.
pub(super) fn format_bedrock_service_error(name: &str, status: u16, body: &str) -> String {
    let core = if body.is_empty() {
        format!("{status}")
    } else {
        format!("{status}: {body}")
    };
    let hint = data_retention_hint(&core);
    match bedrock_error_prefix(name) {
        Some(prefix) => format!("{prefix}: {core}{hint}"),
        None if !name.is_empty() => format!("{name}: {core}{hint}"),
        None => format!("{core}{hint}"),
    }
}

/// pi `mapStopReason` (`bedrock-converse-stream.ts:962-977`).
///
/// Returns `(stopReason, errorMessage)` — the diagnostic is inseparable from the mapping: without
/// it a guardrail/content-filter stop would land on the generic `"An unknown error occurred"`
/// fallback and become indistinguishable from a transport failure.
pub(super) fn map_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        Some("end_turn") | Some("stop_sequence") => (StopReason::Stop, None),
        Some("max_tokens") | Some("model_context_window_exceeded") => (StopReason::Length, None),
        Some("tool_use") => (StopReason::ToolUse, None),
        Some(other) if !other.is_empty() => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
        _ => (StopReason::Error, None),
    }
}

/// Upper-case the first character (`internalServerException` → `InternalServerException`), so an
/// event-stream `:exception-type` maps onto the SDK exception names
/// [`bedrock_error_prefix`] is keyed by.
pub(super) fn upper_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
