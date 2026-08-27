//! The Gemini `finishReason` table (Pi `mapStopReason`, google-shared.ts:309-336).

use cyrup_core::StopReason;

/// Map a raw Gemini `finishReason` to `(stop_reason, error_message)` (Pi `mapStopReason`,
/// google-shared.ts:309-336 — only `STOP`/`MAX_TOKENS` are non-error).
///
/// The message half is the point. Gemini's characteristic failures are all finish reasons rather
/// than HTTP errors — `SAFETY`, `RECITATION`, `PROHIBITED_CONTENT`, `BLOCKLIST`,
/// `MALFORMED_FUNCTION_CALL` — and this used to discard the raw string, so every one of them
/// surfaced as the identical, information-free "An unknown error occurred". A content-policy
/// refusal and a tool-schema bug demand completely different responses from the user, and the
/// message carried nothing to tell them apart.
///
/// pi keeps the raw value on `output.rawStopReason` (`google-generative-ai.ts:214-216`) and builds
/// the terminal error as ``output.rawStopReason ? `Provider stopped with: ${output.rawStopReason}`
/// : "An unknown error occurred"`` (`:269-273`), so the reason names itself.
pub(super) fn map_stop_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "STOP" => (StopReason::Stop, None),
        "MAX_TOKENS" => (StopReason::Length, None),
        other => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
    }
}
