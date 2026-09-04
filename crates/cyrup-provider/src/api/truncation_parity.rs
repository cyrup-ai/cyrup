//! Cross-converter regression suite for PROV-010 / AGENT-014 / DRIFT-012: **a truncated stream must
//! never be transcribed as a completed turn**, and every wire API must agree on that.
//!
//! Pi seeds `output.stopReason = "pending"` in every stream function and, at end of stream, throws
//! rather than pushing a `done` event when the provider never delivered a terminal stop reason
//! (`anthropic-messages.ts:751`, `google-generative-ai.ts:266`, `mistral-conversations.ts:88`,
//! `openai-responses.ts:170`, `openai-completions.ts:580`). The throw is caught by the same
//! function's `catch` and re-emitted as `{type:"error", reason:"error"}` carrying whatever content
//! had accumulated.
//!
//! cyrup used to honour that in `openai_completions` (and, differently spelled, in
//! `openai_responses`) while `anthropic_messages`, `google_generative_ai` and
//! `mistral_conversations` defaulted a stop-reason-less stream to a clean `Stop` — a SILENT wrong
//! answer: the truncated turn was persisted to JSONL as complete and fed back to the model as a
//! finished assistant message. All five now route their end-of-stream path through
//! [`StreamEvent::end_of_stream`], and this suite drives all five to prove they agree.
//!
//! Each api gets two fixtures that differ ONLY in the terminal stop reason, so a passing
//! "truncated" assertion cannot be an artefact of a malformed transcript: the `complete` twin must
//! still decode to `Done { reason: Stop }` with the same content.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::api::channel;
use crate::model::{Modality, Model, ModelCost};
use crate::stream::sse::decode_sse_bytes;
use crate::stream::{DoneReason, ErrorReason, StreamEvent};
use cyrup_core::{ApiId, Content, StopReason};

fn model_for(api: &str, provider: &str) -> Model {
    Model {
        id: "test-model".into(),
        name: "Test Model".into(),
        api: api.into(),
        provider: provider.into(),
        base_url: "https://example.invalid".to_string(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.5,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 4_096,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// The five wire APIs cyrup decodes, each identified by the `known_api` id its decoder is
/// registered under. `azure-openai-responses` is deliberately absent: it delegates verbatim to
/// `openai_responses::decode_stream` (`azure_openai_responses.rs` imports and calls it), so it is
/// covered transitively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wire {
    Anthropic,
    Google,
    Mistral,
    OpenAiCompletions,
    OpenAiResponses,
}

impl Wire {
    const ALL: [Wire; 5] = [
        Wire::Anthropic,
        Wire::Google,
        Wire::Mistral,
        Wire::OpenAiCompletions,
        Wire::OpenAiResponses,
    ];

    fn api_id(self) -> &'static str {
        match self {
            Wire::Anthropic => crate::known_api::ANTHROPIC_MESSAGES,
            Wire::Google => crate::known_api::GOOGLE_GENERATIVE_AI,
            Wire::Mistral => crate::known_api::MISTRAL_CONVERSATIONS,
            Wire::OpenAiCompletions => crate::known_api::OPENAI_COMPLETIONS,
            Wire::OpenAiResponses => crate::known_api::OPENAI_RESPONSES,
        }
    }

    fn provider(self) -> &'static str {
        match self {
            Wire::Anthropic => "anthropic",
            Wire::Google => "google",
            Wire::Mistral => "mistral",
            Wire::OpenAiCompletions | Wire::OpenAiResponses => "openai",
        }
    }

    /// The diagnostic Pi's `throw` puts in `errorMessage` for a still-`"pending"` output.
    fn truncated_diagnostic(self) -> &'static str {
        match self {
            Wire::Anthropic => "Anthropic stream ended without a stop reason",
            Wire::Google => "Google stream ended without a finish reason",
            Wire::Mistral => "Mistral stream ended without a finish reason",
            Wire::OpenAiCompletions => "Stream ended without finish_reason",
            Wire::OpenAiResponses => {
                "OpenAI Responses stream ended before a terminal response event"
            }
        }
    }

    /// A well-formed transcript that streams the text `Hello` and then **stops mid-turn**: the
    /// connection ends without the provider ever sending its terminal stop reason.
    ///
    /// The Anthropic fixture deliberately includes `message_stop`, so it clears the pre-existing
    /// `saw_message_start && !saw_message_stop` guard and lands on the code path that used to
    /// default to `Stop`. That path is the actual defect; a fixture that merely dropped
    /// `message_stop` would have passed before this fix.
    fn truncated(self) -> &'static str {
        match self {
            Wire::Anthropic => concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
                "event: content_block_stop\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            ),
            Wire::Google => {
                "data: {\"responseId\":\"resp_1\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}]}}]}\n\n"
            }
            Wire::Mistral => concat!(
                "data: {\"id\":\"resp_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
                "data: [DONE]\n\n",
            ),
            Wire::OpenAiCompletions => concat!(
                "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
                "data: [DONE]\n\n",
            ),
            Wire::OpenAiResponses => concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\"}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Hello\"}\n\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}}\n\n",
            ),
        }
    }

    /// The same stream, completed cleanly. Differs from [`Self::truncated`] ONLY by the terminal
    /// stop reason, so it pins the fixtures as otherwise valid.
    fn complete(self) -> &'static str {
        match self {
            Wire::Anthropic => concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
                "event: content_block_stop\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            ),
            Wire::Google => {
                "data: {\"responseId\":\"resp_1\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}]},\"finishReason\":\"STOP\"}]}\n\n"
            }
            Wire::Mistral => concat!(
                "data: {\"id\":\"resp_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            ),
            Wire::OpenAiCompletions => concat!(
                "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            ),
            Wire::OpenAiResponses => concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\"}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Hello\"}\n\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\n",
            ),
        }
    }
}

