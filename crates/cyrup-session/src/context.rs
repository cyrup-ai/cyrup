//! Context building (arch-04 §6.3, R-04-011/012/013). The leaf→root walk lives on
//! `SessionManager`; this module owns the result type and the entry→message conversions.

use cyrup_core::{Content, Message, ModelRef};
use serde_json::Value;

use crate::entry::{Entry, KnownEntry};

/// The LLM context built from the active path (returned to the agent loop / arch-06).
#[derive(Clone, Debug)]
pub struct SessionContext {
    /// Active-path messages (extension-state `Custom` entries already filtered out).
    pub messages: Vec<Message>,
    /// Most recent thinking level on the path, else `"off"`.
    pub thinking_level: String,
    /// Most recent model on the path (session-local — `api: None`), if any.
    pub model: Option<ModelRef>,
}

impl SessionContext {
    pub fn empty() -> Self {
        Self { messages: Vec::new(), thinking_level: "off".to_string(), model: None }
    }
}

/// Append the LLM-message form of an entry, per R-04-006/013:
/// - `Message` → included verbatim;
/// - `CustomMessage` → converted to a user-form message (extension-injected message);
/// - `BranchSummary` (non-empty) → converted to a synthesized message;
/// - everything else (`Custom`, `Label`, `ModelChange`, `ThinkingLevelChange`, `SessionInfo`,
///   `Compaction`, `Unknown`) → skipped.
pub fn push_as_message(out: &mut Vec<Message>, e: &Entry) {
    if let Entry::Known(k) = e {
        match k {
            KnownEntry::Message { message, .. } => out.push(message.clone()),
            KnownEntry::CustomMessage { content, .. } => out.push(custom_to_message(content)),
            KnownEntry::BranchSummary { summary, .. } if !summary.is_empty() => {
                out.push(branch_summary_message(summary));
            }
            _ => {}
        }
    }
}

/// Compaction summary rendered as the FIRST message of a compacted context (R-04-012).
pub fn compaction_summary_message(summary: &str, tokens_before: u64) -> Message {
    let text = format!("[compacted history — {tokens_before} tokens summarized]\n{summary}");
    Message::User { content: vec![Content::text(text)], timestamp: 0 }
}

/// A `BranchSummary` rendered as a user-form note describing the abandoned branch's work.
pub fn branch_summary_message(summary: &str) -> Message {
    Message::User {
        content: vec![Content::text(format!("[branch summary]\n{summary}"))],
        timestamp: 0,
    }
}

/// Convert a `CustomMessage` `content` (string | (Text|Image)[]) into a user-form message.
fn custom_to_message(content: &Value) -> Message {
    let blocks = match content {
        Value::String(s) => vec![Content::text(s.clone())],
        Value::Array(_) => serde_json::from_value::<Vec<Content>>(content.clone())
            .unwrap_or_else(|_| vec![Content::text(content.to_string())]),
        other => vec![Content::text(other.to_string())],
    };
    Message::User { content: blocks, timestamp: 0 }
}
