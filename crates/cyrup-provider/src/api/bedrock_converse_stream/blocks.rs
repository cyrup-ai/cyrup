//! Streaming-decode state (pi's `output` accumulation, `bedrock-converse-stream.ts:114-132`).

use crate::api::content_cache::ContentCache;
use crate::model::Model;
use crate::usage::compute_cost;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{
    ApiId, AssistantMessage, Content, LazyArgs, SharedStr, StopReason, ToolCall, ToolCallId, Usage,
};
use std::sync::Arc;

/// One in-progress content block, keyed by Bedrock's `contentBlockIndex` (pi's `Block` type,
/// `bedrock-converse-stream.ts:102`). `index` and `partial_json` are the streaming scratch fields
/// upstream `delete`s before the message escapes; here they are separate struct fields that the
/// snapshot never projects, so there is nothing to strip.
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

/// Streaming-decode state (pi's `output` accumulation, `bedrock-converse-stream.ts:114-132`).
#[derive(Default)]
pub(super) struct Decoder {
    pub(super) blocks: Vec<Block>,
    /// Memoised projection of `blocks` (PERF-001). Write to `blocks` ONLY through
    /// [`Self::push_block`] and [`Self::block_mut`], or this goes stale.
    cache: ContentCache,
    pub(super) usage: Usage,
    pub(super) stop_reason: Option<StopReason>,
    /// The provider's own `messageStop.stopReason`, kept verbatim beside the narrowed
    /// [`StopReason`] (pi `output.rawStopReason = item.messageStop.stopReason`,
    /// `v0.84.1 ai/src/api/bedrock-converse-stream.ts:276`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too (`v0.83.0 ai/src/api/bedrock-converse-stream.ts:270`) and cyrup never
    /// ported it. Assigned UNCONDITIONALLY at `messageStop` — pi has no truthiness guard there, so a
    /// `messageStop` with no `stopReason` writes `undefined`, i.e. `None`.
    pub(super) raw_stop_reason: Option<String>,
    pub(super) error_message: Option<String>,
}

impl Decoder {
    pub(super) fn position_of(&self, index: i64) -> Option<usize> {
        self.blocks.iter().position(|b| b.index() == index)
    }

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
    /// `calculateCost` fills only `usage.cost` upstream (`:543`), and `handleMetadata` sets
    /// `totalTokens` from the provider's own figure (`:542`), so `total_tokens` is NOT recomputed
    /// here.
    pub(super) fn snapshot_owned(&mut self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        usage.cost = compute_cost(&model.cost, &usage);
        AssistantMessage {
            content: self.content(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage,
            // pi seeds `output.stopReason = "pending"` (`:128`) and that seed IS the `partial`
            // attached to every non-terminal event. The terminal never takes this value — it goes
            // through `StreamEvent::end_of_stream`.
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            timestamp: now_millis(),
        }
    }
}

/// Project ONE block to its `Content`. Was the body of a `blocks_to_content(&[Block])` map, split
/// out per block so [`ContentCache`] can memoise it (PERF-001).
fn project_block(b: &Block) -> Content {
    {
        match b {
            Block::Text { text, .. } => Content::text(text.clone()),
            Block::Thinking {
                thinking,
                signature,
                ..
            } => Content::Thinking {
                thinking: thinking.clone(),
                thinking_signature: if signature.is_empty() {
                    None
                } else {
                    Some(signature.clone())
                },
                redacted: false,
            },
            Block::Tool {
                id,
                name,
                partial_json,
                ..
            } => Content::ToolCall(ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                // A handle on the buffer, not a parse of it (PERF-001).
                arguments: LazyArgs::streaming(partial_json.clone()),
                thought_signature: None,
            }),
        }
    }
}
