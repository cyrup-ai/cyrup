//! Terminal error / incomplete handling (G12).

use super::*;

// -----------------------------------------------------------------------
// G12: a Responses terminal must never settle on `error` with no message, and the v0.84.1
// `incomplete` split.
//
// `azure-openai-responses` reaches this decoder through the same `decode_stream` import
// (`azure_openai_responses.rs:25-27`, used at `:198`), mirroring upstream's shared
// `processResponsesStream` (v0.84.1 `azure-openai-responses.ts:20,129`), and it carries the
// identical guard at `azure-openai-responses.ts:138-139`. Both api ids are exercised so a fix
// that missed the sibling would be caught.
// -----------------------------------------------------------------------

/// The two api ids that share this decoder and both spell the guard upstream.
const RESPONSES_SIBLINGS: [&str; 2] = ["openai-responses", "azure-openai-responses"];

async fn terminal_for(api_id: &str, raw: &str) -> AssistantMessage {
    let frames = decode_sse_bytes(raw.as_bytes().to_vec());
    let (sink, rx) = crate::api::channel(64);
    let m = model();
    let api = ApiId::from(api_id);
    decode_stream(frames, &m, &api, &sink).await;
    drop(sink);
    let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
    collect_message(stream).await
}

/// A settled `error` stop reason with nothing recorded: upstream throws
/// `output.errorMessage || "An unknown error occurred"` (v0.84.1 `openai-responses.ts:174`,
/// `azure-openai-responses.ts:139`) and the catch writes that text back to `errorMessage`
/// (`openai-responses.ts:188`), so the fallback text is always present. `mapStopReason`'s
/// `failed`/`cancelled` arm supplies no message (v0.84.1 `openai-responses-shared.ts:760-762`),
/// which is exactly how this state is reached.
#[tokio::test]
async fn settled_error_terminal_always_carries_a_message() {
    for api_id in RESPONSES_SIBLINGS {
        for status in ["failed", "cancelled"] {
            let raw = format!(
                "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"r\",\"status\":\"{status}\"}}}}\n\n"
            );
            let msg = terminal_for(api_id, &raw).await;
            assert_eq!(
                msg.stop_reason,
                StopReason::Error,
                "{api_id} / {status} stop reason"
            );
            assert_eq!(
                msg.error_message.as_deref(),
                Some("An unknown error occurred"),
                "{api_id} / {status} error message"
            );
            assert_eq!(msg.raw_stop_reason.as_deref(), Some(status));
        }
    }
}

/// v0.84.1 `openai-responses-shared.ts:750-759`: only `max_output_tokens` is a clean `length`
/// stop; every other incomplete reason is an error terminal carrying the provider's reason.
/// Mapping them all to `length` (the ported `v0.83.0:744-745` behaviour) reported a
/// content-filtered response as a successful turn.
#[tokio::test]
async fn incomplete_without_max_output_tokens_is_an_error_terminal() {
    for api_id in RESPONSES_SIBLINGS {
        let filtered = terminal_for(
            api_id,
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"content_filter\"}}}\n\n",
        )
        .await;
        assert_eq!(filtered.stop_reason, StopReason::Error, "{api_id}");
        assert_eq!(
            filtered.error_message.as_deref(),
            Some("Response incomplete: content_filter"),
            "{api_id}"
        );
        // `${status}.${incompleteReason}` (v0.84.1 openai-responses-shared.ts:570).
        assert_eq!(
            filtered.raw_stop_reason.as_deref(),
            Some("incomplete.content_filter"),
            "{api_id}"
        );

        let bare = terminal_for(
            api_id,
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\"}}\n\n",
        )
        .await;
        assert_eq!(bare.stop_reason, StopReason::Error, "{api_id}");
        assert_eq!(
            bare.error_message.as_deref(),
            Some("Response incomplete without a provider reason"),
            "{api_id}"
        );
        assert_eq!(
            bare.raw_stop_reason.as_deref(),
            Some("incomplete"),
            "{api_id}"
        );
    }
}

/// Upstream's `default:` arm is a `never` exhaustiveness check that *throws*
/// `Unhandled stop reason: <status>` (v0.84.1 `openai-responses-shared.ts:767-770`); the throw
/// lands in the caller's catch and becomes an error terminal, never a clean stop.
#[tokio::test]
async fn unknown_status_is_an_error_terminal() {
    for api_id in RESPONSES_SIBLINGS {
        let msg = terminal_for(
            api_id,
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"status\":\"bogus\"}}\n\n",
        )
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error, "{api_id}");
        assert_eq!(
            msg.error_message.as_deref(),
            Some("Unhandled stop reason: bogus"),
            "{api_id}"
        );
    }
}

/// MIRROR: the success statuses must stay clean terminals with no error message, and
/// `rawStopReason` must be stamped on them too (v0.84.1 `openai-responses-shared.ts:570` runs
/// on every settled turn, not just failures). Proves the guard is not over-broad.
#[tokio::test]
async fn success_statuses_stay_clean_terminals() {
    let cases = [
        (
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"status\":\"completed\"}}\n\n",
            StopReason::Stop,
            Some("completed"),
        ),
        (
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            StopReason::Length,
            Some("incomplete.max_output_tokens"),
        ),
        (
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"status\":\"queued\"}}\n\n",
            StopReason::Stop,
            Some("queued"),
        ),
        (
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\"}}\n\n",
            StopReason::Stop,
            None,
        ),
    ];
    for api_id in RESPONSES_SIBLINGS {
        for (raw, want_reason, want_raw) in cases {
            let msg = terminal_for(api_id, raw).await;
            assert_eq!(msg.stop_reason, want_reason, "{api_id} / {raw}");
            assert_eq!(msg.error_message, None, "{api_id} / {raw}");
            assert_eq!(msg.raw_stop_reason.as_deref(), want_raw, "{api_id} / {raw}");
        }
    }
}
