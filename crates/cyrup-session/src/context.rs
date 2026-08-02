//! Context building (arch-04 §6.3, R-04-011/012/013). The leaf→root walk lives on
//! `SessionManager`; this module owns the result type, the entry→message conversions, and the
//! standalone `build_session_context`-style message assembly (Pi `buildSessionContext`,
//! `session-manager.ts:325-433`).

use cyrup_core::{Content, Message, ModelRef};

use crate::agent_message::{
    custom_to_message, AgentMessage, BranchSummaryMessage, CompactionSummaryMessage,
    CustomRoleMessage, MessageRole,
};
use crate::entry::{Entry, KnownEntry};

/// Compaction-summary wrapper (Pi `COMPACTION_SUMMARY_PREFIX`/`SUFFIX`, `messages.ts:11-17`). The
/// model conditions on this exact text, so it is byte-1:1 with Pi.
pub const COMPACTION_SUMMARY_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

/// Branch-summary wrapper (Pi `BRANCH_SUMMARY_PREFIX`/`SUFFIX`, `messages.ts:19-24`).
pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";
pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

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

/// Append the LLM-message form of an entry, per R-04-006/013 and Pi `buildSessionContext`'s
/// `appendMessage` (`session-manager.ts:389-399`) composed with `convertToLlm`:
/// - `Message` → rendered via [`AgentMessage::push_llm`] (`bashExecution`/`custom` roles become
///   user messages; an `excludeFromContext` bash message is dropped);
/// - `CustomMessage` (entry) → a user message;
/// - `BranchSummary` (non-empty) → the wrapped branch-summary message;
/// - `Compaction` → the wrapped compaction-summary message (Pi `sessionEntryToContextMessages`'s
///   `compaction` arm, `session-manager.ts:404-406`, composed with `convertToLlm`'s
///   `compactionSummary` arm). Reachable only for an *older* compaction that survives inside the
///   window kept by a LATER one — the governing (latest) compaction is prepended by the path
///   builders and excluded from their per-entry loops by range, exactly as Pi's
///   `buildContextEntries` keeps it at the head and iterates `0..compactionIdx`
///   (`session-manager.ts:441-453`);
/// - everything else (`Custom`, `Label`, `ModelChange`, `ThinkingLevelChange`, `SessionInfo`,
///   `Unknown`) → skipped.
pub fn push_as_message(out: &mut Vec<Message>, e: &Entry) {
    if let Entry::Known(k) = e {
        match k {
            KnownEntry::Message { message, .. } => message.push_llm(out),
            KnownEntry::CustomMessage { content, base, .. } => {
                out.push(custom_to_message(content, parse_entry_ts(&base.timestamp)));
            }
            KnownEntry::BranchSummary { summary, base, .. } if !summary.is_empty() => {
                out.push(branch_summary_message(summary, parse_entry_ts(&base.timestamp)));
            }
            KnownEntry::Compaction { summary, tokens_before, base, .. } => {
                out.push(compaction_summary_message(
                    summary,
                    *tokens_before,
                    parse_entry_ts(&base.timestamp),
                ));
            }
            _ => {}
        }
    }
}

/// Compaction summary rendered as the FIRST message of a compacted context (R-04-012, Pi
/// `convertToLlm` compactionSummary arm, `messages.ts:176-183`). The persisted `tokensBefore` count
/// is NOT injected into the prompt (Pi only embeds the summary text). `timestamp` is the originating
/// entry's parsed timestamp, matching Pi `createCompactionSummaryMessage`
/// (`messages.ts:111-120`), which sets `timestamp: new Date(entry.timestamp).getTime()`.
pub fn compaction_summary_message(summary: &str, _tokens_before: u64, timestamp: i64) -> Message {
    let text = format!("{COMPACTION_SUMMARY_PREFIX}{summary}{COMPACTION_SUMMARY_SUFFIX}");
    Message::User { content: vec![Content::text(text)], timestamp }
}

/// A `BranchSummary` rendered as the wrapped user-form note (Pi `messages.ts:170-175`).
/// `timestamp` is the originating entry's parsed timestamp, matching Pi
/// `createBranchSummaryMessage` (`messages.ts:100-107`).
pub fn branch_summary_message(summary: &str, timestamp: i64) -> Message {
    let text = format!("{BRANCH_SUMMARY_PREFIX}{summary}{BRANCH_SUMMARY_SUFFIX}");
    Message::User { content: vec![Content::text(text)], timestamp }
}

