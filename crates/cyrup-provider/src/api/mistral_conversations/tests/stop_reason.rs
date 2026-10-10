//! The `finishReason` table.

use super::*;

/// pi `mapChatStopReason` (`mistral-conversations.ts:936-952` @f1b2e77f5). The unknown arm is the whole
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
    // fall back to the generic "An unknown error occurred" (`:946-948` @f1b2e77f5).
    let (stop, err) = map_chat_stop_reason(Some("error"));
    assert_eq!(stop, StopReason::Error);
    assert_eq!(
        err.as_deref(),
        Some("Provider stopped with: error (server error)")
    );

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

/// PROV-141 — pi `7fb59f995` (#10487): Mistral reports transient server failures as
/// `finish_reason: "error"`, and upstream's `"error"` arm says `(server error)` so the retry
/// classifier's `server.?error` pattern matches. Asserted on the message the decoder actually
/// emits, then fed to the same classifier the agent loop uses, so a classifier change or a
/// decoder that stops carrying the mapped message both bite.
#[tokio::test]
async fn prov141_a_mistral_error_finish_reason_is_retryable_and_an_unknown_one_is_not() {
    use crate::utils::retry::is_retryable_assistant_error;
    let m = model_with("mistral-large-latest", false);

    let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"error\"}]}\n\ndata: [DONE]\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(error.stop_reason, StopReason::Error);
    assert_eq!(
        error.error_message.as_deref(),
        Some("Provider stopped with: error (server error)")
    );
    assert!(
        is_retryable_assistant_error(error),
        "a Mistral `error` finish must be auto-retried: {:?}",
        error.error_message
    );

    // upstream's negative case: an unknown reason stays non-retryable.
    let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"unmapped_error\"}]}\n\ndata: [DONE]\n\n";
    let events = collect(raw.as_bytes().to_vec(), &m).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Provider stopped with: unmapped_error")
    );
    assert!(!is_retryable_assistant_error(error));
}
