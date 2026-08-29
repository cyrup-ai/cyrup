//! Response decoding — the SSE stream driver (Pi's stream loop).

use super::blocks::Decoder;
use super::events::{
    emit_error, finish_blocks, process_block_delta, process_block_start, process_block_stop,
};
use super::stop_reason::map_stop_reason;
use super::usage::{apply_message_delta_usage, apply_message_start_usage};
use crate::api::EventSink;
use crate::context::ToolDef;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::stream::sse::SseFrame;
use crate::utils::json_parse::parse_json_with_repair;
use cyrup_core::{ApiId, StopReason};
use futures::{Stream, StreamExt};
use serde_json::Value;

/// Drive the Anthropic SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi's stream loop,
/// anthropic-messages.ts:546-737).
pub(crate) async fn decode_stream<S>(
    mut frames: S,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    is_oauth: bool,
    tools: &[ToolDef],
) where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    let mut dec = Decoder::new(is_oauth, tools.iter().map(|t| t.name.clone()).collect());
    if !sink
        .send(StreamEvent::Start {
            partial: dec.snapshot(model, api),
        })
        .await
    {
        return;
    }

    while let Some(frame) = frames.next().await {
        let frame = match frame {
            Ok(f) => f,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        // An `event: error` frame surfaces the data as an error (Pi anthropic-messages.ts:439-441).
        if frame.event == "error" {
            let msg = if frame.data.trim().is_empty() {
                "Anthropic stream error".to_string()
            } else {
                frame.data.clone()
            };
            emit_error(&mut dec, model, api, sink, msg).await;
            return;
        }
        if !is_message_event(&frame.event) {
            continue;
        }
        let data = frame.data.trim();
        if data.is_empty() {
            continue;
        }
        let Some(event) = parse_json_with_repair(data) else {
            emit_error(
                &mut dec,
                model,
                api,
                sink,
                format!("Could not parse Anthropic SSE event {}", frame.event),
            )
            .await;
            return;
        };
        if !process_event(&event, &mut dec, model, api, sink).await {
            return; // consumer dropped
        }
        if dec.stop_reason == Some(StopReason::Error) {
            // The message is lifted out first: `emit_error` now takes `&mut dec` (its snapshot
            // memoises), which cannot overlap a read of `dec.error_message` in the same call.
            let message = dec
                .error_message
                .clone()
                .unwrap_or_else(|| "An unknown error occurred".to_string());
            emit_error(&mut dec, model, api, sink, message).await;
            return;
        }
    }

    // Stream ended. A `message_start` with no `message_stop` is a protocol error (Pi
    // anthropic-messages.ts:463-465).
    if dec.saw_message_start && !dec.saw_message_stop {
        emit_error(
            &mut dec,
            model,
            api,
            sink,
            "Anthropic stream ended before message_stop".to_string(),
        )
        .await;
        return;
    }

    finish_blocks(&dec, model, api, sink).await;
    // A stream that ran to EOF without a `message_delta.stop_reason` is TRUNCATED, not complete.
    // `dec.stop_reason == None` is cyrup's spelling of Pi's still-`"pending"` output, and
    // `end_of_stream` turns it into the same `error` terminal Pi's throw produces
    // (anthropic-messages.ts:751-753) instead of the clean `stop` this used to default to.
    sink.send(StreamEvent::end_of_stream(
        dec.snapshot_owned(model, api),
        dec.stop_reason,
        "Anthropic stream ended without a stop reason",
    ))
    .await;
}

/// Whether `event` is one of the six Anthropic message events (Pi `ANTHROPIC_MESSAGE_EVENTS`).
fn is_message_event(event: &str) -> bool {
    matches!(
        event,
        "message_start"
            | "message_delta"
            | "message_stop"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
    )
}

/// Process one decoded Anthropic event. Returns `false` if the consumer dropped the stream.
async fn process_event(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    match event.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            dec.saw_message_start = true;
            if let Some(message) = event.get("message") {
                if let Some(id) = message.get("id").and_then(Value::as_str) {
                    dec.response_id = Some(id.to_string());
                }
                if let Some(usage) = message.get("usage") {
                    apply_message_start_usage(&mut dec.usage, usage);
                }
            }
            true
        }
        Some("content_block_start") => process_block_start(event, dec, model, api, sink).await,
        Some("content_block_delta") => process_block_delta(event, dec, model, api, sink).await,
        Some("content_block_stop") => process_block_stop(event, dec, model, api, sink).await,
        Some("message_delta") => {
            // pi guards with `if (event.delta.stop_reason)` (`v0.84.1
            // ai/src/api/anthropic-messages.ts:708`) — a JS truthiness test, so `""` leaves the
            // `"pending"` seed alone rather than mapping to `Unhandled stop reason: `.
            if let Some(delta) = event.get("delta")
                && let Some(reason) = delta
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .filter(|r| !r.is_empty())
            {
                // The raw string is recorded FIRST and unconditionally, exactly where pi records it
                // (`v0.84.1 ai/src/api/anthropic-messages.ts:709`), so a turn that maps to
                // `tool_use`/`refusal`/an unknown reason still carries the provider's own word.
                dec.raw_stop_reason = Some(reason.to_string());
                let (stop, err) = map_stop_reason(reason, delta.get("stop_details"));
                dec.stop_reason = Some(stop);
                if let Some(err) = err {
                    dec.error_message = Some(err);
                }
            }
            if let Some(usage) = event.get("usage") {
                apply_message_delta_usage(&mut dec.usage, usage);
            }
            true
        }
        Some("message_stop") => {
            dec.saw_message_stop = true;
            true
        }
        _ => true,
    }
}
