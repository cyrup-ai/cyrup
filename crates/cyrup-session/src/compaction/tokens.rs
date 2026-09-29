//! Token estimation (arch-05 §3.2, R-05-024). A cheap chars/4 heuristic for the cut-point walk,
//! plus a trigger estimate that trusts the provider-reported `Usage` of the last valid assistant
//! message and only locally estimates the trailing tail. Per-immutable-entry estimates are cached.

use std::collections::HashMap;
use std::sync::Mutex;

use cyrup_core::{Content, EntryId, Message, StopReason, Usage};

use crate::agent_message::AgentMessage;
use crate::context::push_as_message;
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
/// `compaction.ts:266-301`).
///
/// The per-role content functions are DIFFERENT upstream and are reproduced here:
/// * `user` (`:270-275`) and `toolResult` (`:289-293`) both call
///   `estimateTextAndImageContentChars` (`:246-260`), which counts **text and image only** — a
///   `thinking` or `toolCall` block contributes 0.
/// * `assistant` (`:276-288`) walks the blocks itself and counts **text, thinking and toolCall
///   only** — an `image` block contributes 0.
///
/// Applying one content function to every role over-counted an assistant image by
/// `ESTIMATED_IMAGE_CHARS` (1200 tokens) and counted thinking/toolCall blocks that reach a
/// user/toolResult message through the read-tolerant deserializer. This estimate feeds the
/// persisted `tokensBefore`, the keep-recent cut-point walk and the raw trigger, so the divergence
/// moves the cut point. `cyrup_provider::estimate_message_tokens`
/// (`cyrup-provider/src/utils/estimate.rs`) is the second port of the same function and already
/// splits the arms this way.
pub fn estimate_tokens(msg: &Message) -> u32 {
    let chars: usize = match msg {
        // pi `compaction.ts:324-334` @v0.87.1 — the system case is its OWN arm and counts content
        // chars, then each non-`null` section's length, then `JSON.stringify(toolsAdded).length`.
        // `toolsRemoved` is deliberately NOT counted: this is the coding-agent's own cut-point
        // estimator, and it differs from `cyrup_provider::estimate_message_tokens`, which ports
        // `packages/ai/src/utils/estimate.ts:49-55` and DOES charge both lists. Two upstream
        // functions, two formulas (PROV-083a).
        Message::System(m) => {
            let mut chars: usize = m.content.iter().map(text_and_image_chars).sum();
            if let Some(sections) = &m.sections {
                for section in sections.values() {
                    chars += utf16_len(section);
                }
            }
            if !m.tools_added.is_empty() {
                // `JSON.stringify(system.toolsAdded).length`. Serialization of a `ToolDef` vec
                // cannot fail; an impossible error contributes 0 rather than panicking.
                chars += serde_json::to_string(&m.tools_added).map_or(0, |s| utf16_len(&s));
            }
            chars
        }
        Message::User { content, .. } | Message::ToolResult { content, .. } => {
            content.iter().map(text_and_image_chars).sum()
        }
        Message::Assistant(a) => a.content.iter().map(assistant_content_chars).sum(),
    };
    // Pi rounds UP: `Math.ceil(chars / 4)` (`compaction.ts:274,288,293,297,301`). Flooring would
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
            .map(
                |b| match b.get("type").and_then(serde_json::Value::as_str) {
                    Some("text") => b
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .map_or(0, utf16_len),
                    Some("image") => ESTIMATED_IMAGE_CHARS,
                    _ => 0,
                },
            )
            .sum(),
        _ => 0,
    }
}

/// Pi `estimateTextAndImageContentChars` (`compaction.ts:246-260`) over a typed block: text and
/// image only — `thinking` and `toolCall` fall through the `if/else if` chain and contribute 0.
/// Used for the `user`, `toolResult` and `custom` roles.
fn text_and_image_chars(c: &Content) -> usize {
    match c {
        Content::Text { text, .. } => utf16_len(text),
        Content::Image { .. } => ESTIMATED_IMAGE_CHARS,
        Content::Thinking { .. } | Content::ToolCall(_) => 0,
    }
}