/// Drive one wire API's decoder over `raw` and return **every** event it emitted, in order.
async fn events(wire: Wire, raw: &str) -> Vec<StreamEvent> {
    let (sink, mut rx) = channel(256);
    let model = model_for(wire.api_id(), wire.provider());
    let api = ApiId::from(wire.api_id());
    let frames = decode_sse_bytes(raw.as_bytes().to_vec());

    let task = tokio::spawn(async move {
        match wire {
            Wire::Anthropic => {
                crate::api::anthropic_messages::decode_stream(
                    frames,
                    &model,
                    &api,
                    &sink,
                    false,
                    &[],
                )
                .await
            }
            Wire::Google => {
                crate::api::google_generative_ai::decode_stream(frames, &model, &api, &sink).await
            }
            Wire::Mistral => {
                crate::api::mistral_conversations::decode_stream(frames, &model, &api, &sink).await
            }
            Wire::OpenAiCompletions => {
                crate::api::openai_completions::decode_stream(frames, &model, &api, &sink).await
            }
            Wire::OpenAiResponses => {
                crate::api::openai_responses::decode_stream(frames, &model, &api, &sink).await
            }
        }
    });

    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    task.await.unwrap();
    out
}

/// Drive one wire API's decoder over `raw` and return the terminal event.
async fn terminal(wire: Wire, raw: &str) -> StreamEvent {
    let mut evs = events(wire, raw).await;
    let last = evs
        .pop()
        .unwrap_or_else(|| panic!("{wire:?}: decoder emitted no events at all"));
    assert!(
        last.terminal_message().is_some(),
        "{wire:?}: last event is not a terminal: {last:?}"
    );
    last
}

