//! Response decoding — the SSE frame loop (1:1 with Pi's stream loop,
//! google-generative-ai.ts:88-265).

use crate::api::EventSink;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::SseFrame;
use crate::stream::StreamEvent;
use crate::utils::json_parse::parse_json_with_repair;
use cyrup_core::{ApiId, StopReason};
use futures::{Stream, StreamExt};
use super::decoder::Decoder;
use super::finish::{close_current, emit_error};
use super::parts::process_chunk;

/// Drive the Gemini SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi's stream loop,
/// google-generative-ai.ts:88-265).
pub(crate) async fn decode_stream<S>(mut frames: S, model: &Model, api: &ApiId, sink: &EventSink)
where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    let mut dec = Decoder::default();
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
        let data = frame.data.trim();
        if data.is_empty() {
            continue;
        }
        let Some(chunk) = parse_json_with_repair(data) else {
            emit_error(
                &dec,
                model,
                api,
                sink,
                "Could not parse Gemini SSE chunk".to_string(),
            )
            .await;
            return;
        };
        if !process_chunk(&chunk, &mut dec, model, api, sink).await {
            return; // consumer dropped
        }
    }

    // Close a trailing in-progress block (Pi google-generative-ai.ts:238-254).
    if !close_current(&mut dec, model, api, sink).await {
        return;
    }

    if matches!(
        dec.stop_reason,
        Some(StopReason::Aborted) | Some(StopReason::Error)
    ) {
        emit_error(
            &dec,
            model,
            api,
            sink,
            dec.error_message
                .clone()
                .unwrap_or_else(|| "An unknown error occurred".to_string()),
        )
        .await;
        return;
    }

    // No candidate ever carried a `finishReason` → the stream was TRUNCATED. Pi throws
    // "Google stream ended without a finish reason" (google-generative-ai.ts:266-268); this used to
    // fall through to the `Stop` seed and report a clean turn.
    sink.send(StreamEvent::end_of_stream(
        dec.snapshot(model, api),
        dec.stop_reason,
        "Google stream ended without a finish reason",
    ))
    .await;
}
