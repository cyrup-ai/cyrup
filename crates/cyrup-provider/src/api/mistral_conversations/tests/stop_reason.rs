//! The `finishReason` table.

use super::*;

/// pi `mapChatStopReason` (`mistral-conversations.ts:662-677`). The unknown arm is the whole
/// point: before this, `Some(_) => StopReason::Stop` meant a provider-terminated turn was
/// transcribed as a clean success — no error banner, no retry, just the partial text that
/// arrived before the cutoff.
#[test]
fn an_unrecognized_finish_reason_is_an_error_not_a_clean_stop() {
    // The reason that motivated this: a real Mistral value outside the known five.
    let (stop, err) = map_chat_stop_reason(Some("content_filter"));
    assert_eq!(stop, StopReason::Error, "must NOT be transcribed as Stop");
    assert_eq!(
        err.as_deref(),
        Some("Provider stopped with: content_filter")
    );

    // Anything Mistral adds later behaves the same way, by construction.
    let (stop, err) = map_chat_stop_reason(Some("some_future_reason"));
    assert_eq!(stop, StopReason::Error);
    assert_eq!(
        err.as_deref(),
        Some("Provider stopped with: some_future_reason")
    );

    // pi's explicit `"error"` arm carries its own message rather than letting the call site
    // fall back to the generic "An unknown error occurred".
    let (stop, err) = map_chat_stop_reason(Some("error"));
    assert_eq!(stop, StopReason::Error);
    assert_eq!(err.as_deref(), Some("Provider stopped with: error"));

    // The known-good arms stay clean and carry no message.
    for (reason, expected) in [
        (None, StopReason::Stop),
        (Some("stop"), StopReason::Stop),
        (Some("length"), StopReason::Length),
        (Some("model_length"), StopReason::Length),
        (Some("tool_calls"), StopReason::ToolUse),
    ] {
        let (stop, err) = map_chat_stop_reason(reason);
        assert_eq!(stop, expected, "{reason:?}");
        assert_eq!(err, None, "{reason:?} must carry no errorMessage");
    }
}
