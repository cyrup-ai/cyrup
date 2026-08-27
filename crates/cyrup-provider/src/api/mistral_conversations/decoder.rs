//! Response decoding — the accumulator the SSE loop threads through (Pi's `currentBlock` +
//! `output` pair, mistral-conversations.ts:295-310).

use crate::model::Model;
use crate::usage::compute_cost;
use crate::utils::json_parse::parse_streaming_json_object;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{ApiId, AssistantMessage, Content, StopReason, ToolCall, Usage};
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
    /// Tool-call scratch buffers (the `partialArgs` Pi strips before persisting), keyed by block idx.
    pub(super) tool_partial_args: HashMap<usize, String>,
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
    pub(super) fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
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

    /// The content blocks with tool arguments re-parsed from their scratch buffers.
    fn content_snapshot(&self) -> Vec<Content> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(i, b)| match b {
                Content::ToolCall(tc) => {
                    let args = self
                        .tool_partial_args
                        .get(&i)
                        .map(|p| parse_streaming_json_object(Some(p)))
                        .unwrap_or_else(|| tc.arguments.clone());
                    Content::ToolCall(ToolCall {
                        arguments: args,
                        ..tc.clone()
                    })
                }
                other => other.clone(),
            })
            .collect()
    }

    pub(super) fn block_index(&self) -> usize {
        self.blocks.len().saturating_sub(1)
    }
}
