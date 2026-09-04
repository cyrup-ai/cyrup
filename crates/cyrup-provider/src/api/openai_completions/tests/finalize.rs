//! SSE decode: finish reasons, stop-reason mapping and error terminals.

use super::*;

/// PORT BUG (present at v0.83.0, never ported): pi writes
/// `output.rawStopReason = choice.finish_reason`
/// (`v0.84.1 ai/src/api/openai-completions.ts:463`; `v0.83.0 …:459`). This is the widest-reach
/// of the five missing writers — `openai-completions` is the fleet wire api behind 16 built-in
/// providers, whose finish reasons are the least standardized in the workspace.
#[tokio::test]
async fn a_finish_reason_is_recorded_raw_beside_the_narrowed_one() {
    let events =
        collect_events("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\ndata: [DONE]\n\n")
            .await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(error.raw_stop_reason.as_deref(), Some("content_filter"));

    // MIRROR 1: a clean `stop` keeps its raw word on the `done` terminal.
    let events = collect_events(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));

    // MIRROR 2: no `finish_reason` ever arrives → nothing recorded, and the terminal is the
    // truncation error, not a fabricated raw value.
    let events = collect_events(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Stream ended without finish_reason")
    );
    assert_eq!(error.raw_stop_reason, None);
}

/// VERSION LAG (v0.83.0 → v0.84.1): the new `compat.supportsFinishReason` key
/// (v0.84.1 `ai/src/types.ts:547-548`, detected default `true` at
/// `ai/src/api/openai-completions.ts:1499`) makes a stream that ends with no `finish_reason`
/// INFER its stop reason instead of erroring:
/// `output.stopReason = output.content.some(b => b.type === "toolCall") ? "toolUse" : "stop"`
/// (v0.84.1 `ai/src/api/openai-completions.ts:578-580`). At v0.83.0 (`…:577`) the guard was the
/// unconditional `if (!hasFinishReason || output.stopReason === "pending") throw`.
#[tokio::test]
async fn absent_finish_reason_is_inferred_when_the_provider_reports_none() {
    let quiet = || {
        let mut m = model();
        m.compat = Some(ModelCompat {
            supports_finish_reason: Some(false),
            ..ModelCompat::default()
        });
        m
    };

    // No tool call in the turn → `"stop"`.
    let events = collect_events_with(
        quiet(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.error_message, None);

    // A tool call in the turn → `"toolUse"`.
    let events = collect_events_with(
        quiet(),
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"function\":{\"name\":\"add\",\"arguments\":\"{}\"}}]}}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::ToolUse);

    // MIRROR 1: the inference is a FALLBACK — a delivered `finish_reason` still wins, even for
    // a provider flagged `supportsFinishReason: false` (pi's guard is `!hasFinishReason && …`).
    let events = collect_events_with(
        quiet(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let Some(StreamEvent::Done { message, .. }) = events.last() else {
        panic!("expected a done terminal, got {:?}", events.last());
    };
    assert_eq!(message.stop_reason, StopReason::Length);

    // MIRROR 2: at the DEFAULT `supportsFinishReason: true`, a missing reason is still the
    // truncated-stream error — the fix must not turn every truncation into a clean `stop`
    // (v0.84.1 `…:584-586`).
    let events = collect_events(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", events.last());
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Stream ended without finish_reason")
    );

    // MIRROR 3: `detectCompat` leaves the flag `true` for every provider (v0.84.1 `…:1499`),
    // so an unconfigured model never infers.
    assert!(get_compat(&model()).supports_finish_reason);
}

#[tokio::test]
async fn finish_reason_length_maps_to_length() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;
    match events.last() {
        Some(StreamEvent::Done { message, .. }) => {
            assert_eq!(message.stop_reason, StopReason::Length)
        }
        other => panic!("expected Done terminal, got {other:?}"),
    }
}

#[tokio::test]
async fn content_filter_maps_to_error_terminal() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;
    match events.last() {
        Some(StreamEvent::Error { error: message, .. }) => {
            assert_eq!(message.stop_reason, StopReason::Error);
            assert!(
                message
                    .error_message
                    .as_deref()
                    .unwrap()
                    .contains("content_filter")
            );
        }
        other => panic!("expected Error terminal, got {other:?}"),
    }
}

// Gap 1: Pi openai-completions.ts:452-454 — a stream that ends without ever emitting a
// `finish_reason` is a protocol error, not a defaulted success.
#[tokio::test]
async fn stream_without_finish_reason_errors() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;
    match events.last() {
        Some(StreamEvent::Error { error: message, .. }) => {
            assert_eq!(message.stop_reason, StopReason::Error);
            assert_eq!(
                message.error_message.as_deref(),
                Some("Stream ended without finish_reason")
            );
        }
        other => panic!("expected Error terminal, got {other:?}"),
    }
}

// Gap 4: Pi error enrichment (openai-completions.ts:466-469) — an OpenRouter-style error chunk
// with `error.metadata.raw` appends the raw provider detail to the terminal error message.
#[tokio::test]
async fn provider_error_chunk_appends_raw_metadata() {
    let raw = concat!(
        "data: {\"error\":{\"message\":\"upstream failed\",\"metadata\":{\"raw\":\"503 Service Unavailable\"}}}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;
    match events.last() {
        Some(StreamEvent::Error { error: message, .. }) => {
            assert_eq!(message.stop_reason, StopReason::Error);
            let em = message.error_message.as_deref().unwrap();
            assert!(em.contains("upstream failed"), "got: {em}");
            assert!(em.contains("503 Service Unavailable"), "got: {em}");
        }
        other => panic!("expected Error terminal, got {other:?}"),
    }
}
