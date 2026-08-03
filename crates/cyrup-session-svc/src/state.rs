//! Serializable read-only state views the RPC/print front-ends snapshot (arch-11 §3.1; the
//! `state.rs` module arch-11 §2.1 prescribes). These mirror Pi's `getSessionStats`
//! (agent-session.ts:2932), the `state` getter (agent-session.ts:753), and `getContextUsage`
//! (agent-session.ts:2977) — none of which mutate the session.

use cyrup_core::{AssistantMessage, Message};

/// Aggregate transcript counters for the current branch (Pi `SessionStats`, agent-session.ts:2932).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    /// Total persisted messages on the current branch.
    pub message_count: usize,
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub tool_result_count: usize,
    /// Summed input tokens across assistant turns **and** tool results that reported usage.
    pub input_tokens: u64,
    /// Summed output tokens across assistant turns **and** tool results that reported usage.
    pub output_tokens: u64,
    /// Summed cache-read + cache-write tokens across assistant turns **and** tool results that
    /// reported usage.
    pub cache_tokens: u64,
}

impl SessionStats {
    /// Compute the stats from the current-branch messages (Pi `getSessionStats`).
    pub fn from_messages(messages: &[Message]) -> Self {
        let mut s = Self { message_count: messages.len(), ..Self::default() };
        for m in messages {
            match m {
                Message::User { .. } => s.user_message_count += 1,
                // A tool that reported usage for its OWN execution spends real tokens, so it is
                // billed and must appear in the totals (Pi `if (message.usage)
                // { addUsageToTotals(usageTotals, message.usage); }`, agent-session.ts:3129-3132).
                // Before this a metering/summarizer tool was billed-but-invisible here.
                Message::ToolResult { usage, .. } => {
                    s.tool_result_count += 1;
                    if let Some(u) = usage {
                        s.input_tokens += u.input;
                        s.output_tokens += u.output;
                        s.cache_tokens += u.cache_read + u.cache_write;
                    }
                }
                Message::Assistant(a) => {
                    s.assistant_message_count += 1;
                    s.input_tokens += a.usage.input;
                    s.output_tokens += a.usage.output;
                    s.cache_tokens += a.usage.cache_read + a.usage.cache_write;
                }
            }
        }
        s
    }
}

/// Context-window occupancy derived from the most recent assistant turn (Pi `getContextUsage`,
/// agent-session.ts:2977): what the footer renders as "tokens used / window".
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    /// Tokens occupying the window after the last turn (input + cache + output of that turn).
    pub used_tokens: u64,
    /// The active model's context window.
    pub context_window: u64,
    /// `used_tokens / context_window` clamped to `[0, 1]` (0 when the window is unknown).
    pub fraction: f64,
}

impl ContextUsage {
    /// Build from the last assistant message's usage + the model's context window.
    pub fn from_last_assistant(last: Option<&AssistantMessage>, context_window: u64) -> Self {
        let used = last
            .map(|a| a.usage.input + a.usage.cache_read + a.usage.cache_write + a.usage.output)
            .unwrap_or(0);
        let fraction = if context_window == 0 {
            0.0
        } else {
            (used as f64 / context_window as f64).clamp(0.0, 1.0)
        };
        Self { used_tokens: used, context_window, fraction }
    }
}

/// The outcome of a compaction (Pi `CompactionResult`, agent-session.ts:1751-1757). Returned by
/// [`crate::AgentSession::compact`] and surfaced on the `compaction_end` event.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    /// Estimated token count of the rebuilt (post-compaction) context.
    pub estimated_tokens_after: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// A serializable snapshot of the live session for RPC `get_state` (Pi `state` getter,
/// agent-session.ts:753). Read-only; carries no handles.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStateView {
    pub session_id: String,
    pub cwd: String,
    pub provider: String,
    pub model: String,
    pub session_name: Option<String>,
    pub is_streaming: bool,
    pub message_count: usize,
    pub pending_message_count: usize,
    pub stats: SessionStats,
    pub context_usage: ContextUsage,
}
