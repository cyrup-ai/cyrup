//! Token estimation (arch-05 §3.2, R-05-024). A cheap chars/4 heuristic for the cut-point walk,
//! plus a trigger estimate that trusts the provider-reported `Usage` of the last valid assistant
//! message and only locally estimates the trailing tail. Per-immutable-entry estimates are cached.

use std::collections::HashMap;
use std::sync::Mutex;

use cyrup_core::{Content, EntryId, Message, StopReason, Usage};

use crate::agent_message::AgentMessage;
use crate::context::{push_as_message, RawContextMessage};
use crate::entry::{Entry, KnownEntry};

/// Images count as this many chars before the `/4` division (Pi parity).
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// UTF-16 code-unit length, matching JavaScript `String.length` (Pi estimates with `.length`,
/// `compaction.ts:236-296`). Using unicode scalars instead would diverge on non-BMP / multi-byte
/// text.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Conservative chars/4 estimate for a single core message (Pi `estimateTokens`,
/// `compaction.ts:256-296`).
pub fn estimate_tokens(msg: &Message) -> u32 {
    let blocks = match msg {
        Message::User { content, .. } | Message::ToolResult { content, .. } => content,
        Message::Assistant(a) => &a.content,
    };
    let chars: usize = blocks.iter().map(content_chars).sum();
    // Pi rounds UP: `Math.ceil(chars / 4)` (`compaction.ts:264,277,287,291`). Flooring would
    // systematically under-count every message by up to 1 token and shift the cut-point / trigger.
    chars.div_ceil(4) as u32
}

/// Pi `estimateTokens` over the full `AgentMessage` superset (`compaction.ts:256-296`): a
/// `bashExecution` costs `(command.length + output.length)/4`; a `custom` costs its content
/// chars/4; core roles match [`estimate_tokens`]. This is what the cut-point walk accumulates, so
/// it must match Pi's raw per-message estimate (not the rendered LLM text).
pub fn estimate_agent_message(msg: &AgentMessage) -> u32 {
    match msg {
        AgentMessage::Core(m) => estimate_tokens(m),
        AgentMessage::BashExecution(b) => {
            // Pi `Math.ceil((command.length + output.length) / 4)` (`compaction.ts:284-287`).
            (utf16_len(&b.command) + utf16_len(&b.output)).div_ceil(4) as u32
        }
        AgentMessage::Custom(c) => custom_content_chars(&c.content).div_ceil(4) as u32,
    }
}

/// Pi `estimateTokens` for a `custom_message` entry's content (`custom` role → content chars/4,
/// `compaction.ts:279-283`). Used by branch budgeting.
pub fn estimate_custom_message_content(content: &serde_json::Value) -> u32 {
    // Pi `Math.ceil(chars / 4)` (`compaction.ts:282`).
    custom_content_chars(content).div_ceil(4) as u32
}

/// Pi `estimateTokens` for a `branchSummary`/`compactionSummary` message (`summary.length/4`,
/// `compaction.ts:288-292`).
pub fn estimate_summary_text(summary: &str) -> u32 {
    // Pi `Math.ceil(summary.length / 4)` (`compaction.ts:288-292`).
    utf16_len(summary).div_ceil(4) as u32
}

/// Chars in a `custom` message `content` (`string | (Text|Image)[]`), per Pi
/// `estimateTextAndImageContentChars` (`compaction.ts:236-250`).
fn custom_content_chars(content: &serde_json::Value) -> usize {
    match content {
        serde_json::Value::String(s) => utf16_len(s),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .map(|b| match b.get("type").and_then(serde_json::Value::as_str) {
                Some("text") => b.get("text").and_then(serde_json::Value::as_str).map_or(0, utf16_len),
                Some("image") => ESTIMATED_IMAGE_CHARS,
                _ => 0,
            })
            .sum(),
        _ => 0,
    }
}

fn content_chars(c: &Content) -> usize {
    match c {
        Content::Text { text, .. } => utf16_len(text),
        Content::Thinking { thinking, .. } => utf16_len(thinking),
        Content::ToolCall(tc) => {
            utf16_len(&tc.name)
                + serde_json::to_string(&tc.arguments).map(|s| utf16_len(&s)).unwrap_or(0)
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

/// Pi `estimateTokens` over a single [`RawContextMessage`] (`compaction.ts:256-296`): a
/// `bashExecution` costs `(command+output)/4` (counted even when `excludeFromContext`), a `custom`
/// costs its content chars/4, a `branchSummary`/`compactionSummary` costs `summary.length/4`, and
/// core roles match [`estimate_tokens`]. This is the raw per-role basis Pi uses for `tokensBefore`,
/// NOT the LLM-rendered text.
fn estimate_raw_message(msg: &RawContextMessage) -> u32 {
    match msg {
        RawContextMessage::Agent(a) => estimate_agent_message(a),
        RawContextMessage::CustomContent(c) => estimate_custom_message_content(c),
        RawContextMessage::BranchSummary(s) | RawContextMessage::CompactionSummary(s) => {
            estimate_summary_text(s)
        }
    }
}

/// Estimate live context over the **raw `AgentMessage`** context (Pi
/// `estimateContextTokens(buildSessionContext(pathEntries).messages)`, `compaction.ts:192-228,678`).
/// Identical anchor logic to [`estimate_context_tokens`] — prefer the last *valid* core-assistant
/// usage, then locally estimate the trailing messages — but the trailing estimate dispatches on the
/// raw role via [`estimate_raw_message`], so bash/summary/excluded-bash entries after the anchor are
/// counted exactly as Pi counts them (not as their `convertToLlm`-rendered, wrapper-padded text).
pub fn estimate_context_tokens_raw(messages: &[RawContextMessage]) -> ContextUsageEstimate {
    let mut last_usage_index = None;
    let mut usage_tokens = 0;
    for (i, m) in messages.iter().enumerate() {
        if let RawContextMessage::Agent(boxed) = m
            && let AgentMessage::Core(Message::Assistant(a)) = boxed.as_ref()
        {
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
        .map(estimate_raw_message)
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

    /// Estimate (chars/4) a `type:"message"` entry's raw `AgentMessage`, memoized by id; `0` for any
    /// non-message entry. This mirrors Pi `findCutPoint`'s accumulation, which `continue`s past every
    /// non-`message` entry and estimates `entry.message` directly (`compaction.ts:408-414`).
    pub fn estimate_message_entry(&self, entry: &Entry) -> u32 {
        if let Entry::Known(KnownEntry::Message { message, .. }) = entry {
            let id = entry.id();
            if let Ok(map) = self.map.lock()
                && let Some(v) = map.get(&id) {
                    return *v;
                }
            let est = estimate_agent_message(message);
            if let Ok(mut map) = self.map.lock() {
                map.insert(id, est);
            }
            est
        } else {
            0
        }
    }

    /// Drop a cached estimate (only needed on rare entry mutation).
    pub fn invalidate(&self, id: &EntryId) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(id);
        }
    }
}
