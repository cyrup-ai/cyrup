//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! the in-flight content blocks.

use crate::utils::json_parse::parse_streaming_json_object;
use cyrup_core::{Content, ToolCall, ToolCallId};

pub(super) enum RBlock {
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    Text {
        text: String,
        signature: Option<String>,
    },
    Tool {
        call_id: String,
        item_id: String,
        name: String,
        partial_json: String,
    },
}

pub(super) fn blocks_to_content(blocks: &[RBlock]) -> Vec<Content> {
    blocks
        .iter()
        .map(|b| match b {
            RBlock::Thinking {
                thinking,
                signature,
            } => Content::Thinking {
                thinking: thinking.clone(),
                thinking_signature: signature.clone(),
                redacted: false,
            },
            RBlock::Text { text, signature } => Content::Text {
                text: text.clone(),
                text_signature: signature.clone(),
            },
            RBlock::Tool {
                call_id,
                item_id,
                name,
                partial_json,
            } => Content::ToolCall(ToolCall {
                id: ToolCallId::from(format!("{call_id}|{item_id}").as_str()),
                name: name.clone(),
                arguments: parse_streaming_json_object(Some(partial_json)),
                thought_signature: None,
            }),
        })
        .collect()
}