/// The compaction entry that governs a path, if any (the latest one — Pi takes the last on the
/// path, `session-manager.ts:377-379`).
fn latest_compaction(path: &[&Entry]) -> Option<usize> {
    path.iter().rposition(|e| matches!(e, Entry::Known(KnownEntry::Compaction { .. })))
}

/// Build the active-path LLM messages exactly as Pi `buildSessionContext` does (the message-list
/// half), handling a compaction boundary: emit the compaction summary first, then the kept entries
/// from `firstKeptEntryId` to the compaction, then everything after (Pi `session-manager.ts:382-430`).
/// Used both by [`crate::manager::SessionManager::build_context`] and by compaction's
/// `tokensBefore`/trigger estimation so they measure the same reconstructed context.
pub fn build_context_messages(path: &[&Entry]) -> Vec<Message> {
    let mut messages = Vec::new();
    match latest_compaction(path).and_then(|i| path.get(i).copied().map(|e| (i, e))) {
        Some((cpos, Entry::Known(KnownEntry::Compaction {
            summary,
            first_kept_entry_id,
            tokens_before,
            base,
            ..
        }))) => {
            messages.push(compaction_summary_message(
                summary,
                *tokens_before,
                parse_entry_ts(&base.timestamp),
            ));
            if let Some(before) = path.get(..cpos) {
                let mut keeping = false;
                for e in before {
                    if &e.id() == first_kept_entry_id {
                        keeping = true;
                    }
                    if keeping {
                        push_as_message(&mut messages, e);
                    }
                }
            }
            if let Some(after) = path.get(cpos + 1..) {
                for e in after {
                    push_as_message(&mut messages, e);
                }
            }
        }
        _ => {
            for e in path {
                push_as_message(&mut messages, e);
            }
        }
    }
    messages
}

/// Append the **raw `AgentMessage`** form of an entry — Pi `sessionEntryToContextMessages`
/// (`session-manager.ts:383-408`) without `convertToLlm` rendering. The arms are: `message` →
/// `entry.message`, `custom_message` → `createCustomMessage(...)`, non-empty `branch_summary` →
/// `createBranchSummaryMessage(...)`, `compaction` → `createCompactionSummaryMessage(...)`.
/// Everything else is skipped.
///
/// The `compaction` arm only fires for an *older* compaction kept inside a later one's window: the
/// path builders prepend the governing compaction themselves and exclude it from the per-entry loop
/// by range, mirroring Pi's `buildContextEntries` (`session-manager.ts:441-453`), which puts the
/// latest compaction at the head of the list and then iterates `0..compactionIdx`.
///
/// Keeping the `bashExecution` / `custom` / `branchSummary` / `compactionSummary` roles intact is
/// what lets [`crate::compaction::tokens::estimate_context_tokens_raw`] dispatch on them exactly as
/// Pi `estimateTokens` does (`compaction.ts:256-296`): a `bashExecution` costs
/// `(command+output)/4` **even when `excludeFromContext`** (Pi's raw context never drops it), and a
/// summary costs `summary.length/4` **without** the LLM wrapper prefix/suffix.
fn push_as_raw(out: &mut Vec<AgentMessage>, e: &Entry) {
    if let Entry::Known(k) = e {
        match k {
            KnownEntry::Message { message, .. } => out.push(message.clone()),
            KnownEntry::CustomMessage { content, custom_type, display, details, base } => {
                out.push(AgentMessage::Custom(CustomRoleMessage {
                    custom_type: custom_type.clone(),
                    content: content.clone(),
                    display: *display,
                    details: details.clone(),
                    timestamp: parse_entry_ts(&base.timestamp),
                }));
            }
            KnownEntry::BranchSummary { summary, from_id, base, .. } if !summary.is_empty() => {
                out.push(AgentMessage::BranchSummary(BranchSummaryMessage {
                    summary: summary.clone(),
                    from_id: from_id.clone(),
                    timestamp: parse_entry_ts(&base.timestamp),
                }));
            }
            KnownEntry::Compaction { summary, tokens_before, base, .. } => {
                out.push(AgentMessage::CompactionSummary(CompactionSummaryMessage {
                    summary: summary.clone(),
                    tokens_before: *tokens_before,
                    timestamp: parse_entry_ts(&base.timestamp),
                }));
            }
            _ => {}
        }
    }
}

