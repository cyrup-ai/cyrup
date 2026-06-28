//! Token estimation (arch-05 §3.2, R-05-024). A cheap chars/4 heuristic for the cut-point walk,
//! plus a trigger estimate that trusts the provider-reported `Usage` of the last valid assistant
//! message and only locally estimates the trailing tail. Per-immutable-entry estimates are cached.

use std::collections::HashMap;
use std::sync::Mutex;

use cyrup_core::{Content, EntryId, Message, StopReason, Usage};

use crate::context::push_as_message;
use crate::entry::Entry;

/// Images count as this many chars before the `/4` division (Pi parity).
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// Conservative chars/4 estimate for a single message.
pub fn estimate_tokens(msg: &Message) -> u32 {
    let blocks = match msg {
        Message::User { content, .. } | Message::ToolResult { content, .. } => content,
        Message::Assistant(a) => &a.content,
    };
    let chars: usize = blocks.iter().map(content_chars).sum();
    (chars / 4) as u32
}

fn content_chars(c: &Content) -> usize {
    match c {
        Content::Text { text, .. } => text.chars().count(),
        Content::Thinking { thinking, .. } => thinking.chars().count(),
        Content::ToolCall(tc) => {
            tc.name.chars().count()
                + serde_json::to_string(&tc.arguments).map(|s| s.chars().count()).unwrap_or(0)
        }
        Content::Image { .. } => ESTIMATED_IMAGE_CHARS,
    }
}

/// `calculateContextTokens` parity: `usage.total_tokens` when present, else the sum of
/// input/output/cache parts.
pub fn context_tokens_from_usage(u: &Usage) -> u32 {
    let total = if u.total_tokens > 0 {
        u.total_tokens
    } else {
        u.input
            .saturating_add(u.output)
            .saturating_add(u.cache_read)
            .saturating_add(u.cache_write)
    };
    u32::try_from(total).unwrap_or(u32::MAX)
}

/// Result of estimating live context (R-05-024): provider usage of the last valid assistant
/// message + a chars/4 estimate of any trailing messages.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContextUsageEstimate {
    /// The trigger value: `usage_tokens + trailing_tokens`.
    pub tokens: u32,
    /// Provider-reported, authoritative when present.
    pub usage_tokens: u32,
    /// chars/4 estimate of messages after the last usage.
    pub trailing_tokens: u32,
    pub last_usage_index: Option<usize>,
}

/// Estimate the live context: prefer the last *valid* assistant usage (skip aborted/error/all-zero)
/// and locally estimate only the trailing messages.
pub fn estimate_context_tokens(messages: &[Message]) -> ContextUsageEstimate {
    let mut last_usage_index = None;
    let mut usage_tokens = 0;
    for (i, m) in messages.iter().enumerate() {
        if let Message::Assistant(a) = m {
            let valid = !matches!(a.stop_reason, StopReason::Error | StopReason::Aborted);
            let tok = context_tokens_from_usage(&a.usage);
            if valid && tok > 0 {
                last_usage_index = Some(i);
                usage_tokens = tok;
            }
        }
    }
    let trailing_start = last_usage_index.map(|i| i + 1).unwrap_or(0);
    let trailing_tokens: u32 = messages
        .get(trailing_start..)
        .unwrap_or(&[])
        .iter()
        .map(estimate_tokens)
        .fold(0u32, |a, b| a.saturating_add(b));
    ContextUsageEstimate {
        tokens: usage_tokens.saturating_add(trailing_tokens),
        usage_tokens,
        trailing_tokens,
        last_usage_index,
    }
}

/// Per-entry token-estimate cache keyed by `EntryId` so the trigger check and cut-point walk do
/// NOT re-tokenize the whole history each turn (R-05-024). Entries are immutable once appended ⇒
/// estimates never invalidate.
#[derive(Default)]
pub struct TokenCache {
    map: Mutex<HashMap<EntryId, u32>>,
}

impl TokenCache {
    /// Estimate (chars/4) the messages an entry contributes, memoized by entry id.
    pub fn estimate_entry(&self, entry: &Entry) -> u32 {
        let id = entry.id();
        if let Ok(map) = self.map.lock()
            && let Some(v) = map.get(&id) {
                return *v;
            }
        let mut msgs = Vec::new();
        push_as_message(&mut msgs, entry);
        let est = msgs.iter().map(estimate_tokens).fold(0u32, |a, b| a.saturating_add(b));
        if let Ok(mut map) = self.map.lock() {
            map.insert(id, est);
        }
        est
    }

    /// Drop a cached estimate (only needed on rare entry mutation).
    pub fn invalidate(&self, id: &EntryId) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(id);
        }
    }
}
