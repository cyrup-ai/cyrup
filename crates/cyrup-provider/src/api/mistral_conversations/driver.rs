//! Response decoding — the SSE frame loop (1:1 with Pi `consumeChatStream`,
//! mistral-conversations.ts:295-483).

use super::blocks::{close_current, finalize_tool_blocks};
use super::content::process_chunk;
use super::decoder::Decoder;
use super::finish::emit_error;
use crate::api::EventSink;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::stream::sse::SseFrame;
use crate::utils::json_parse::parse_json_with_repair;
use cyrup_core::{ApiId, StopReason};
use futures::{Stream, StreamExt};

/// Drive the Mistral SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi `consumeChatStream`,
/// mistral-conversations.ts:295-483).
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
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Some(chunk) = parse_json_with_repair(data) else {
            emit_error(
                &mut dec,
                model,
                api,
                sink,
                "Could not parse Mistral SSE chunk".to_string(),
            )
            .await;
            return;
        };
        if !process_chunk(&chunk, &mut dec, model, api, sink).await {
            return; // consumer dropped
        }
    }

    // Close a trailing text/thinking block, then finalize every tool block (Pi
    // mistral-conversations.ts:467-482).
    if !close_current(&mut dec, model, api, sink).await {
        return;
    }
    if !finalize_tool_blocks(&mut dec, model, api, sink).await {
        return;
    }

    if matches!(
        dec.stop_reason,
        Some(StopReason::Aborted) | Some(StopReason::Error)
    ) {
        // Lifted out first: `emit_error` takes `&mut dec` (its snapshot memoises), which cannot
        // overlap a read of `dec.error_message` in the same call.
        let message = dec
            .error_message
            .clone()
            .unwrap_or_else(|| "An unknown error occurred".to_string());
        emit_error(&mut dec, model, api, sink, message).await;
        return;
    }

    // No chunk carried a truthy `finishReason` → TRUNCATED. Pi throws
    // "Mistral stream ended without a finish reason" (mistral-conversations.ts:88-90).
    sink.send(StreamEvent::end_of_stream(
        dec.snapshot_owned(model, api),
        dec.stop_reason,
        "Mistral stream ended without a finish reason",
    ))
    .await;
}
