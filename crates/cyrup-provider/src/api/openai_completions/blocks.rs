//! Response decoding: the in-progress block set and the decoder's accumulated state.

use crate::api::content_cache::ContentCache;
use crate::model::Model;
use crate::usage::apply_cost;
use crate::utils::constrained_sampling::{ConstrainedSamplingError, CustomToolInput};
use cyrup_core::{
    ApiId, AssistantMessage, Content, LazyArgs, SharedStr, StopReason, ToolCall, ToolCallId, Usage,
};
use std::collections::HashMap;
use std::sync::Arc;

/// One in-progress content block, in first-appearance order (its index == `content_index`).
pub(super) enum Block {
    /// Shared with every snapshot taken from it, so projecting this block is a refcount bump
    /// rather than a copy of everything streamed so far (PERF-001).
    Text(SharedStr),
    Thinking {
        text: SharedStr,
        signature: Option<String>,
    },
    Tool {
        id: String,
        name: String,
        /// The raw accumulated argument buffer. [`LazyArgs`] recovers the `Map` from it on the
        /// first read, so a snapshot nobody reads never parses it at all (PERF-001).
        args: SharedStr,
        thought_signature: Option<String>,
        /// `Some` for a grammar-constrained (`custom`) call (PROV-101). Its `args` is then the JSON
        /// form of `{ [property]: input }`, kept in step by [`Decoder::append_custom_input`], and
        /// `function.arguments` fragments no longer apply to it.
        custom: Option<CustomToolInput>,
    },
}

/// Streaming-decode state.
#[derive(Default)]
pub(super) struct Decoder {
    /// Wall-clock start of this response — the `timestamp` of every message it produces (pi seeds
    /// `output.timestamp = Date.now()` once, before the request, and the v1.1.0 type documents it
    /// as *"when the request started"*). Set from [`crate::api::EventSink::started_at`] by the
    /// driver; `0` only in a decoder a unit test built by hand.
    pub(super) started_at: i64,
    pub(super) blocks: Vec<Block>,
    /// Memoised projection of `blocks` (PERF-001). Write to `blocks` ONLY through
    /// [`Self::push_block`] and [`Self::block_mut`], or this goes stale.
    cache: ContentCache,
    pub(super) text_idx: Option<usize>,
    pub(super) thinking_idx: Option<usize>,
    pub(super) tool_by_stream: HashMap<i64, usize>,
    pub(super) tool_by_id: HashMap<String, usize>,
    /// The merged `reasoning_details` seen so far (Pi `streamedReasoningDetails`,
    /// openai-completions.ts:328). Replay metadata, not a user-visible delta: it is serialized once
    /// onto the thinking block's signature when that block is finalized
    /// (`applyStreamedReasoningDetails`, openai-completions.ts:329-333).
    pub(super) streamed_reasoning_details: Option<Vec<serde_json::Value>>,
    pub(super) usage: Option<Usage>,
    pub(super) response_id: Option<String>,
    pub(super) response_model: Option<String>,
    pub(super) stop_reason: Option<StopReason>,
    /// The choice's own `finish_reason`, kept verbatim beside the narrowed [`StopReason`] (pi
    /// `output.rawStopReason = choice.finish_reason`,
    /// `v0.84.1 ai/src/api/openai-completions.ts:463`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too (`v0.83.0 ai/src/api/openai-completions.ts:459`) and cyrup never
    /// ported it. This is the widest-reach one of the five — `openai-completions` is the fleet wire
    /// api shared by 16 built-in providers (`providers/fleet.rs`), so it carried the gap for xAI,
    /// Groq, DeepSeek, Moonshot and the rest.
    pub(super) raw_stop_reason: Option<String>,
    pub(super) error_message: Option<String>,
    /// Whether any chunk carried a `finish_reason` (Pi `hasFinishReason`). A stream that ends
    /// without one is a protocol error (Pi openai-completions.ts:452-454).
    pub(super) saw_finish_reason: bool,
    /// Pi `grammarToolInputProperties` (`openai-completions.ts:162`): tool name → the argument
    /// property a `custom` tool call's raw text is stored under (PROV-101).
    pub(super) grammar_inputs: HashMap<String, String>,
}

