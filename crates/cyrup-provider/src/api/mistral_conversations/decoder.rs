//! Response decoding — the accumulator the SSE loop threads through (Pi's `currentBlock` +
//! `output` pair, mistral-conversations.ts:295-310).

use crate::model::Model;
use crate::usage::compute_cost;
use crate::api::content_cache::ContentCache;
use crate::utils::provider_plumbing::now_millis;
use std::sync::Arc;
use cyrup_core::{
    ApiId, AssistantMessage, Content, LazyArgs, SharedStr, StopReason, ToolCall, Usage,
};
use std::collections::HashMap;

/// The in-progress text/thinking block being accumulated (Pi `currentBlock`,
/// mistral-conversations.ts:301).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CurrentKind {
    Text,
    Thinking,
}

/// Streaming-decode state (mirrors Pi's `output` accumulation + `consumeChatStream`,
/// mistral-conversations.ts:295-483).
#[derive(Default)]
pub(super) struct Decoder {
    pub(super) blocks: Vec<Content>,
    /// Memoised projection of `blocks` (PERF-001). Write to `blocks` ONLY through
    /// [`Self::push_block`] and [`Self::block_mut`], and to the scratch below ONLY through
    /// [`Self::push_tool_args`], or this goes stale.
    cache: ContentCache,
    /// Tool-call scratch buffers (the `partialArgs` Pi strips before persisting), keyed by block idx.
    ///
    /// Shared with every snapshot taken from them, so projecting a tool block is a refcount bump
    /// and the `Map` is recovered only if something reads it (PERF-001).
    pub(super) tool_partial_args: HashMap<usize, SharedStr>,
    /// `{callId}:{index}` → content-block index (Pi `toolBlocksByKey`).
    pub(super) tool_blocks_by_key: HashMap<String, usize>,
    pub(super) current: Option<CurrentKind>,
    pub(super) usage: Usage,
    pub(super) response_id: Option<String>,
    /// The settled stop reason, or `None` while none has been delivered — cyrup's spelling of Pi's
    /// `output.stopReason = "pending"` seed (mistral-conversations.ts:153), which is where the
    /// derived `Default` now starts. It previously seeded `Stop`, so a Mistral stream that ended
    /// without a truthy `finishReason` was transcribed as a cleanly completed turn (PROV-010).
    pub(super) stop_reason: Option<StopReason>,
    /// The choice's own `finishReason`, kept verbatim beside the narrowed [`StopReason`] (pi
    /// `output.rawStopReason = choice.finishReason`,
    /// `v0.84.1 ai/src/api/mistral-conversations.ts:356`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too, at the same line (`v0.83.0
    /// ai/src/api/mistral-conversations.ts:356`), and cyrup never ported it. Written under the same
    /// truthiness guard pi uses (`:355`), so a `null`/`""` `finishReason` leaves it unset.
    pub(super) raw_stop_reason: Option<String>,
    pub(super) error_message: Option<String>,
}

impl Decoder {
    /// Append a block. The ONLY push — it keeps [`ContentCache`] in step (PERF-001).
    pub(super) fn push_block(&mut self, block: Content) {
        self.blocks.push(block);
        self.cache.push();
    }

    /// The ONLY `&mut Content`. Invalidates exactly that block's memo (PERF-001).
    pub(super) fn block_mut(&mut self, pos: usize) -> Option<&mut Content> {
        self.cache.invalidate(pos);
        self.blocks.get_mut(pos)
    }

    /// Append to a tool block's scratch buffer.
    ///
    /// This decoder keeps tool arguments OUTSIDE `blocks` (Pi strips `partialArgs` before
    /// persisting), so the scratch is a second thing the projection depends on and therefore a
    /// second invalidation edge the memo needs (PERF-001).
    pub(super) fn push_tool_args(&mut self, pos: usize, fragment: &str) {
        self.tool_partial_args
            .entry(pos)
            .or_default()
            .push_str(fragment);
        self.cache.invalidate(pos);
    }

    /// Seed an empty scratch buffer for a freshly opened tool block.
    pub(super) fn open_tool_args(&mut self, pos: usize) {
        self.tool_partial_args.insert(pos, SharedStr::new());
        self.cache.invalidate(pos);
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
    pub(super) fn snapshot_owned(&mut self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        usage.cost = compute_cost(&model.cost, &usage);
        AssistantMessage {
            content: self.content_snapshot(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage,
            // In-flight snapshots carry Pi's `"pending"` seed (mistral-conversations.ts:153). The
            // TERMINAL event never takes this value: it goes through
            // `StreamEvent::end_of_stream`, which routes `None`/`Pending` to the `error` terminal.
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            timestamp: now_millis(),
        }
    }

    /// The content blocks with tool arguments projected from their scratch buffers, recomputing
    /// only the blocks whose memo was invalidated (PERF-001).
    fn content_snapshot(&mut self) -> Vec<Content> {
        let cache = &mut self.cache;
        let blocks = &self.blocks;
        let scratch = &self.tool_partial_args;
        // Indexed because the projection depends on the scratch map, which is keyed by position.
        cache.project_indexed(blocks, |i, b| {
            match b {
                Content::ToolCall(tc) => {
                    // A handle on the scratch buffer, not a parse of it (PERF-001).
                    let args = match scratch.get(&i) {
                        Some(p) => LazyArgs::streaming(p.clone()),
                        None => tc.arguments.clone(),
                    };
                    Content::ToolCall(ToolCall {
                        arguments: args,
                        ..tc.clone()
                    })
                }
                other => other.clone(),
            }
        })
    }

    pub(super) fn block_index(&self) -> usize {
        self.blocks.len().saturating_sub(1)
    }
}
