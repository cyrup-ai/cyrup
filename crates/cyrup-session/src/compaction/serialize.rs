//! Transcript serialization for summarization (arch-05 §6.6, R-05-012/025). Renders role-labeled
//! lines so the model summarizes rather than continues the conversation; tool results are truncated
//! to 2000 chars per-result before assembly (bounded peak memory). Exported for extensions (R-05-023).

use cyrup_core::{Content, Message};

/// Per-tool-result truncation budget, in characters (Pi parity).
const TOOL_RESULT_LIMIT: usize = 2000;

/// Render `messages` to the plain-text transcript form used for summarization (R-05-012).
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        match m {
            Message::User { content, .. } => {
                push_line(&mut out, "[User]:", &text_of(content));
            }
            Message::Assistant(a) => {
                let thinking = thinking_of(&a.content);
                if !thinking.is_empty() {
                    push_line(&mut out, "[Assistant thinking]:", &thinking);
                }
                let text = text_of(&a.content);
                if !text.is_empty() {
                    push_line(&mut out, "[Assistant]:", &text);
                }
                let calls = tool_calls_of(&a.content);
                if !calls.is_empty() {
                    push_line(&mut out, "[Assistant tool calls]:", &calls);
                }
            }
            Message::ToolResult { content, .. } => {
                let truncated = truncate(&text_of(content));
                push_line(&mut out, "[Tool result]:", &truncated);
            }
        }
    }
    out
}

fn push_line(out: &mut String, label: &str, body: &str) {
    out.push_str(label);
    out.push(' ');
    out.push_str(body);
    out.push('\n');
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn thinking_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Thinking { thinking, .. } => Some(thinking.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_calls_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => {
                let args = match &tc.arguments {
                    serde_json::Value::Object(map) => map
                        .iter()
                        .map(|(k, v)| format!("{k}={}", render_arg(v)))
                        .collect::<Vec<_>>()
                        .join("; "),
                    other => other.to_string(),
                };
                Some(format!("{}({args})", tc.name))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_arg(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Truncate a tool-result body to [`TOOL_RESULT_LIMIT`] chars, appending a marker (R-05-025).
fn truncate(s: &str) -> String {
    let total = s.chars().count();
    if total <= TOOL_RESULT_LIMIT {
        return s.to_string();
    }
    let kept: String = s.chars().take(TOOL_RESULT_LIMIT).collect();
    let remaining = total - TOOL_RESULT_LIMIT;
    format!("{kept}\n[... {remaining} more characters truncated]")
}
