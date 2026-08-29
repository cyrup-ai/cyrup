//! Response decoding — in-progress block and decoder state.

use crate::model::Model;
use crate::usage::apply_cost;
use crate::api::content_cache::ContentCache;
use crate::utils::provider_plumbing::now_millis;
use std::sync::Arc;
use cyrup_core::{
    ApiId, AssistantMessage, Content, LazyArgs, SharedStr, StopReason, ToolCall, ToolCallId, Usage,
};

/// One in-progress content block, keyed by the Anthropic `index`.
pub(super) enum Block {
    Text {
        index: i64,
        /// Shared with every snapshot taken from it, so projecting this block is a refcount bump
        /// rather than a copy of everything streamed so far (PERF-001).
        text: SharedStr,
    },
    Thinking {
        index: i64,
        thinking: SharedStr,
        signature: String,
        redacted: bool,
    },
    Tool {
        index: i64,
        id: String,
        name: String,
        /// The raw accumulated argument buffer. [`LazyArgs`] recovers the `Map` from it on the
        /// first read, so a snapshot nobody reads never parses it at all (PERF-001).
        partial_json: SharedStr,
    },
}

impl Block {
    fn index(&self) -> i64 {
        match self {
            Block::Text { index, .. }
            | Block::Thinking { index, .. }
            | Block::Tool { index, .. } => *index,
        }
    }
}

/// Streaming-decode state (mirrors Pi's `output` accumulation, anthropic-messages.ts:476-715).
#[derive(Default)]
pub(super) struct Decoder {
    pub(super) blocks: Vec<Block>,
    /// Memoised projection of `blocks`. Write to `blocks` ONLY through [`Self::push_block`] and
    /// [`Self::block_mut`], or this goes stale.
    cache: ContentCache,
    pub(super) usage: Usage,
    pub(super) response_id: Option<String>,
    pub(super) stop_reason: Option<StopReason>,
    /// The provider's own `stop_reason` string, kept verbatim beside the narrowed [`StopReason`]
    /// (pi `output.rawStopReason = event.delta.stop_reason`,
    /// `v0.84.1 ai/src/api/anthropic-messages.ts:709`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too (`v0.83.0 ai/src/api/anthropic-messages.ts:709`) and cyrup never
    /// ported it, so `rawStopReason` was `None` on every anthropic turn. Set once, from
    /// `message_delta`, and never cleared — a mapped `tool_use`/`refusal` still names itself.
    pub(super) raw_stop_reason: Option<String>,
    pub(super) error_message: Option<String>,
    pub(super) saw_message_start: bool,
    pub(super) saw_message_stop: bool,
    /// OAuth replay remaps decoded tool names back to the caller's declared names (Pi
    /// `fromClaudeCodeName`, anthropic-messages.ts:592-594).
    pub(super) is_oauth: bool,
    pub(super) tool_names: Vec<String>,
}

impl Decoder {
    /// Seed a decoder for one stream.
    ///
    /// Exists so [`ContentCache`] can stay a PRIVATE field: a `..Default::default()` struct literal
    /// at the driver's call site would require every field to be visible there, and the memo's
    /// whole correctness argument is that nothing outside this file can write it.
    pub(super) fn new(is_oauth: bool, tool_names: Vec<String>) -> Self {
        Self {
            is_oauth,
            tool_names,
            ..Default::default()
        }
    }

    pub(super) fn position_of(&self, index: i64) -> Option<usize> {
        self.blocks.iter().position(|b| b.index() == index)
    }

    /// Append a block. The ONLY push — it keeps [`ContentCache`] in step (PERF-001).
    pub(super) fn push_block(&mut self, block: Block) {
        self.blocks.push(block);
        self.cache.push();
    }

    /// The ONLY `&mut Block`. Invalidates exactly that block's memo and nothing else, so a delta
    /// against block *i* re-projects block *i* and leaves every sibling's cached `Content` alone
    /// (PERF-001).
    pub(super) fn block_mut(&mut self, pos: usize) -> Option<&mut Block> {
        self.cache.invalidate(pos);
        self.blocks.get_mut(pos)
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
    /// `&mut self` for [`Self::content`]'s memo only. Everything the message carries beyond
    /// `content` is still recomputed per call — the fresh `timestamp` and the `apply_cost` over the
    /// live `usage` — because neither is a pure function of the blocks and freezing either would
    /// change what a subscriber observes (PERF-001).
    pub(super) fn snapshot_owned(&mut self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        apply_cost(&model.cost, &mut usage);
        AssistantMessage {
            content: self.content(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage,
            // In-flight: Pi's `output.stopReason` is still its `"pending"` seed until a
            // `message_delta` carries one (anthropic-messages.ts:509,714-717), and `output` IS the
            // `partial` attached to every non-terminal event. The TERMINAL never takes this value —
            // it goes through `StreamEvent::end_of_stream`, which rewrites `Pending` to the `error`
            // terminal Pi's throw produces.
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            timestamp: now_millis(),
        }
    }
}

/// Project ONE block to its `Content`. Was the body of a `blocks_to_content(&[Block])` map, split
/// out per block so [`ContentCache`] can memoise it (PERF-001). The arms are unchanged.
fn project_block(b: &Block) -> Content {
    match b {
        Block::Text { text, .. } => Content::text(text.clone()),
        Block::Thinking {
            thinking,
            signature,
            redacted,
            ..
        } => Content::Thinking {
            thinking: thinking.clone(),
            thinking_signature: if signature.is_empty() {
                None
            } else {
                Some(signature.clone())
            },
            redacted: *redacted,
        },
        Block::Tool {
            id,
            name,
            partial_json,
            ..
        } => Content::ToolCall(ToolCall {
            id: ToolCallId::from(id.as_str()),
            name: name.clone(),
            // A handle on the buffer, not a parse of it: the arguments are recovered by the same
            // whole-buffer parse the terminal `toolcall_end` uses, but only if something reads
            // them (PERF-001).
            arguments: LazyArgs::streaming(partial_json.clone()),
            thought_signature: None,
        }),
    }
}
