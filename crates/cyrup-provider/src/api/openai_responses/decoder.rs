//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! the decoder state and the SSE frame loop.

use super::blocks::{RBlock, blocks_to_content};
use super::errors::emit_error;
use super::events::{ProcessResult, process_event};
use super::slots::SlotKind;
use crate::api::EventSink;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::SseFrame;
use crate::stream::StreamEvent;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{ApiId, AssistantMessage, StopReason, Usage};
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::collections::HashMap;

pub(super) struct RDecoder {
    pub(super) blocks: Vec<RBlock>,
    /// Active output-index → (block position, kind). Removed on `output_item.done`.
    pub(super) slots: HashMap<i64, (usize, SlotKind)>,
    pub(super) usage: Usage,
    pub(super) response_id: Option<String>,
    pub(super) stop_reason: StopReason,
    /// Pi's `output.errorMessage` (v0.84.1 `ai/src/types.ts:425`). Written by `finalizeResponse`
    /// from `mapStopReason(...).errorMessage` (v0.84.1 `openai-responses-shared.ts:573`), so a
    /// terminal that settles on `Error` carries the provider's reason instead of nothing.
    pub(super) error_message: Option<String>,
    /// Pi's `output.rawStopReason` (`ai/src/types.ts:426`) — the provider's own status string,
    /// stamped on **every** settled turn by v0.84.1 `openai-responses-shared.ts:570` and `:726`.
    /// Present since `v0.83.0 ai/src/types.ts:411` / `openai-responses-shared.ts:567,721`, so its
    /// absence here was a PORT BUG at the ported baseline, not version lag.
    pub(super) raw_stop_reason: Option<String>,
    pub(super) saw_terminal: bool,
}

impl Default for RDecoder {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            slots: HashMap::new(),
            usage: Usage::default(),
            response_id: None,
            // Pi's seed is `"pending"` (openai-responses.ts:124); only a terminal `response.*`
            // event overwrites it (and sets `saw_terminal`). Seeding `Stop` made the in-flight
            // `partial` claim a completed turn.
            stop_reason: StopReason::Pending,
            error_message: None,
            raw_stop_reason: None,
            saw_terminal: false,
        }
    }
}

impl RDecoder {
    pub(super) fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        AssistantMessage {
            content: blocks_to_content(&self.blocks),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage: self.usage.clone(),
            stop_reason: self.stop_reason,
            deferred: None,
            // Pi mutates the single `output` object the `partial` frames alias, so whatever
            // `finalizeResponse` wrote into `errorMessage`/`rawStopReason` is visible on the
            // snapshot too (v0.84.1 `openai-responses-shared.ts:566-573`).
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            timestamp: now_millis(),
        }
    }

    pub(super) fn slot(&self, output_index: i64, kind: SlotKind) -> Option<usize> {
        self.slots
            .get(&output_index)
            .filter(|(_, k)| *k == kind)
            .map(|(i, _)| *i)
    }
}

/// Drive the Responses SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi's stream loop).
pub(crate) async fn decode_stream<S>(mut frames: S, model: &Model, api: &ApiId, sink: &EventSink)
where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    let mut dec = RDecoder::default();
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
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            emit_error(
                &dec,
                model,
                api,
                sink,
                "Could not parse OpenAI Responses SSE event".into(),
            )
            .await;
            return;
        };
        match process_event(&event, &mut dec, model, api, sink).await {
            ProcessResult::Continue => {}
            ProcessResult::Dropped => return,
            ProcessResult::Error(msg) => {
                emit_error(&dec, model, api, sink, msg).await;
                return;
            }
        }
    }

    // `saw_terminal` is this decoder's spelling of "the provider delivered a stop reason": only a
    // terminal `response.*` event sets `dec.stop_reason`, so without one the seeded `Stop` is a
    // guess. Routed through the same `end_of_stream` seam as the other four wire APIs so the
    // truncated-stream rule lives in exactly one place (Pi openai-responses.ts:170-172).
    //
    // A settled `error` reason is *thrown* upstream as
    // `throw new Error(output.errorMessage || "An unknown error occurred")` (v0.84.1
    // `openai-responses.ts:174`, and identically for the Azure sibling that shares this decoder at
    // v0.84.1 `azure-openai-responses.ts:139`), and the catch stamps that text back onto
    // `output.errorMessage` (`:188`) — so no Pi build can emit an `error` terminal whose
    // `errorMessage` is unset. `end_of_stream` only fills the message on the truncated branch, so
    // the fallback is applied here, the same guard `bedrock_converse_stream.rs:454-457` carries.
    // The `|| "An unknown error occurred"` fallback dates to `v0.83.0 openai-responses.ts:174`
    // (unconditional there), so this is a PORT BUG at the ported baseline, not version lag.
    let mut message = dec.snapshot(model, api);
    if dec.saw_terminal && dec.stop_reason == StopReason::Error && message.error_message.is_none() {
        message.error_message = Some("An unknown error occurred".to_string());
    }
    sink.send(StreamEvent::end_of_stream(
        message,
        dec.saw_terminal.then_some(dec.stop_reason),
        "OpenAI Responses stream ended before a terminal response event",
    ))
    .await;
}