fn text_of(msg: &cyrup_core::AssistantMessage) -> String {
    msg.content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// THE regression: every converter must refuse to call a truncated stream `stop`.
#[tokio::test]
async fn truncated_stream_is_never_reported_as_a_completed_turn() {
    for wire in Wire::ALL {
        let ev = terminal(wire, wire.truncated()).await;

        match &ev {
            StreamEvent::Done { reason, message } => panic!(
                "{wire:?}: a stream that ended WITHOUT a terminal stop reason was transcribed as a \
                 completed turn — done(reason={reason:?}, stopReason={:?}). Pi throws here.",
                message.stop_reason
            ),
            StreamEvent::Error { reason, error } => {
                assert_eq!(
                    *reason,
                    ErrorReason::Error,
                    "{wire:?}: a truncated stream is a protocol error, not an abort"
                );
                assert_eq!(
                    error.stop_reason,
                    StopReason::Error,
                    "{wire:?}: terminal message must carry stopReason=error"
                );
                assert_eq!(
                    error.error_message.as_deref(),
                    Some(wire.truncated_diagnostic()),
                    "{wire:?}: diagnostic must match Pi's throw text"
                );
                // The partial content survives, so a caller can see WHAT was cut off — Pi's catch
                // block re-emits `output` with its accumulated blocks intact.
                assert_eq!(
                    text_of(error),
                    "Hello",
                    "{wire:?}: accumulated content must survive the error terminal"
                );
            }
            other => panic!("{wire:?}: unexpected terminal {other:?}"),
        }
    }
}

/// The control: the same fixtures WITH a terminal stop reason still decode cleanly. Without this,
/// the test above would also pass if a converter simply errored on everything.
#[tokio::test]
async fn complete_stream_still_reports_stop() {
    for wire in Wire::ALL {
        let ev = terminal(wire, wire.complete()).await;
        match &ev {
            StreamEvent::Done { reason, message } => {
                assert_eq!(*reason, DoneReason::Stop, "{wire:?}");
                assert_eq!(message.stop_reason, StopReason::Stop, "{wire:?}");
                assert_eq!(message.error_message, None, "{wire:?}");
                assert_eq!(text_of(message), "Hello", "{wire:?}");
            }
            other => panic!("{wire:?}: a complete stream must yield done/stop, got {other:?}"),
        }
    }
}

/// Mistral's second form of the same defect. Pi guards with `if (choice.finishReason)` — a JS
/// TRUTHINESS test (`mistral-conversations.ts:355`) — so an explicit `"finishReason": null` (or
/// `""`) leaves `output.stopReason` at its `"pending"` seed and the stream ends truncated. cyrup
/// had an extra `else if is_null` branch that mapped it to a clean `Stop`.
#[tokio::test]
async fn mistral_null_or_empty_finish_reason_is_truncation_not_stop() {
    for raw in [
        concat!(
            "data: {\"id\":\"resp_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":null}]}\n\n",
            "data: [DONE]\n\n",
        ),
        concat!(
            "data: {\"id\":\"resp_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"\"}]}\n\n",
            "data: [DONE]\n\n",
        ),
    ] {
        let ev = terminal(Wire::Mistral, raw).await;
        match &ev {
            StreamEvent::Error { error, .. } => {
                assert_eq!(error.stop_reason, StopReason::Error);
                assert_eq!(
                    error.error_message.as_deref(),
                    Some(Wire::Mistral.truncated_diagnostic())
                );
            }
            other => panic!("falsy finishReason must not settle the turn, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The `pending` half of PROV-010 / AGENT-014 / DRIFT-012: the in-flight sentinel.
// ---------------------------------------------------------------------------

/// Every converter's IN-FLIGHT `partial` must report Pi's `"pending"` sentinel, not a fabricated
/// `"stop"`. Pi seeds `output.stopReason = "pending"` and attaches that same mutable `output` to
/// every non-terminal event (anthropic-messages.ts:509, google-generative-ai.ts:73,
/// openai-responses.ts:124, openai-completions.ts:218, mistral-conversations.ts:153), so a consumer
/// reading `message_start` / `message_update` off cyrup's event stream — an extension guest
/// (`cyrup-ext/src/event.rs`), the RPC/SDK fan-out, or a Pi-shaped client — used to be told the
/// turn had completed cleanly before a single token arrived.
///
/// Asserted on the SERIALIZED bytes, because the wire spelling is the whole point.
#[tokio::test]
async fn in_flight_partials_report_pending_on_the_wire() {
    for wire in Wire::ALL {
        // Use the *complete* transcript: this is about healthy in-flight events, not truncation.
        let evs = events(wire, wire.complete()).await;
        let non_terminal: Vec<_> = evs.iter().filter_map(StreamEvent::partial).collect();
        assert!(
            !non_terminal.is_empty(),
            "{wire:?}: no non-terminal events to inspect"
        );

        // The FIRST partial is always pre-stop-reason, for every api.
        let first = non_terminal[0];
        assert_eq!(
            first.stop_reason,
            StopReason::Pending,
            "{wire:?}: the seed partial claims a settled outcome"
        );
        let json = serde_json::to_value(first).unwrap();
        assert_eq!(
            json["stopReason"], "pending",
            "{wire:?}: wire spelling must be Pi's `\"pending\"`"
        );

        // On the TRUNCATED twin the provider never delivers a stop reason at all, so no partial may
        // EVER claim one — this is the strong form, and all five must agree on it. (The complete
        // twin cannot be asserted this way: Google's fixture carries its text and its
        // `finishReason` in a single chunk, so its later partials legitimately settle mid-stream,
        // exactly as Pi's `output.stopReason` does once the candidate is processed.)
        let truncated = events(wire, wire.truncated()).await;
        let truncated_partials: Vec<_> =
            truncated.iter().filter_map(StreamEvent::partial).collect();
        assert!(
            !truncated_partials.is_empty(),
            "{wire:?}: truncated fixture produced no non-terminal events"
        );
        for (i, p) in truncated_partials.iter().enumerate() {
            assert_eq!(
                p.stop_reason,
                StopReason::Pending,
                "{wire:?}: partial #{i} of a stream that NEVER delivered a stop reason claims {:?}",
                p.stop_reason
            );
        }
    }
}

/// `Pending` must be structurally incapable of reaching a `done` event. This is the invariant that
/// makes the variant safe to add: Pi enforces it with a `throw`, cyrup with these two seams.
#[test]
fn pending_can_never_reach_a_done_terminal() {
    // 1. The narrowing itself refuses it, with `error` (not `aborted`) — Pi's catch sets
    //    `output.stopReason = "error"` for the non-abort throw (anthropic-messages.ts:765).
    assert_eq!(
        DoneReason::try_from(StopReason::Pending),
        Err(ErrorReason::Error)
    );

    // 2. `terminal()` normalizes rather than propagating, so a `Pending` value cannot survive into
    //    `message_end` / the settled transcript / a session file even if a caller bypasses
    //    `end_of_stream` entirely.
    let mut msg = cyrup_core::AssistantMessage::errored(
        "p".into(),
        "m",
        Some(ApiId::from("a")),
        StopReason::Pending,
        "",
    );
    msg.error_message = None;
    match StreamEvent::terminal(msg) {
        StreamEvent::Error { reason, error } => {
            assert_eq!(reason, ErrorReason::Error);
            assert_eq!(
                error.stop_reason,
                StopReason::Error,
                "a terminal must never carry the in-flight sentinel"
            );
            assert_eq!(
                error.error_message.as_deref(),
                Some(crate::stream::PENDING_AT_TERMINAL),
                "the normalization must leave a diagnostic, not swallow the bug"
            );
        }
        other => panic!("expected error terminal, got {other:?}"),
    }

    // 3. …but a diagnostic the decoder already recorded is NOT clobbered.
    let mut msg = cyrup_core::AssistantMessage::errored(
        "p".into(),
        "m",
        Some(ApiId::from("a")),
        StopReason::Pending,
        "",
    );
    msg.error_message = Some("Anthropic stream ended without a stop reason".into());
    let ev = StreamEvent::terminal(msg);
    assert_eq!(
        ev.terminal_message().unwrap().error_message.as_deref(),
        Some("Anthropic stream ended without a stop reason")
    );
}

/// `end_of_stream` must treat an explicit `Some(Pending)` exactly like `None`. Pi's guard is a
/// VALUE test on the sentinel (`output.stopReason === "pending"`), not a "was anything assigned"
/// test — `openai_responses` tracks its reason as a plain `StopReason`, so this is the arm that
/// keeps it honest.
#[test]
fn end_of_stream_treats_an_explicit_pending_like_no_reason_at_all() {
    let base = cyrup_core::AssistantMessage::errored(
        "p".into(),
        "m",
        Some(ApiId::from("a")),
        StopReason::Stop,
        "",
    );
    for delivered in [None, Some(StopReason::Pending)] {
        match StreamEvent::end_of_stream(base.clone(), delivered, "boom") {
            StreamEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Error, "{delivered:?}");
                assert_eq!(error.stop_reason, StopReason::Error, "{delivered:?}");
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("boom"),
                    "{delivered:?}"
                );
            }
            other => panic!("{delivered:?}: expected error terminal, got {other:?}"),
        }
    }
}