/// The raw-context projection of ONE entry — Pi `sessionEntryToContextMessages(entry)`
/// (`session-manager.ts:383-408`): `message` → its raw `AgentMessage`, `custom_message` →
/// `createCustomMessage(...)`, a NON-EMPTY `branch_summary` → `createBranchSummaryMessage(...)`,
/// `compaction` → `createCompactionSummaryMessage(...)`, everything else → nothing.
///
/// An entry is "context-visible" iff this is non-empty — the predicate Pi's live `findCutPoint`
/// uses both for its token accumulation and for its back-scan (`compaction.ts:418-446`). Use
/// [`context_message_role`] where only the classification is needed: it answers the same question
/// without cloning the message.
pub fn raw_context_messages(e: &Entry) -> Vec<AgentMessage> {
    let mut out = Vec::new();
    push_as_raw(&mut out, e);
    out
}

/// The role of the single message [`raw_context_messages`] would project this entry to, or `None`
/// when the entry is context-invisible (`sessionEntryToContextMessages(entry).length === 0`).
///
/// This is the classification half of the projection, extracted so the cut-point layer can run Pi's
/// `isCutPointMessage` / `isTurnStartMessage` predicates and its back-scan visibility test over
/// every entry in a range WITHOUT cloning summaries and assistant content. It must stay in lockstep
/// with [`raw_context_messages`]: every arm that projects a message here must project one there,
/// with the same role — notably the `!summary.is_empty()` guard on `branch_summary`, which is Pi's
/// `if (entry.type === "branch_summary" && entry.summary)` (`session-manager.ts:400`).
pub fn context_message_role(e: &Entry) -> Option<MessageRole> {
    match e {
        Entry::Known(KnownEntry::Message { message, .. }) => Some(message.role()),
        Entry::Known(KnownEntry::CustomMessage { .. }) => Some(MessageRole::Custom),
        Entry::Known(KnownEntry::BranchSummary { summary, .. }) if !summary.is_empty() => {
            Some(MessageRole::BranchSummary)
        }
        Entry::Known(KnownEntry::Compaction { .. }) => Some(MessageRole::CompactionSummary),
        _ => None,
    }
}

/// Build the active-path **raw `AgentMessage`** context exactly as Pi `buildSessionContext` does
/// (`session-manager.ts:382-430`) — Pi's `messages` field is `AgentMessage[]`, roles intact.
/// Handles the compaction boundary identically to [`build_context_messages`]: emit the compaction
/// summary first, then kept entries from `firstKeptEntryId`, then everything after. This is the input
/// to `tokensBefore` / `should_compact` so the estimate matches Pi
/// `estimateContextTokens(buildSessionContext(pathEntries).messages)` (`compaction.ts:678`) rather
/// than estimating over the LLM-rendered text.
pub fn build_context_agent_messages(path: &[&Entry]) -> Vec<AgentMessage> {
    let mut messages = Vec::new();
    match latest_compaction(path).and_then(|i| path.get(i).copied().map(|e| (i, e))) {
        Some((cpos, Entry::Known(KnownEntry::Compaction {
            summary,
            first_kept_entry_id,
            tokens_before,
            base,
            ..
        }))) => {
            messages.push(AgentMessage::CompactionSummary(CompactionSummaryMessage {
                summary: summary.clone(),
                tokens_before: *tokens_before,
                timestamp: parse_entry_ts(&base.timestamp),
            }));
            if let Some(before) = path.get(..cpos) {
                let mut keeping = false;
                for e in before {
                    if &e.id() == first_kept_entry_id {
                        keeping = true;
                    }
                    if keeping {
                        push_as_raw(&mut messages, e);
                    }
                }
            }
            if let Some(after) = path.get(cpos + 1..) {
                for e in after {
                    push_as_raw(&mut messages, e);
                }
            }
        }
        _ => {
            for e in path {
                push_as_raw(&mut messages, e);
            }
        }
    }
    messages
}

/// Best-effort RFC3339 → unix-ms parse for an entry timestamp (Pi passes the entry timestamp through
/// `createCustomMessage`). Defaults to 0 on a non-RFC3339 string.
pub fn parse_entry_ts(ts: &str) -> i64 {
    time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
        .unwrap_or(0)
}
