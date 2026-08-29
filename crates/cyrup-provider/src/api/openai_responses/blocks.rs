//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! the in-flight content blocks.

use cyrup_core::{Content, LazyArgs, SharedStr, ToolCall, ToolCallId};

pub(super) enum RBlock {
    Thinking {
        /// Shared with every snapshot taken from it, so projecting this block is a refcount bump
        /// rather than a copy of everything streamed so far (PERF-001).
        thinking: SharedStr,
        signature: Option<String>,
    },
    Text {
        text: SharedStr,
        signature: Option<String>,
    },
    Tool {
        call_id: String,
        item_id: String,
        name: String,
        /// The raw accumulated argument buffer. [`LazyArgs`] recovers the `Map` from it on the
        /// first read, so a snapshot nobody reads never parses it at all (PERF-001).
        ///
        /// The buffer is the ONLY argument state this block holds, which is why the frames that
        /// REPLACE it wholesale (`…arguments.done`, `output_item.done`) need no repair: there is
        /// no derived scanner left to fall out of step with it.
        partial_json: SharedStr,
    },
}

/// Project ONE block to its `Content`, split out per block so [`crate::api::content_cache::
/// ContentCache`] can memoise it (PERF-001).
pub(super) fn project_block(b: &RBlock) -> Content {
    {
        match b {
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
                // A handle on the buffer, not a parse of it (PERF-001).
                arguments: LazyArgs::streaming(partial_json.clone()),
                thought_signature: None,
            }),
        }
    }
}
