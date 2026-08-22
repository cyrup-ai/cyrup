//! Stop-reason table and error composition.

use super::*;

#[test]
fn stop_reason_table_matches_upstream() {
    assert_eq!(map_stop_reason(Some("end_turn")), (StopReason::Stop, None));
    assert_eq!(
        map_stop_reason(Some("stop_sequence")),
        (StopReason::Stop, None)
    );
    assert_eq!(
        map_stop_reason(Some("max_tokens")),
        (StopReason::Length, None)
    );
    assert_eq!(
        map_stop_reason(Some("model_context_window_exceeded")),
        (StopReason::Length, None)
    );
    assert_eq!(
        map_stop_reason(Some("tool_use")),
        (StopReason::ToolUse, None)
    );
    // pi `bedrock-raw-stop-reason.test.ts:78-86`: the diagnostic is part of the mapping.
    assert_eq!(
        map_stop_reason(Some("guardrail_intervened")),
        (
            StopReason::Error,
            Some("Provider stopped with: guardrail_intervened".to_string())
        )
    );
    assert_eq!(map_stop_reason(None), (StopReason::Error, None));
}

#[test]
fn service_errors_carry_the_legacy_prefix_and_the_status_body_core() {
    assert_eq!(
        format_bedrock_service_error("ThrottlingException", 429, "{\"message\":\"slow down\"}"),
        "Throttling error: 429: {\"message\":\"slow down\"}"
    );
    assert_eq!(
        format_bedrock_service_error("ServiceUnavailableException", 503, "down"),
        "Service unavailable: 503: down"
    );
    // An unmodeled shape falls back to the raw name (pi's `?? error.name`).
    assert_eq!(
        format_bedrock_service_error("AccessDeniedException", 403, "nope"),
        "AccessDeniedException: 403: nope"
    );
}

#[test]
fn a_data_retention_message_gains_the_docs_hint() {
    let out = format_bedrock_service_error(
        "ValidationException",
        400,
        "data retention mode 'default' is not available for this model",
    );
    assert!(out.starts_with("Validation error: 400: data retention mode"));
    assert!(out.ends_with(&format!(
        " See {BEDROCK_DATA_RETENTION_DOCS_URL} for supported data retention modes."
    )));

    // MIRROR: an unrelated message gains nothing.
    let plain = format_bedrock_service_error("ValidationException", 400, "bad input");
    assert_eq!(plain, "Validation error: 400: bad input");
}