/// Pi's inline `assistant` accumulator (`compaction.ts:276-288`): text, thinking and toolCall only
/// — an `image` block on an assistant message contributes **nothing**.
fn assistant_content_chars(c: &Content) -> usize {
    match c {
        Content::Text { text, .. } => utf16_len(text),
        Content::Thinking { thinking, .. } => utf16_len(thinking),
        Content::ToolCall(tc) => {
            utf16_len(&tc.name)
                + serde_json::to_string(&tc.arguments)
                    .map(|s| utf16_len(&s))
                    .unwrap_or(0)
        }
        Content::Image { .. } => 0,
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

/// Pi `estimateProjectedContextTokens` (`core/compaction/compaction.ts:248-283` @v0.87.1) — the
/// estimate every shipped upstream reader of live context size goes through
/// (`agent-session.ts:596`, `:758`, `:2708`, `:3893`, `compaction.ts:919` @v0.87.1).
///
/// [`estimate_context_tokens_raw`] anchors on the newest provider-reported assistant `usage` and
/// only estimates the tail after it. That anchor is a reading of the context **as the provider saw
/// it at the time of that response**, so it is only usable while nothing has since changed what the
/// earlier context projects to. A `context_edit` (SESS-052) and a `compaction` both do exactly that:
/// an edit can omit or shrink an entry that is *inside* the usage anchor's own accounting, and a
/// compaction replaces a whole prefix with a summary. Upstream therefore DISCARDS the anchor when
/// either entry appears LATER in the branch than the entry the anchor came from, and falls back to
/// a pure chars/4 estimate of the whole projection.
///
/// Concretely, on the row's own scenario: pi's `_omitRecoveryAttempt` (`agent-session.ts:1015-1031`
/// @v0.87.1) appends a `null` `context_edit` for an abandoned attempt, which by construction
/// post-dates the usage-bearing assistant message it omits. Trusting the anchor there keeps the
/// stale pre-edit reading, over-reports context and auto-compacts early.
///
/// `path` is the FULL branch (Pi `sessionManager.getBranch()` / `pathEntries`), not the
/// compaction-admitted projection: the invalidation scan is over raw branch order, so a
/// `context_edit` whose target was summarized away still counts as invalidating, exactly as
/// upstream's `branchEntries.findIndex` / reverse walk does.
///
/// The fallback is upstream's, arm for arm (`compaction.ts:277-282`): the REPLAYED current system
/// message counted ONCE, then every `message.role !== "system"`.
///
/// This used to be a `CYRUP-DELTA` claiming both halves were no-ops here, on the reasoning that
/// "cyrup's [`AgentMessage`] union has no system arm at all" and that a system prompt "never becomes
/// a session entry". That was TRUE when it was written, and PROV-083a is what ended it: adding the
/// [`Message::System`] arm (`cyrup-core/src/message/conversation.rs`) and
/// [`crate::agent_message::MessageRole::System`] turned both halves false in one step, and the delta
/// then read as a licence to skip two arms that had become load-bearing. Concretely, with that arm
/// present:
///
/// * a pi ≥v0.86 `{"type":"message","message":{"role":"system",…}}` entry deserializes as a
///   KNOWN message entry rather than `Entry::Unknown` — pi writes such entries itself
///   (`agent-session.ts:1420`,`:1441` @v0.87.1);
/// * [`crate::context::context_message_role`] returns `Some(System)` for it, so it IS a projected
///   message and the `role !== "system"` filter finally has something to exclude.
///
/// So both of upstream's arms became load-bearing on a resumed pi file, and summing every projected
/// message instead over-counted it. pi replays the system messages into ONE current message —
/// `content` joined `"\n\n"`, `sections` patched by name with a `null` DELETING, tools resolved —
/// and charges that once; the per-message sum charges every system entry separately, including ones
/// whose sections a later entry deleted. Two system entries where the second nulls a 60-char section
/// the first added measured 23 tokens here against upstream's 8.
///
/// [`cyrup_provider::get_current_system_message`] is pi's `getCurrentSystemMessage`
/// (`packages/ai/src/utils/transcript.ts:73-101` @v0.87.1) and [`estimate_tokens`]'s
/// `Message::System` arm is pi's `case "system"` (`compaction.ts:324-334`) — both landed with
/// PROV-083a, so this port is those two composed and is COUPLED to that row: unpicking PROV-083a
/// unpicks this with it.
pub fn estimate_projected_context_tokens(path: &[&Entry]) -> ContextUsageEstimate {
    let tagged = crate::context::build_context_agent_messages_tagged(path);
    let messages: Vec<AgentMessage> = tagged.iter().map(|(_, m)| m.clone()).collect();
    let estimate = estimate_context_tokens_raw(&messages);

    // Pi walks `projection.entries` accumulating `entry.messages.length` to find which entry owns
    // `lastUsageIndex` (`compaction.ts:252-261`). The tagged projection already carries that
    // ownership per message, so the walk is one index read and the two cannot drift.
    if let Some(i) = estimate.last_usage_index
        && let Some((usage_entry_id, _)) = tagged.get(i)
    {
        let usage_entry_index = path.iter().position(|e| &e.id() == usage_entry_id);
        let latest_invalidating = path.iter().rposition(|e| {
            matches!(
                e,
                Entry::Known(KnownEntry::ContextEdit { .. } | KnownEntry::Compaction { .. })
            )
        });
        // Pi compares raw array indices with `-1` standing for "not found"
        // (`compaction.ts:269-276`), and `usageEntryIndex > latestInvalidatingEntryIndex` KEEPS the
        // anchor. With no invalidating entry that is `n > -1` ⇒ always keep; with an unlocatable
        // usage entry it is `-1 > k` ⇒ never keep. `isize` reproduces both without a special case.
        let usage_idx = usage_entry_index.map_or(-1_isize, |x| x as isize);
        let invalid_idx = latest_invalidating.map_or(-1_isize, |x| x as isize);
        if usage_idx > invalid_idx {
            return estimate;
        }
    }

    // `const currentSystem = getCurrentSystemMessage(projection.messages);`
    // `let tokens = currentSystem ? estimateTokens(currentSystem) : 0;` (`compaction.ts:277-278`).
    // Only a `Core` message can be a system message, so narrowing to `Message` loses nothing —
    // `get_current_tools`, which `get_current_system_message` consults for `toolsAdded`, reads system
    // messages and nothing else.
    let core: Vec<Message> = messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Core(c) => Some(c.clone()),
            _ => None,
        })
        .collect();
    let mut tokens = cyrup_provider::get_current_system_message(&core)
        .map_or(0, |s| estimate_tokens(&Message::System(s)));
    // `for (const message of projection.messages) if (message.role !== "system") tokens +=
    // estimateTokens(message);` (`:279-281`) — the replayed head above already carries every system
    // message's current contribution, so charging them again here would double-count.
    for m in &messages {
        if !matches!(m, AgentMessage::Core(Message::System(_))) {
            tokens = tokens.saturating_add(estimate_agent_message(m));
        }
    }
    ContextUsageEstimate {
        tokens,
        usage_tokens: 0,
        trailing_tokens: tokens,
        last_usage_index: None,
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
            && let Some(v) = map.get(&key)
        {
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
            msgs.iter()
                .map(estimate_tokens)
                .fold(0u32, |a, b| a.saturating_add(b))
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
