//! Heuristic context-token estimation (1:1 with Pi `utils/estimate.ts`).
//!
//! A fast, character-count-based token estimate used to clamp `max_tokens` to the remaining context
//! window (see [`crate::utils::simple_options`]) and to drive compaction decisions before a real
//! usage figure is available. Faithful port of `estimate.ts:14-111`.
//!
//! String lengths use UTF-16 code-unit counts (`encode_utf16().count()`) to match JS `String.length`
//! exactly, so the estimate is byte-identical to Pi for non-ASCII content.

use crate::context::{Context, ToolDef};
use cyrup_core::{Content, Message, Usage};

/// Per-call context-usage estimate (Pi `ContextUsageEstimate`, estimate.ts:3-12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextUsageEstimate {
    /// Estimated total context tokens.
    pub tokens: u64,
    /// Tokens reported by the most recent assistant usage block.
    pub usage_tokens: u64,
    /// Estimated tokens after the most recent assistant usage block.
    pub trailing_tokens: u64,
    /// Index of the message that provided usage, or `None` when none exists.
    pub last_usage_index: Option<usize>,
}

const CHARS_PER_TOKEN: u64 = 4;
const ESTIMATED_IMAGE_CHARS: u64 = 4800;

/// JS `String.length` (UTF-16 code units).
fn js_len(s: &str) -> u64 {
    s.encode_utf16().count() as u64
}

/// `Math.ceil(chars / CHARS_PER_TOKEN)` in integer arithmetic.
fn ceil_div(chars: u64) -> u64 {
    chars.div_ceil(CHARS_PER_TOKEN)
}

/// Total context tokens for a usage block (Pi `calculateContextTokens`, estimate.ts:17-19):
/// `totalTokens` when non-zero, else `input + output + cacheRead + cacheWrite`.
pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens != 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

/// `JSON.stringify(value)` with Pi's fallbacks (`"undefined"` / `"[unserializable]"`). For a
/// `serde_json::Value` serialization never fails, so only the value-to-string path is needed.
fn safe_json_stringify(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_string())
}

/// Chars for a `Text|Image` content array (Pi `estimateTextAndImageContentChars`, estimate.ts:29-35).
fn text_and_image_chars(content: &[Content]) -> u64 {
    let mut chars = 0;
    for block in content {
        chars += match block {
            Content::Text { text, .. } => js_len(text),
            Content::Image { .. } => ESTIMATED_IMAGE_CHARS,
            // user/toolResult content is validated to Text|Image only; other variants contribute 0.
            _ => 0,
        };
    }
    chars
}

/// Estimate tokens for a raw text string (Pi `estimateTextTokens`, estimate.ts:37-39).
pub fn estimate_text_tokens(text: &str) -> u64 {
    ceil_div(js_len(text))
}

/// Estimate tokens for a `Text|Image` content array (Pi `estimateTextAndImageContentTokens`,
/// estimate.ts:41-43).
pub fn estimate_text_and_image_content_tokens(content: &[Content]) -> u64 {
    ceil_div(text_and_image_chars(content))
}

/// Estimate tokens for one message (Pi `estimateMessageTokens`, estimate.ts:45-61).
pub fn estimate_message_tokens(message: &Message) -> u64 {
    match message {
        Message::User { content, .. } => estimate_text_and_image_content_tokens(content),
        Message::ToolResult { content, .. } => estimate_text_and_image_content_tokens(content),
        Message::Assistant(assistant) => {
            let mut chars = 0;
            for block in &assistant.content {
                chars += match block {
                    Content::Text { text, .. } => js_len(text),
                    Content::Thinking { thinking, .. } => js_len(thinking),
                    Content::ToolCall(tc) => {
                        let args = serde_json::Value::Object(tc.arguments.clone());
                        js_len(&tc.name) + js_len(&safe_json_stringify(&args))
                    }
                    Content::Image { .. } => 0,
                };
            }
            ceil_div(chars)
        }
    }
}

/// The most recent non-aborted/non-error assistant message carrying usage (Pi
/// `getLastAssistantUsageInfo`, estimate.ts:63-72).
fn last_assistant_usage_info(messages: &[Message]) -> Option<(&Usage, usize)> {
    for (i, message) in messages.iter().enumerate().rev() {
        if let Message::Assistant(assistant) = message {
            use cyrup_core::StopReason;
            if matches!(
                assistant.stop_reason,
                StopReason::Aborted | StopReason::Error
            ) {
                continue;
            }
            if calculate_context_tokens(&assistant.usage) > 0 {
                return Some((&assistant.usage, i));
            }
        }
    }
    None
}

/// Estimate over a raw message list (Pi `estimateMessages`, estimate.ts:74-88).
fn estimate_messages(messages: &[Message]) -> ContextUsageEstimate {
    if let Some((usage, index)) = last_assistant_usage_info(messages) {
        let usage_tokens = calculate_context_tokens(usage);
        let mut trailing_tokens = 0;
        for message in messages.iter().skip(index + 1) {
            trailing_tokens += estimate_message_tokens(message);
        }
        return ContextUsageEstimate {
            tokens: usage_tokens + trailing_tokens,
            usage_tokens,
            trailing_tokens,
            last_usage_index: Some(index),
        };
    }

    let mut tokens = 0;
    for message in messages {
        tokens += estimate_message_tokens(message);
    }
    ContextUsageEstimate {
        tokens,
        usage_tokens: 0,
        trailing_tokens: tokens,
        last_usage_index: None,
    }
}

