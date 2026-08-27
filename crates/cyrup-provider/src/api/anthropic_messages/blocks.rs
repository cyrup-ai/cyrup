//! Response decoding — in-progress block and decoder state.

use crate::model::Model;
use crate::usage::apply_cost;
use crate::utils::json_parse::parse_streaming_json_object;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{ApiId, AssistantMessage, Content, StopReason, ToolCall, ToolCallId, Usage};

/// One in-progress content block, keyed by the Anthropic `index`.
pub(super) enum Block {
    Text {
        index: i64,
        text: String,
    },
    Thinking {
        index: i64,
        thinking: String,
        signature: String,
        redacted: bool,
    },
    Tool {
        index: i64,
        id: String,
        name: String,
        partial_json: String,
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
    pub(super) fn position_of(&self, index: i64) -> Option<usize> {
        self.blocks.iter().position(|b| b.index() == index)
    }

    /// Build the live `partial` snapshot.
    pub(super) fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        apply_cost(&model.cost, &mut usage);
        AssistantMessage {
            content: blocks_to_content(&self.blocks),
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

fn blocks_to_content(blocks: &[Block]) -> Vec<Content> {
    blocks
        .iter()
        .map(|b| match b {
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
                arguments: parse_streaming_json_object(Some(partial_json)),
                thought_signature: None,
            }),
        })
        .collect()
}
