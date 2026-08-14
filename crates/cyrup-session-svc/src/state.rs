//! Serializable read-only state views the RPC/print front-ends snapshot (arch-11 §3.1; the
//! `state.rs` module arch-11 §2.1 prescribes). These mirror Pi's `getSessionStats`
//! (agent-session.ts:3112, interface at :260-277), the `state` getter (agent-session.ts:861), and
//! `getContextUsage` (agent-session.ts:3164) — none of which mutate the session.

use cyrup_core::{AssistantMessage, Content, Message, Usage};
use cyrup_session::agent_message::AgentMessage;
use cyrup_session::entry::{Entry, KnownEntry};

/// Aggregate session statistics (Pi `SessionStats`, agent-session.ts:260-277).
///
/// SEAM-031 — this is a byte-level port of Pi's interface, field for field:
/// ```text
/// interface SessionStats {
///     sessionFile: string | undefined;
///     sessionId: string;
///     userMessages: number;
///     assistantMessages: number;
///     toolCalls: number;
///     toolResults: number;
///     totalMessages: number;
///     tokens: { input; output; cacheRead; cacheWrite; total };
///     cost: number;
///     contextUsage?: ContextUsage;
/// }
/// ```
/// cyrup previously answered `get_session_stats` with a cyrup-invented object
/// (`{messageCount,userMessageCount,assistantMessageCount,toolResultCount,inputTokens,outputTokens,
/// cacheTokens}`) — not one key of which a Pi-contract client can read — computed from the
/// LLM-flattened, **post-compaction** context. Pi's docstring (agent-session.ts:3107-3111) is
/// explicit that the aggregation runs over ALL session entries "including history that was compacted
/// away, so token/cost totals reflect what was actually billed across the session"; recomputing from
/// the rebuilt context silently DROPPED the reported spend at every compaction.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    /// The on-disk session file, absent for an in-memory session (Pi `sessionFile: string |
    /// undefined`, which `JSON.stringify` omits when undefined).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    pub session_id: String,
    pub user_messages: usize,
    pub assistant_messages: usize,
    /// `toolCall` content blocks across all assistant messages (Pi agent-session.ts:3140-3143).
    pub tool_calls: usize,
    pub tool_results: usize,
    /// Every `message` entry, whatever its role (Pi `totalMessages`, agent-session.ts:3126).
    pub total_messages: usize,
    pub tokens: StatsTokens,
    /// Summed `usage.cost.total` (Pi `addUsageToTotals`, usage-totals.ts:22-28).
    pub cost: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<StatsContextUsage>,
}

/// The `tokens` sub-object of [`SessionStats`] (Pi agent-session.ts:268-274). `cacheRead` and
/// `cacheWrite` are separate — cyrup used to collapse them into one `cacheTokens`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// `input + output + cacheRead + cacheWrite` (Pi agent-session.ts:3157).
    pub total: u64,
}

/// The optional `contextUsage` sub-object of [`SessionStats`], in Pi's shape (`ContextUsage`,
/// extensions/types.ts:288-294): `{tokens: number|null, contextWindow: number, percent: number|null}`.
///
/// Deliberately NOT the same type as this module's [`ContextUsage`], which is cyrup's own
/// `{usedTokens, contextWindow, fraction}` shape and is what `get_state`, the TUI footer and the
/// guest `ctx.getContextUsage()` capability read. Converging those onto Pi's spelling is a separate
/// divergence; this type exists so the `get_session_stats` wire is Pi-shaped today.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsContextUsage {
    /// `null` when the occupied-token count is unknown (Pi returns `tokens: null` right after a
    /// compaction, before the next LLM response, agent-session.ts:3188).
    pub tokens: Option<u64>,
    pub context_window: u64,
    /// Percentage (0-100) of the window, `null` whenever `tokens` is (Pi agent-session.ts:3201).
    pub percent: Option<f64>,
}

impl SessionStats {
    /// Compute the stats from ALL session entries (Pi `getSessionStats`, agent-session.ts:3112-3161).
    ///
    /// Walks `sessionManager.getEntries()`, not the rebuilt LLM context, and folds
    /// `branch_summary`/`compaction` `usage` back in (agent-session.ts:3120-3122) so a compaction
    /// does not erase the tokens it already billed.
    pub fn from_entries(
        entries: &[Entry],
        session_id: String,
        session_file: Option<String>,
        context_usage: Option<StatsContextUsage>,
    ) -> Self {
        let mut s = Self { session_id, session_file, context_usage, ..Self::default() };
        for entry in entries {
            let Entry::Known(known) = entry else { continue };
            match known {
                // `if ((entry.type === "branch_summary" || entry.type === "compaction") &&
                // entry.usage) addUsageToTotals(...)` — counted even though the entry is not a
                // message, because the summarization call spent real tokens (agent-session.ts:3120).
                KnownEntry::Compaction { usage, .. } | KnownEntry::BranchSummary { usage, .. } => {
                    if let Some(u) = usage {
                        s.add_usage(u);
                    }
                }
                // `if (entry.type !== "message") continue;` (agent-session.ts:3123).
                KnownEntry::Message { message, .. } => {
                    s.total_messages += 1;
                    match message {
                        AgentMessage::Core(Message::User { .. }) => s.user_messages += 1,
                        AgentMessage::Core(Message::ToolResult { usage, .. }) => {
                            s.tool_results += 1;
                            if let Some(u) = usage {
                                s.add_usage(u);
                            }
                        }
                        AgentMessage::Core(Message::Assistant(a)) => {
                            s.assistant_messages += 1;
                            s.tool_calls += a
                                .content
                                .iter()
                                .filter(|c| matches!(c, Content::ToolCall { .. }))
                                .count();
                            s.add_usage(&a.usage);
                        }
                        // `bashExecution`/`custom`/`branchSummary`/`compactionSummary` roles count
                        // toward `totalMessages` and nothing else — Pi's role switch has no arm for
                        // them either (agent-session.ts:3127-3145).
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        s.tokens.total =
            s.tokens.input + s.tokens.output + s.tokens.cache_read + s.tokens.cache_write;
        s
    }

    /// Pi `addUsageToTotals` (usage-totals.ts:22-28).
    fn add_usage(&mut self, u: &Usage) {
        self.tokens.input += u.input;
        self.tokens.output += u.output;
        self.tokens.cache_read += u.cache_read;
        self.tokens.cache_write += u.cache_write;
        self.cost += u.cost.total;
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
    /// `used_tokens / context_window` (0 when the window is unknown).
    ///
    /// **Unclamped, and may exceed `1.0`.** Pi computes `const percent = (estimate.tokens /
    /// contextWindow) * 100` with no cap (agent-session.ts:3211) and the footer prints it verbatim
    /// (`footer.ts:151`), so an over-budget context reads e.g. `112.3%` in `error` red. Clamping to
    /// `[0, 1]` here made every overflow look like a tidy 100%, hiding the one number that tells a
    /// user a compaction is overdue. The only consumer is
    /// [`AgentSession::stats_context_usage`](crate::AgentSession::stats_context_usage), which
    /// multiplies it by 100 to build pi's `ContextUsage.percent`.
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
            used as f64 / context_window as f64
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
    /// `None` on a modelless session — pi's `RpcSessionState.model` is `Model | undefined`
    /// (rpc-types.ts:95) because `AgentSession.model` is (agent-session.ts:866-868). SEAM-075.
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_name: Option<String>,
    pub is_streaming: bool,
    pub message_count: usize,
    pub pending_message_count: usize,
    pub stats: SessionStats,
    pub context_usage: ContextUsage,
}