/// Adding a variant changes a serialized shape, so pin BOTH directions of the wire contract.
///
/// - the five pre-existing values still serialize to the exact bytes an OLD session JSONL holds,
///   so an old transcript still loads (the change is purely additive);
/// - `"pending"` now DESERIALIZES, which is the interop gap that motivated the variant: a
///   Pi-produced `message_start` payload (agent-loop.ts:314-318 emits `{...partialMessage}`, whose
///   `stopReason` is the `"pending"` seed) previously failed to parse.
#[test]
fn stop_reason_wire_shape_is_additive_and_accepts_pi_pending() {
    for (reason, wire) in [
        (StopReason::Pending, "pending"),
        (StopReason::Stop, "stop"),
        (StopReason::Length, "length"),
        (StopReason::ToolUse, "toolUse"),
        (StopReason::Error, "error"),
        (StopReason::Aborted, "aborted"),
    ] {
        assert_eq!(
            serde_json::to_value(reason).unwrap(),
            wire,
            "serialize {reason:?}"
        );
        let back: StopReason = serde_json::from_value(serde_json::json!(wire)).unwrap();
        assert_eq!(back, reason, "deserialize {wire:?}");
    }

    // A whole Pi-shaped `message_start` message payload round-trips.
    let pi_partial = serde_json::json!({
        "role": "assistant",
        "stopReason": "pending",
        "content": [],
        "api": "anthropic-messages",
        "provider": "anthropic",
        "model": "claude-x",
        "usage": {
            "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
            "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
        },
        "timestamp": 0
    });
    let parsed: cyrup_core::AssistantMessage = serde_json::from_value(pi_partial).unwrap();
    assert_eq!(parsed.stop_reason, StopReason::Pending);

    // And an unknown FUTURE value is still rejected rather than silently absorbed — the deliberate
    // absence of `#[serde(other)]`. See the `StopReason` docs for why.
    assert!(serde_json::from_value::<StopReason>(serde_json::json!("someNewReason")).is_err());
}