/// Estimate context tokens for a full [`Context`] (Pi `estimateContextTokens`, estimate.ts:94-111).
/// When the message list already carries a recent assistant usage block, the system-prompt + tools
/// prefix is assumed to be already accounted for and is NOT added (matching Pi's early return).
pub fn estimate_context_tokens(context: &Context) -> ContextUsageEstimate {
    let estimate = estimate_messages(&context.messages);
    if estimate.last_usage_index.is_some() {
        return estimate;
    }

    let mut prefix_tokens = match context.system_prompt.as_deref() {
        Some(prompt) => estimate_text_tokens(prompt),
        None => 0,
    };
    if !context.tools.is_empty() {
        prefix_tokens += estimate_text_tokens(&safe_json_stringify(&tools_to_json(&context.tools)));
    }

    ContextUsageEstimate {
        tokens: estimate.tokens + prefix_tokens,
        usage_tokens: estimate.usage_tokens,
        trailing_tokens: estimate.trailing_tokens + prefix_tokens,
        last_usage_index: estimate.last_usage_index,
    }
}

/// Estimate context tokens directly over a message slice (Pi's `estimateContextTokens(Message[])`
/// overload, estimate.ts:94-95).
pub fn estimate_message_list_tokens(messages: &[Message]) -> ContextUsageEstimate {
    estimate_messages(messages)
}

/// Serialize the tool defs the way `JSON.stringify(context.tools)` would (so the char count matches
/// Pi). [`ToolDef`] is camelCase-serde, so this is byte-1:1 with Pi's `Tool[]` JSON.
fn tools_to_json(tools: &[ToolDef]) -> serde_json::Value {
    serde_json::to_value(tools).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use cyrup_core::{AssistantMessage, ProviderId, StopReason, ToolCall, ToolCallId, Usage};

    fn user(text: &str) -> Message {
        Message::User {
            content: vec![Content::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            timestamp: 0,
        }
    }

    fn assistant_with_usage(usage: Usage, stop: StopReason) -> Message {
        let mut m = AssistantMessage::errored(ProviderId::from("p"), "m", None, stop, "");
        m.error_message = None;
        m.usage = usage;
        m.content = vec![Content::Text {
            text: "hello world".into(),
            text_signature: None,
        }];
        Message::Assistant(m)
    }

    #[test]
    fn calculate_context_tokens_prefers_total_else_sums() {
        let with_total = Usage {
            total_tokens: 100,
            input: 1,
            output: 2,
            ..Usage::default()
        };
        assert_eq!(calculate_context_tokens(&with_total), 100);
        let no_total = Usage {
            total_tokens: 0,
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 5,
            ..Usage::default()
        };
        assert_eq!(calculate_context_tokens(&no_total), 40);
    }

    #[test]
    fn text_tokens_ceil_divide_by_four() {
        assert_eq!(estimate_text_tokens(""), 0);
        assert_eq!(estimate_text_tokens("abcd"), 1);
        assert_eq!(estimate_text_tokens("abcde"), 2); // ceil(5/4)
    }

    #[test]
    fn image_block_counts_estimated_chars() {
        let content = vec![Content::Image {
            data: "x".into(),
            mime_type: "image/png".into(),
        }];
        // ceil(4800/4) = 1200
        assert_eq!(estimate_text_and_image_content_tokens(&content), 1200);
    }

    #[test]
    fn assistant_toolcall_counts_name_plus_args_json() {
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::Value::String("a.txt".into()));
        let mut m =
            AssistantMessage::errored(ProviderId::from("p"), "m", None, StopReason::Stop, "");
        m.error_message = None;
        m.content = vec![Content::ToolCall(ToolCall {
            id: ToolCallId::from("t1"),
            name: "read".into(),
            arguments: args.clone(),
            thought_signature: None,
        })];
        let json = serde_json::to_string(&serde_json::Value::Object(args)).unwrap();
        let expected = ceil_div(js_len("read") + js_len(&json));
        assert_eq!(estimate_message_tokens(&Message::Assistant(m)), expected);
    }

    #[test]
    fn context_estimate_uses_last_assistant_usage_and_skips_prefix() {
        let usage = Usage {
            total_tokens: 500,
            ..Usage::default()
        };
        let ctx = Context {
            system_prompt: Some("a very long system prompt".into()),
            messages: vec![
                user("hi"),
                assistant_with_usage(usage, StopReason::Stop),
                user("again"),
            ],
            tools: Vec::new(),
        };
        let est = estimate_context_tokens(&ctx);
        assert_eq!(est.last_usage_index, Some(1));
        assert_eq!(est.usage_tokens, 500);
        // trailing = estimate of the final "again" user message = ceil(5/4) = 2
        assert_eq!(est.trailing_tokens, estimate_text_tokens("again"));
        assert_eq!(est.tokens, 500 + est.trailing_tokens);
    }

    #[test]
    fn context_estimate_adds_prefix_when_no_usage() {
        let ctx = Context {
            system_prompt: Some("sys".into()),
            messages: vec![user("hello")],
            tools: Vec::new(),
        };
        let est = estimate_context_tokens(&ctx);
        assert_eq!(est.last_usage_index, None);
        // tokens = msg("hello") + prefix("sys")
        assert_eq!(
            est.tokens,
            estimate_text_tokens("hello") + estimate_text_tokens("sys")
        );
    }

    #[test]
    fn aborted_assistant_usage_is_skipped() {
        let usage = Usage {
            total_tokens: 999,
            ..Usage::default()
        };
        let ctx = Context {
            system_prompt: None,
            messages: vec![assistant_with_usage(usage, StopReason::Aborted), user("hi")],
            tools: Vec::new(),
        };
        let est = estimate_context_tokens(&ctx);
        assert_eq!(est.last_usage_index, None); // aborted usage ignored
    }
}
