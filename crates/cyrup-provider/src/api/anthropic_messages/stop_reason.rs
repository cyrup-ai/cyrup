//! The Anthropic `stop_reason` table (Pi `mapStopReason`).

use cyrup_core::StopReason;
use serde_json::Value;

/// Map an Anthropic `stop_reason` to a cyrup [`StopReason`] (Pi `mapStopReason`,
/// anthropic-messages.ts:1325-1351 @ v0.83.0). Unknown reasons return an error with a message.
///
/// Every arm that yields [`StopReason::Error`] must also yield a message: Pi surfaces it as
/// `throw new Error(output.errorMessage || "An unknown error occurred")`
/// (anthropic-messages.ts:755), so a `None` here silently degrades to that generic fallback.
pub(super) fn map_stop_reason(
    reason: &str,
    stop_details: Option<&Value>,
) -> (StopReason, Option<String>) {
    match reason {
        "end_turn" => (StopReason::Stop, None),
        "max_tokens" => (StopReason::Length, None),
        "tool_use" => (StopReason::ToolUse, None),
        "refusal" => {
            let explanation = stop_details
                .and_then(|d| d.get("explanation"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "The model refused to complete the request".to_string());
            (StopReason::Error, Some(explanation))
        }
        "pause_turn" | "stop_sequence" => (StopReason::Stop, None),
        // Content flagged by Anthropic's safety filters (not yet in the SDK types). The message is
        // load-bearing: without it the terminal falls through to the generic
        // `"An unknown error occurred"` fallback (`decode_stream`, below) and a content-policy stop
        // becomes indistinguishable from a transport failure.
        "sensitive" => (
            StopReason::Error,
            Some("Provider stopped with: sensitive".to_string()),
        ),
        other => (
            StopReason::Error,
            Some(format!("Unhandled stop reason: {other}")),
        ),
    }
}
