//! Transcript serialization for summarization (arch-05 §6.6, R-05-012/025). Renders role-labeled
//! lines so the model summarizes rather than continues the conversation; tool results are truncated
//! to 2000 chars per-result before assembly (bounded peak memory). Exported for extensions (R-05-023).
//! Byte-1:1 with Pi `serializeConversation` (`utils.ts:109-162`): parts joined with `\n\n`, tool-call
//! args joined with `, ` and calls joined with `; `, arg values JSON-encoded, empty lines skipped.

use cyrup_core::{Content, Message};

/// Per-tool-result truncation budget, in characters (Pi parity).
const TOOL_RESULT_LIMIT: usize = 2000;

/// Render `messages` to the plain-text transcript form used for summarization (Pi
/// `serializeConversation`, `utils.ts:109-162`).
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for m in messages {
        match m {
            Message::User { content, .. } => {
                // Pi joins user text blocks with "" (no separator).
                let body = join_text(content, "");
                if !body.is_empty() {
                    parts.push(format!("[User]: {body}"));
                }
            }
            Message::Assistant(a) => {
                let thinking = thinking_of(&a.content);
                if !thinking.is_empty() {
                    parts.push(format!("[Assistant thinking]: {thinking}"));
                }
                let text = join_text(&a.content, "\n");
                if !text.is_empty() {
                    parts.push(format!("[Assistant]: {text}"));
                }
                let calls = tool_calls_of(&a.content);
                if !calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {calls}"));
                }
            }
            Message::ToolResult { content, .. } => {
                let body = join_text(content, "");
                if !body.is_empty() {
                    parts.push(format!("[Tool result]: {}", truncate(&body)));
                }
            }
        }
    }
    parts.join("\n\n")
}

fn join_text(content: &[Content], sep: &str) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(sep)
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
                // Pi: `${k}=${JSON.stringify(v)}` joined with ", " (args), calls joined with "; ".
                let args = tc
                    .arguments
                    .iter()
                    .map(|(k, v)| format!("{k}={}", render_arg(v)))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!("{}({args})", tc.name))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// `JSON.stringify(value)`: strings are quoted, objects/arrays/numbers/bools/null are compact JSON.
fn render_arg(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| v.to_string())
}

/// Truncate a tool-result body to [`TOOL_RESULT_LIMIT`] **UTF-16 code units**, appending a marker
/// (R-05-025). Pi `truncateForSummary` (`utils.ts:95-99`) measures with JS `text.length` and slices
/// with `text.slice(0, maxChars)` — both in UTF-16 code units, NOT Unicode scalar values. Counting
/// scalars instead diverges on non-BMP text (emoji etc.): both the cut boundary AND the
/// `[... N more characters truncated]` count would differ, changing the transcript handed to the
/// summarizer model. The reported remainder is `total_utf16 − maxChars` exactly as Pi computes it
/// (independent of the slice); the kept slice is the longest prefix whose UTF-16 length is `≤`
/// `TOOL_RESULT_LIMIT` snapped to a `char` boundary (a non-BMP char straddling the boundary is
/// excluded rather than split into a lone surrogate, which is unrepresentable in UTF-8 — the only
/// residual, pathological divergence, and the byte-count marker still matches Pi).
fn truncate(s: &str) -> String {
    let total = s.encode_utf16().count();
    if total <= TOOL_RESULT_LIMIT {
        return s.to_string();
    }
    let mut units = 0usize;
    let mut end = 0usize;
    for (i, ch) in s.char_indices() {
        let w = ch.len_utf16();
        if units + w > TOOL_RESULT_LIMIT {
            break;
        }
        units += w;
        end = i + ch.len_utf8();
    }
    let kept = s.get(..end).unwrap_or("");
    let remaining = total - TOOL_RESULT_LIMIT;
    format!("{kept}\n\n[... {remaining} more characters truncated]")
}