impl Decoder {
    /// Append a block. The ONLY push — it keeps [`ContentCache`] in step (PERF-001).
    pub(super) fn push_block(&mut self, block: Block) {
        self.blocks.push(block);
        self.cache.push();
    }

    /// The ONLY `&mut Block`. Invalidates exactly that block's memo (PERF-001).
    pub(super) fn block_mut(&mut self, pos: usize) -> Option<&mut Block> {
        self.cache.invalidate(pos);
        self.blocks.get_mut(pos)
    }

    /// Pi `appendCustomToolCallInput` on the block at `pos` (`openai-completions.ts:414-430`):
    /// record `next_input` as the call's raw text and return the JSON delta it adds. `Ok(None)`
    /// for a block that is not a custom call.
    pub(super) fn append_custom_input(
        &mut self,
        pos: usize,
        next_input: &str,
        close: bool,
    ) -> Result<Option<String>, ConstrainedSamplingError> {
        match self.block_mut(pos) {
            Some(Block::Tool {
                args,
                custom: Some(custom),
                ..
            }) => custom.append_into(args, next_input, close),
            _ => Ok(None),
        }
    }

    /// The raw text a custom call holds so far (pi `getCustomToolCallInput`); `None` when the block
    /// at `pos` is not a custom call.
    pub(super) fn custom_input(&self, pos: usize) -> Option<String> {
        match self.blocks.get(pos) {
            Some(Block::Tool {
                custom: Some(custom),
                ..
            }) => Some(custom.input().to_string()),
            _ => None,
        }
    }

    /// The content projection, recomputing only the blocks whose memo was invalidated.
    fn content(&mut self) -> Vec<Content> {
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
    /// before handing it to [`StreamEvent::terminal`](crate::stream::StreamEvent::terminal)/
    /// [`StreamEvent::end_of_stream`](crate::stream::StreamEvent::end_of_stream).
    ///
    /// Pi `output`, the mutated `AssistantMessage` attached to every non-terminal event
    /// (openai-completions.ts:158-175 + `partial: output`). Mirrors
    /// [`build_final_message`](super::finalize::build_final_message) but borrows: the stream is
    /// still in progress, so `stop_reason` is the in-flight sentinel until a `finish_reason`
    /// arrives — Pi seeds `output.stopReason = "pending"` (openai-completions.ts:218) and attaches
    /// that same `output` as every non-terminal event's `partial`.
    pub(super) fn snapshot_owned(&mut self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone().unwrap_or_default();
        apply_cost(&model.cost, &mut usage);
        AssistantMessage {
            content: self.content(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: self.response_model.clone(),
            response_id: self.response_id.clone(),
            provider_thinking_level: None,
            thinking_level: None,
            diagnostics: None,
            usage,
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            end_turn: None,
            timestamp: self.started_at,
            duration_ms: None,
        }
    }
}

/// Convert in-progress decoder blocks to content (shared by the live `partial` snapshot and the
/// terminal message). Tool args are parsed best-effort (`{}` for incomplete/invalid JSON).
pub(super) fn project_block(block: &Block) -> Content {
    {
        match block {
            Block::Text(text) => Content::text(text.clone()),
            Block::Thinking { text, signature } => Content::Thinking {
                thinking: text.clone(),
                thinking_signature: signature.clone(),
                redacted: false,
            },
            Block::Tool {
                id,
                name,
                args,
                thought_signature,
                custom: _,
            } => Content::ToolCall(ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                // A handle on the buffer, not a parse of it (PERF-001).
                arguments: LazyArgs::streaming(args.clone()),
                thought_signature: thought_signature.clone(),
                namespace: None,
            }),
        }
    }
}