/// `StreamEvent::end_of_stream` is the single seam the rule lives in; pin its contract directly so a
/// future converter author sees the intent even if they never read a decoder.
#[test]
fn end_of_stream_maps_no_delivered_reason_to_the_error_terminal() {
    let base = cyrup_core::AssistantMessage::errored(
        "p".into(),
        "m",
        Some(ApiId::from("a")),
        StopReason::Stop,
        "",
    );

    // No stop reason delivered → error terminal carrying the diagnostic.
    match StreamEvent::end_of_stream(base.clone(), None, "boom") {
        StreamEvent::Error { reason, error } => {
            assert_eq!(reason, ErrorReason::Error);
            assert_eq!(error.stop_reason, StopReason::Error);
            assert_eq!(error.error_message.as_deref(), Some("boom"));
        }
        other => panic!("expected error terminal, got {other:?}"),
    }

    // A delivered reason is used verbatim and the diagnostic is NOT applied.
    for (delivered, want_done) in [
        (StopReason::Stop, true),
        (StopReason::Length, true),
        (StopReason::ToolUse, true),
        (StopReason::Error, false),
        (StopReason::Aborted, false),
    ] {
        let ev = StreamEvent::end_of_stream(base.clone(), Some(delivered), "boom");
        let msg = ev.terminal_message().expect("terminal");
        assert_eq!(msg.stop_reason, delivered);
        assert_ne!(
            msg.error_message.as_deref(),
            Some("boom"),
            "a delivered reason must not be overwritten by the truncation diagnostic"
        );
        assert_eq!(matches!(ev, StreamEvent::Done { .. }), want_done);
    }
}
