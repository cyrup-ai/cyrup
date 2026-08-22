//! Streaming-decode state (pi's `output` accumulation, `bedrock-converse-stream.ts:114-132`).

use crate::model::Model;
use crate::usage::compute_cost;
use crate::utils::json_parse::parse_streaming_json_object;
use cyrup_core::{ApiId, AssistantMessage, Content, StopReason, ToolCall, ToolCallId, Usage};

/// One in-progress content block, keyed by Bedrock's `contentBlockIndex` (pi's `Block` type,
/// `bedrock-converse-stream.ts:102`). `index` and `partial_json` are the streaming scratch fields
/// upstream `delete`s before the message escapes; here they are separate struct fields that the
/// snapshot never projects, so there is nothing to strip.
pub(super) enum Block {
    Text {
        index: i64,
        text: String,
    },
    Thinking {
        index: i64,
        thinking: String,
        signature: String,
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

/// Streaming-decode state (pi's `output` accumulation, `bedrock-converse-stream.ts:114-132`).
#[derive(Default)]
pub(super) struct Decoder {
    pub(super) blocks: Vec<Block>,
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

    /// Build the live `partial` snapshot. `calculateCost` fills only `usage.cost` upstream
    /// (`:543`), and `handleMetadata` sets `totalTokens` from the provider's own figure
    /// (`:542`), so `total_tokens` is NOT recomputed here.
    pub(super) fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        usage.cost = compute_cost(&model.cost, &usage);
        AssistantMessage {
            content: blocks_to_content(&self.blocks),
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

fn blocks_to_content(blocks: &[Block]) -> Vec<Content> {
    blocks
        .iter()
        .map(|b| match b {
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
                arguments: parse_streaming_json_object(Some(partial_json)),
                thought_signature: None,
            }),
        })
        .collect()
}

/// Current unix time in milliseconds (0 on a clock error — never panics).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
