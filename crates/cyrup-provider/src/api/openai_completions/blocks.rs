//! Response decoding: the in-progress block set and the decoder's accumulated state.

use super::finalize::parse_partial_json;
use crate::model::Model;
use crate::usage::apply_cost;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{ApiId, AssistantMessage, Content, StopReason, ToolCall, ToolCallId, Usage};
use std::collections::HashMap;

/// One in-progress content block, in first-appearance order (its index == `content_index`).
pub(super) enum Block {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    Tool {
        id: String,
        name: String,
        args: String,
        thought_signature: Option<String>,
    },
}

/// Streaming-decode state.
#[derive(Default)]
pub(super) struct Decoder {
    pub(super) blocks: Vec<Block>,
    pub(super) text_idx: Option<usize>,
    pub(super) thinking_idx: Option<usize>,
    pub(super) tool_by_stream: HashMap<i64, usize>,
    pub(super) tool_by_id: HashMap<String, usize>,
    /// Encrypted reasoning details whose tool call hasn't been seen yet (Pi
    /// `pendingReasoningDetailsByToolCallId`).
    pub(super) pending_reasoning_by_tool_id: HashMap<String, String>,
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
}

impl Decoder {
    /// Build the live `partial` snapshot (Pi `output`, the mutated AssistantMessage attached to
    /// every non-terminal event, openai-completions.ts:158-175 + `partial: output`). Mirrors
    /// [`build_final_message`](super::finalize::build_final_message) but borrows: the stream is
    /// still in progress, so `stop_reason` is
    /// the in-flight sentinel until a `finish_reason` arrives — Pi seeds
    /// `output.stopReason = "pending"` (openai-completions.ts:218) and attaches that same `output`
    /// as every non-terminal event's `partial`.
    pub(super) fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone().unwrap_or_default();
        apply_cost(&model.cost, &mut usage);
        AssistantMessage {
            content: blocks_to_content(&self.blocks),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: self.response_model.clone(),
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage,
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            timestamp: now_millis(),
        }
    }
}

/// Convert in-progress decoder blocks to content (shared by the live `partial` snapshot and the
/// terminal message). Tool args are parsed best-effort (`{}` for incomplete/invalid JSON).
pub(super) fn blocks_to_content(blocks: &[Block]) -> Vec<Content> {
    blocks
        .iter()
        .map(|block| match block {
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
            } => Content::ToolCall(ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                arguments: parse_partial_json(args),
                thought_signature: thought_signature.clone(),
            }),
        })
        .collect()
}
