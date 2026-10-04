//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! the decoder state and the SSE frame loop.

use super::blocks::{RBlock, project_block};
use super::errors::emit_error;
use super::events::{ProcessResult, process_event};
use super::slots::SlotKind;
use crate::api::EventSink;
use crate::api::content_cache::ContentCache;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::stream::sse::SseFrame;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{ApiId, AssistantMessage, StopReason, Usage};
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Pi `output.endTurn` as `openai-codex-responses` writes it: `mapCodexEvents` sets it from the
/// terminal `response.end_turn` on the very `output` object the shared `processResponsesStream`
/// fills (`openai-codex-responses.ts:748-752` @v0.87.1). The shared Responses path itself never
/// reads the key, so only the Codex adapter hands the decoder a cell (DRIFT-059).
pub(crate) type EndTurnCell = Arc<OnceLock<bool>>;

pub(super) struct RDecoder {
    pub(super) blocks: Vec<RBlock>,
    /// Memoised projection of `blocks` (PERF-001). Write to `blocks` ONLY through
    /// [`Self::push_block`] and [`Self::block_mut`], or this goes stale.
    cache: ContentCache,
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
    /// The Codex `end_turn` cell, read into every snapshot — `None` on the plain Responses and Azure
    /// routes, which never record one.
    end_turn: Option<EndTurnCell>,
}

impl Default for RDecoder {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            cache: ContentCache::default(),
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
            end_turn: None,
        }
    }
}

impl RDecoder {
    /// Append a block. The ONLY push — it keeps [`ContentCache`] in step (PERF-001).
    pub(super) fn push_block(&mut self, block: RBlock) {
        self.blocks.push(block);
        self.cache.push();
    }

    /// The ONLY `&mut RBlock`. Invalidates exactly that block's memo (PERF-001).
    pub(super) fn block_mut(&mut self, pos: usize) -> Option<&mut RBlock> {
        self.cache.invalidate(pos);
        self.blocks.get_mut(pos)
    }

    /// The content projection, recomputing only the blocks whose memo was invalidated.
    fn content(&mut self) -> Vec<cyrup_core::Content> {
        let (cache, blocks) = (&mut self.cache, &self.blocks);
        cache.project(blocks, project_block)
    }

    /// The live `partial`, as a SHARED handle (PERF-001).
    ///
    /// Every non-terminal event carries this message and it is then cloned again by the
    /// agent loop, by `MessageUpdate`, and once per live subscriber. Handing out an `Arc`
    /// turns those into refcount bumps; the wire bytes are unchanged because serde's `rc`
    /// feature serializes an `Arc<T>` transparently as `T`.
    pub(super) fn snapshot(&mut self, model: &Model, api: &ApiId) -> Arc<AssistantMessage> {
        Arc::new(self.snapshot_owned(model, api))
    }

    /// The same message, owned, for the terminal paths that stamp a stop reason onto it
    /// before handing it to [`StreamEvent::terminal`]/[`StreamEvent::end_of_stream`].
    pub(super) fn snapshot_owned(&mut self, model: &Model, api: &ApiId) -> AssistantMessage {
        AssistantMessage {
            content: self.content(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            provider_thinking_level: None,
            diagnostics: None,
            usage: self.usage.clone(),
            stop_reason: self.stop_reason,
            deferred: None,
            // Pi mutates the single `output` object the `partial` frames alias, so whatever
            // `finalizeResponse` wrote into `errorMessage`/`rawStopReason` is visible on the
            // snapshot too (v0.84.1 `openai-responses-shared.ts:566-573`).
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            end_turn: self.end_turn.as_ref().and_then(|cell| cell.get().copied()),
            timestamp: now_millis(),
        }
    }

    /// PROV-116 — the first tool block whose `output_item.done` never arrived, as
    /// `(name, wire call id)` (Pi `openai-responses-shared.ts:765-774`).
    ///
    /// Upstream iterates `output.content` in order and throws on the first offender, so the first
    /// such block is the one named in the message. The id is the one the projected [`ToolCall`]
    /// carries, which is `"{call_id}|{item_id}"` — the same `toolCall.id` upstream interpolates,
    /// since pi's block id is likewise the composed wire id (`openai-responses-shared.ts:488`).
    ///
    /// [`ToolCall`]: cyrup_core::ToolCall
    pub(super) fn first_unfinished_tool_call(&self) -> Option<(String, String)> {
        self.blocks.iter().find_map(|b| match b {
            RBlock::Tool {
                call_id,
                item_id,
                name,
                finished: false,
                ..
            } => Some((name.clone(), format!("{call_id}|{item_id}"))),
            _ => None,
        })
    }

    pub(super) fn slot(&self, output_index: i64, kind: SlotKind) -> Option<usize> {
        self.slots
            .get(&output_index)
            .filter(|(_, k)| *k == kind)
            .map(|(i, _)| *i)
    }
}

/// Drive the Responses SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi's stream loop).
pub(crate) async fn decode_stream<S>(frames: S, model: &Model, api: &ApiId, sink: &EventSink)
where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    decode_stream_with_end_turn(frames, model, api, sink, None).await;
}

/// [`decode_stream`] with the Codex `end_turn` cell: whatever the Codex event mapper records in it
/// before yielding the terminal frame appears on every later snapshot, the terminal message
/// included, exactly as pi's shared `output` object carries it (DRIFT-059).
pub(crate) async fn decode_stream_with_end_turn<S>(
    mut frames: S,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    end_turn: Option<EndTurnCell>,
) where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    let mut dec = RDecoder {
        end_turn,
        ..RDecoder::default()
    };
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
                &mut dec,
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
                emit_error(&mut dec, model, api, sink, msg).await;
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

    // PROV-116 — `1b2aa0ca0` ("reject unfinished Responses tool calls instead of running them",
    // #9974). The agent runs every tool call in the final message, so a call whose
    // `output_item.done` never arrived must not be handed over: its arguments may be cut off, or
    // merged with another call's when a non-compliant server omits `output_index`
    // (`openai-responses-shared.ts:764-775`):
    //
    //     if (output.stopReason === "toolUse") {
    //         for (const block of output.content) { … if (toolCall.partialJson !== undefined …)
    //             throw new Error(`OpenAI Responses stream completed with an unfinished tool call: …`);
    //
    // Ordering matches upstream: this sits AFTER the terminal-event check, so a stream that was
    // truncated before `response.completed` still reports the truncation (`:760-762`) rather than
    // the unfinished call. `dec.stop_reason` is only `ToolUse` once `finalize_response` has run,
    // which is exactly upstream's `sawTerminalResponseEvent` precondition.
    if dec.saw_terminal
        && dec.stop_reason == StopReason::ToolUse
        && let Some((name, id)) = dec.first_unfinished_tool_call()
    {
        emit_error(
            &mut dec,
            model,
            api,
            sink,
            format!(
                "OpenAI Responses stream completed with an unfinished tool call: {name} ({id})"
            ),
        )
        .await;
        return;
    }

    let mut message = dec.snapshot_owned(model, api);
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
