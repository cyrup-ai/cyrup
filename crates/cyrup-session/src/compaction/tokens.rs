//! Token estimation (arch-05 §3.2, R-05-024). A cheap chars/4 heuristic for the cut-point walk,
//! plus a trigger estimate that trusts the provider-reported `Usage` of the last valid assistant
//! message and only locally estimates the trailing tail. Per-immutable-entry estimates are cached.

use std::collections::HashMap;
use std::sync::Mutex;

use cyrup_core::{Content, EntryId, Message, StopReason, Usage};

use crate::agent_message::AgentMessage;
use crate::context::push_as_message;
use crate::entry::Entry;

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

/// Pi `estimateTokens` over the full `AgentMessage` union (`compaction.ts:256-296`): a
/// `bashExecution` costs `(command.length + output.length)/4`; a `custom` costs its content
/// chars/4; a `branchSummary`/`compactionSummary` costs `summary.length/4` (WITHOUT the LLM wrapper
/// prefix/suffix); core roles match [`estimate_tokens`]. This is what the cut-point walk
/// accumulates, so it must match Pi's raw per-message estimate, not the rendered LLM text.
pub fn estimate_agent_message(msg: &AgentMessage) -> u32 {
    match msg {
        AgentMessage::Core(m) => estimate_tokens(m),
        AgentMessage::BashExecution(b) => {
            // Pi `Math.ceil((command.length + output.length) / 4)` (`compaction.ts:284-287`).
            (utf16_len(&b.command) + utf16_len(&b.output)).div_ceil(4) as u32
        }
        AgentMessage::Custom(c) => custom_content_chars(&c.content).div_ceil(4) as u32,
        AgentMessage::BranchSummary(b) => estimate_summary_text(&b.summary),
        AgentMessage::CompactionSummary(c) => estimate_summary_text(&c.summary),
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
            // Byte-faithful to Pi's `getAssistantUsage`: `stopReason !== "aborted" &&
            // stopReason !== "error"` (compaction.ts:186-191). `pending` is deliberately NOT
            // excluded — Pi admits it, and a partial's `usage.input` is a real context reading.
            // Do not "tighten" this to `is_settled()`; that would be a divergence, not a fix.
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

/// Estimate live context over the **raw `AgentMessage`** context (Pi
/// `estimateContextTokens(buildSessionContext(pathEntries).messages)`, `compaction.ts:192-228,678`).
/// Identical anchor logic to [`estimate_context_tokens`] — prefer the last *valid* core-assistant
/// usage, then locally estimate the trailing messages — but the trailing estimate dispatches on the
/// raw role via [`estimate_agent_message`], so bash/summary/excluded-bash entries after the anchor
/// are counted exactly as Pi counts them (not as their `convertToLlm`-rendered, wrapper-padded text).
pub fn estimate_context_tokens_raw(messages: &[AgentMessage]) -> ContextUsageEstimate {
    let mut last_usage_index = None;
    let mut usage_tokens = 0;
    for (i, m) in messages.iter().enumerate() {
        if let AgentMessage::Core(Message::Assistant(a)) = m {
            // Byte-faithful to Pi's `getAssistantUsage`: `stopReason !== "aborted" &&
            // stopReason !== "error"` (compaction.ts:186-191). `pending` is deliberately NOT
            // excluded — Pi admits it, and a partial's `usage.input` is a real context reading.
            // Do not "tighten" this to `is_settled()`; that would be a divergence, not a fix.
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
        .map(estimate_agent_message)
        .fold(0u32, |a, b| a.saturating_add(b));
    ContextUsageEstimate {
        tokens: usage_tokens.saturating_add(trailing_tokens),
        usage_tokens,
        trailing_tokens,
        last_usage_index,
    }
}

/// Which projection of an entry a cached estimate was computed over. The two projections give
/// DIFFERENT numbers for the same entry (`Rendered` measures the `convertToLlm` text, wrapper
/// prefixes and all; `Raw` measures Pi's raw per-role basis), so they must not share a cache slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum EstimateKind {
    /// [`push_as_message`] + [`estimate_tokens`] — the LLM-rendered projection.
    Rendered,
    /// [`crate::context::raw_context_messages`] + [`estimate_agent_message`] — Pi's raw projection,
    /// what `findCutPoint` accumulates.
    Raw,
}

/// Per-entry token-estimate cache keyed by `(EntryId, EstimateKind)` so the trigger check and
/// cut-point walk do NOT re-tokenize the whole history each turn (R-05-024). Entries are immutable
/// once appended ⇒ estimates never invalidate.
#[derive(Default)]
pub struct TokenCache {
    map: Mutex<HashMap<(EntryId, EstimateKind), u32>>,
}

impl TokenCache {
    /// Memoized `compute`, keyed by `(entry id, kind)`.
    fn cached(&self, entry: &Entry, kind: EstimateKind, compute: impl FnOnce() -> u32) -> u32 {
        let key = (entry.id(), kind);
        if let Ok(map) = self.map.lock()
            && let Some(v) = map.get(&key) {
                return *v;
            }
        let est = compute();
        if let Ok(mut map) = self.map.lock() {
            map.insert(key, est);
        }
        est
    }

    /// Estimate (chars/4) the messages an entry contributes, memoized by entry id.
    pub fn estimate_entry(&self, entry: &Entry) -> u32 {
        self.cached(entry, EstimateKind::Rendered, || {
            let mut msgs = Vec::new();
            push_as_message(&mut msgs, entry);
            msgs.iter().map(estimate_tokens).fold(0u32, |a, b| a.saturating_add(b))
        })
    }

    /// Estimate (chars/4) the **raw context projection** of an entry, memoized by id — Pi
    /// `sessionEntryToContextMessages(entry).reduce((sum, m) => sum + estimateTokens(m), 0)`
    /// (`compaction.ts:418-422`, live path). Non-zero for `message`, `custom_message`, non-empty
    /// `branch_summary` and `compaction` entries; `0` for everything the context skips
    /// (`model_change`, `thinking_level_change`, `label`, `session_info`, `custom`, `Unknown`).
    ///
    /// This REPLACES the old `estimate_message_entry`, which returned `0` for every non-`message`
    /// entry per the HARNESS fork (`agent/src/harness/compaction/compaction.ts:412`,
    /// `if (entry.type !== "message") continue;`). Under that rule a `custom_message` holding tens
    /// of thousands of tokens of extension-injected context — or a `branch_summary` — contributed
    /// nothing to the keep-recent budget, so `find_cut_point` walked past it and kept far more than
    /// `keep_recent_tokens` (SESS-002).
    pub fn estimate_raw_entry(&self, entry: &Entry) -> u32 {
        self.cached(entry, EstimateKind::Raw, || {
            crate::context::raw_context_messages(entry)
                .iter()
                .map(estimate_agent_message)
                .fold(0u32, |a, b| a.saturating_add(b))
        })
    }

    /// Drop every cached estimate for an entry (only needed on rare entry mutation).
    pub fn invalidate(&self, id: &EntryId) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(&(id.clone(), EstimateKind::Rendered));
            map.remove(&(id.clone(), EstimateKind::Raw));
        }
    }
}
