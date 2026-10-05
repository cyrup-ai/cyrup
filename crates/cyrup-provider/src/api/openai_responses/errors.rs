//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! the terminal `error` event (Pi catch block), and the ChatGPT-usage hint that catch appends.

use super::decoder::RDecoder;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, StopReason};

/// `CHATGPT_USAGE_URL` (`openai-responses.ts:34` @v1.0.1).
pub(super) const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/settings/usage";

/// The non-retryable limit code "Sign in with ChatGPT" returns when the subscription's **shared**
/// usage allowance is spent (`utils/retry.ts:27`, and the hint's trigger at
/// `openai-responses.ts:228`). Also carried by [`crate::utils::retry`]'s non-retryable set, which
/// is where the "do not back off, this resets in hours" half of the behaviour lives.
pub(super) const SUBSCRIPTION_SHARING_USAGE_LIMIT_EXCEEDED: &str =
    "subscription_sharing_usage_limit_exceeded";

/// PROV-118 — the hint pi's catch block appends (`openai-responses.ts:226-229` @v1.0.1):
///
/// ```ts
/// // Sign in with ChatGPT shares the subscription's usage limit with other apps.
/// output.errorMessage = errorMessage.includes("subscription_sharing_usage_limit_exceeded")
///     ? `${errorMessage}\nCheck your ChatGPT usage: ${CHATGPT_USAGE_URL}`
///     : errorMessage;
/// ```
///
/// **The trigger is the error text, not the credential.** `isChatGPTSignIn` is *not* consulted
/// here — upstream's `:228` tests only `errorMessage.includes(...)`, and its own test
/// (`test/openai-responses-usage-limit.test.ts`) drives both cases with a plain `apiKey: "test"`.
/// That is coherent: only a token-sharing request can be answered with this code at all, so the
/// code *is* the discriminator, and gating on the credential as well would hide the hint from a
/// request whose credential cyrup classified differently (a gateway in front of OpenAI, say) while
/// the endpoint still reported a shared-subscription limit. Keep it keyed on the text.
///
/// Pure, so both arms are assertable without a request.
pub(super) fn with_chatgpt_usage_hint(message: String) -> String {
    if message.contains(SUBSCRIPTION_SHARING_USAGE_LIMIT_EXCEEDED) {
        format!("{message}\nCheck your ChatGPT usage: {CHATGPT_USAGE_URL}")
    } else {
        message
    }
}

/// [`with_chatgpt_usage_hint`] applied to an already-assembled terminal event.
///
/// Upstream's hint sits in the `catch` that wraps the **whole** of `stream` (`:215-232`), so it
/// covers the HTTP failure and the transport failure as well as the mid-stream `response.failed`.
/// cyrup assembles those three into a [`StreamEvent`] at different points
/// (`ProviderError::into_error_event`, vs [`emit_error`] for the decoder), so the hint is applied
/// to the event for the first two and to the message for the third.
pub(super) fn hint_terminal_error(event: StreamEvent) -> StreamEvent {
    let StreamEvent::Error { reason, error } = event else {
        return event;
    };
    let mut message = (*error).clone();
    if let Some(text) = message.error_message.take() {
        message.error_message = Some(with_chatgpt_usage_hint(text));
    }
    StreamEvent::Error {
        reason,
        error: std::sync::Arc::new(message),
    }
}

/// Emit a terminal `error` event carrying the live snapshot + message (Pi catch block).
pub(super) async fn emit_error(
    dec: &mut RDecoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    message: String,
) {
    let mut msg = dec.snapshot_owned(model, api);
    msg.stop_reason = StopReason::Error;
    // PROV-118 — `:226-229`. Every decoder-side error funnels through here, so the hint cannot be
    // applied to one event kind and forgotten on another.
    msg.error_message = Some(with_chatgpt_usage_hint(message));
    sink.send(StreamEvent::terminal(msg)).await;
}
