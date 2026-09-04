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
        let mut s = Self {
            session_id,
            session_file,
            context_usage,
            ..Self::default()
        };
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

/// One row of the `/session` cost breakdown (Pi `UsageCostBreakdownEntry`,
/// `core/usage-totals.ts:30-34` @v0.83.0). PROV-036.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCostBreakdownEntry {
    /// `${provider}/${responseModel ?? model}` for attributable assistant usage, or the literal
    /// `Tools/summaries` bucket (Pi `:44`, `:47`, `:50`).
    pub key: String,
    pub cost: f64,
    /// `input + output + cacheRead + cacheWrite` (Pi `:66`).
    pub tokens: u64,
}

/// The literal bucket name pi gives usage it cannot attribute to a model (`usage-totals.ts:47`,
/// `:50`). It is a user-visible string and is byte-identical on purpose.
pub const TOOLS_SUMMARIES_KEY: &str = "Tools/summaries";

/// Group attributable assistant usage by model, and everything else into one bucket — 1:1 with pi
/// `getUsageCostBreakdown` (`core/usage-totals.ts:37-70` @v0.83.0). PROV-036.
///
/// Three details are load-bearing and all three are upstream's:
///
/// * the key is `provider/responseModel ?? model` (`:44`), so an OpenRouter `auto` route is
///   attributed to the model it actually RESOLVED to, not the one that was asked for;
/// * `toolResult` usage, branch summaries and compactions land in [`TOOLS_SUMMARIES_KEY`]
///   (`:46-52`) — "so the breakdown reconciles with the session total", which is why the bucket has
///   a name rather than being dropped; and
/// * rows with neither cost nor tokens are filtered out and the rest sort by cost DESCENDING
///   (`:68-69`).
///
/// The totals this sums are the same ones [`SessionStats::add_usage`] sums, so
/// `breakdown.iter().map(|e| e.cost).sum()` equals `SessionStats::cost` exactly.
pub fn usage_cost_breakdown(entries: &[Entry]) -> Vec<UsageCostBreakdownEntry> {
    use std::collections::BTreeMap;

    // Insertion-ordered, because the final `sort` is by cost and a stable sort must not reorder
    // equal-cost rows differently from upstream's `Map` iteration order.
    let mut order: Vec<String> = Vec::new();
    let mut totals: BTreeMap<String, (u64, f64)> = BTreeMap::new();
    let mut add = |key: String, usage: &Usage| {
        let slot = totals.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            (0, 0.0)
        });
        slot.0 += usage.input + usage.output + usage.cache_read + usage.cache_write;
        slot.1 += usage.cost.total;
    };

    for entry in entries {
        let Entry::Known(known) = entry else { continue };
        match known {
            // `entry.type === "message" && entry.message.role === "assistant"` (`:43-45`).
            KnownEntry::Message {
                message: AgentMessage::Core(Message::Assistant(a)),
                ..
            } => {
                let model = a.response_model.as_deref().unwrap_or(a.model.as_str());
                add(format!("{}/{model}", a.provider.as_str()), &a.usage);
            }
            // `role === "toolResult" && entry.message.usage` (`:46-48`) — the `&& usage` half
            // matters: a toolResult with no usage contributes no bucket at all.
            KnownEntry::Message {
                message: AgentMessage::Core(Message::ToolResult { usage: Some(u), .. }),
                ..
            } => {
                add(TOOLS_SUMMARIES_KEY.to_string(), u);
            }
            // `(branch_summary || compaction) && entry.usage` (`:49-51`).
            KnownEntry::Compaction { usage: Some(u), .. }
            | KnownEntry::BranchSummary { usage: Some(u), .. } => {
                add(TOOLS_SUMMARIES_KEY.to_string(), u);
            }
            _ => {}
        }
    }

    let mut rows: Vec<UsageCostBreakdownEntry> = order
        .into_iter()
        .filter_map(|key| {
            let (tokens, cost) = *totals.get(&key)?;
            // `.filter((entry) => entry.cost > 0 || entry.tokens > 0)` (`:68`).
            (cost > 0.0 || tokens > 0).then_some(UsageCostBreakdownEntry { key, cost, tokens })
        })
        .collect();
    // `.sort((a, b) => b.cost - a.cost)` (`:69`) — descending by cost. `sort_by` is stable, which
    // is what `Array.prototype.sort` is too, so equal-cost rows keep insertion order on both sides.
    rows.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

/// Project session entries onto the two facts `cyrup_provider::cache_stats` reads — the mechanical
/// adapter its module doc prescribes, so PROV-035's scan can run over a real session. PROV-035.
///
/// `Compaction`/`BranchSummary` are pi's resets (`cache-stats.ts:110-115`); an assistant message is
/// a settled turn; everything else — user messages, tool results, settings entries — is ignored and
/// specifically NOT a reset.
pub fn cache_scan_entries(
    entries: &[Entry],
) -> Vec<cyrup_provider::cache_stats::CacheScanEntry<'_>> {
    use cyrup_provider::cache_stats::CacheScanEntry;
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Known(KnownEntry::Compaction { .. } | KnownEntry::BranchSummary { .. }) => {
                CacheScanEntry::Reset
            }
            Entry::Known(KnownEntry::Message {
                message: AgentMessage::Core(Message::Assistant(a)),
                ..
            }) => CacheScanEntry::Assistant(a),
            _ => CacheScanEntry::Other,
        })
        .collect()
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
        Self {
            used_tokens: used,
            context_window,
            fraction,
        }
    }
}

/// The outcome of a compaction (Pi `CompactionResult`, agent-session.ts:1751-1757). Returned by
/// [`crate::AgentSession::compact`] and surfaced on the `compaction_end` event.
///
/// No `Eq`: `usage` (SEAM-034) carries `f64` cost fields, so only `PartialEq` is derivable — which
/// is also all any caller uses.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    /// Estimated token count of the rebuilt (post-compaction) context.
    ///
    /// `Option` to match pi's `estimatedTokensAfter?` (`core/compaction/compaction.ts:92`
    /// @v0.83.0), elided when absent so the key is missing rather than `null`. SEAM-034.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_tokens_after: Option<u64>,
    /// *"Usage from the LLM call(s) that generated this summary, if available"* — pi's own comment
    /// on `usage?: Usage` (`core/compaction/compaction.ts:93` @v0.83.0), on the wire at
    /// `modes/rpc/rpc-types.ts:171`. On a split turn pi records the SUM via `combineUsage`
    /// (`compaction.ts:99`); cyrup takes the value the compaction ENTRY already carries
    /// (`cyrup-session/src/entry.rs`'s `usage: Option<Usage>`), which is that same total.
    ///
    /// Without it a cost-tracking RPC client under-reported every compaction even though the
    /// session totals (SEAM-031) include it. Elided when absent, so existing goldens stay
    /// byte-identical. SEAM-034.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<cyrup_core::Usage>,
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod prov036_tests {
    use super::*;
    use cyrup_core::{Cost, EntryId, ProviderId, StopReason};
    use cyrup_session::entry::EntryBase;

    fn base(id: &str) -> EntryBase {
        EntryBase {
            id: EntryId::from(id),
            parent_id: None,
            timestamp: "2026-08-15T00:00:00.000Z".to_string(),
            extra: serde_json::Map::new(),
        }
    }

    fn usage(input: u64, output: u64, cost: f64) -> Usage {
        Usage {
            input,
            output,
            cost: Cost {
                total: cost,
                ..Cost::default()
            },
            ..Usage::default()
        }
    }

    fn assistant(
        id: &str,
        provider: &str,
        model: &str,
        response_model: Option<&str>,
        u: Usage,
    ) -> Entry {
        let mut a = AssistantMessage::errored(
            ProviderId::from(provider),
            model,
            None,
            StopReason::Stop,
            "",
        );
        a.error_message = None;
        a.stop_reason = StopReason::Stop;
        a.response_model = response_model.map(str::to_string);
        a.usage = u;
        Entry::Known(KnownEntry::Message {
            base: base(id),
            message: AgentMessage::Core(Message::Assistant(a)),
        })
    }

    /// PROV-036 — the breakdown attributes by `provider/responseModel ?? model`, buckets
    /// unattributable spend under the literal `Tools/summaries`, sorts by cost descending, and sums
    /// to `SessionStats::cost` exactly.
    ///
    /// **Red before the fix:** `grep -rn 'usage_cost_breakdown|UsageCostBreakdown' crates` returned
    /// ZERO — the function did not exist, so this did not compile. `/session` showed one cost total
    /// and a user who switched models could not see which one spent the money.
    #[test]
    fn prov036_breakdown_keys_attribute_sort_and_reconcile() {
        let entries = vec![
            // Two turns on the same model coalesce into one row.
            assistant(
                "a1",
                "anthropic",
                "claude-sonnet-5",
                None,
                usage(100, 10, 0.10),
            ),
            assistant(
                "a2",
                "anthropic",
                "claude-sonnet-5",
                None,
                usage(200, 20, 0.20),
            ),
            // An OpenRouter `auto` route: attributed to what it RESOLVED to (usage-totals.ts:44),
            // never to the requested id.
            assistant(
                "a3",
                "openrouter",
                "auto",
                Some("anthropic/claude-sonnet-4.5"),
                usage(400, 40, 0.31),
            ),
            // Compaction usage is real spend with no model to attribute it to.
            Entry::Known(KnownEntry::Compaction {
                base: base("c1"),
                summary: "s".to_string(),
                first_kept_entry_id: None,
                tokens_before: 0,
                details: None,
                usage: Some(usage(50, 5, 0.05)),
                from_hook: None,
            }),
            // As is a branch summary.
            Entry::Known(KnownEntry::BranchSummary {
                base: base("b1"),
                from_id: EntryId::from("a1"),
                summary: "s".to_string(),
                details: None,
                usage: Some(usage(10, 1, 0.01)),
                from_hook: None,
            }),
        ];

        let rows = usage_cost_breakdown(&entries);
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "openrouter/anthropic/claude-sonnet-4.5",
                "anthropic/claude-sonnet-5",
                TOOLS_SUMMARIES_KEY,
            ],
            "sorted by cost DESCENDING (usage-totals.ts:69); the auto route is keyed by its \
             RESOLVED model (`:44`) and the two unattributable entries share one bucket (`:47`,`:50`)"
        );
        assert_eq!(rows.len(), 3);
        assert!((rows[0].cost - 0.31).abs() < 1e-9);
        assert!(
            (rows[1].cost - 0.30).abs() < 1e-9,
            "two turns on one model coalesce"
        );
        assert!(
            (rows[2].cost - 0.06).abs() < 1e-9,
            "compaction + branch summary share the bucket"
        );
        assert_eq!(
            rows[1].tokens, 330,
            "input+output+cacheRead+cacheWrite (usage-totals.ts:66)"
        );

        // "so the breakdown reconciles with the session total" — pi's own comment at
        // `interactive-mode.ts:5663-5664`. This is the property that comment asserts.
        let stats = SessionStats::from_entries(&entries, "sid".to_string(), None, None);
        let summed: f64 = rows.iter().map(|r| r.cost).sum();
        assert!(
            (summed - stats.cost).abs() < 1e-9,
            "breakdown {summed} must reconcile with SessionStats::cost {}",
            stats.cost
        );
    }

    /// PROV-036 — a single-model session yields ONE row, which is what makes pi's
    /// `usageBreakdown.length > 1` render guard (`interactive-mode.ts:5699`) suppress the block.
    /// Also pins the `cost > 0 || tokens > 0` filter (`usage-totals.ts:68`): a zero-usage assistant
    /// contributes no row at all rather than a `$0.000` line.
    #[test]
    fn prov036_single_model_and_zero_usage_produce_no_extra_rows() {
        let one = vec![assistant("a1", "anthropic", "m", None, usage(10, 1, 0.01))];
        assert_eq!(usage_cost_breakdown(&one).len(), 1);

        let none = vec![assistant("a1", "anthropic", "m", None, usage(0, 0, 0.0))];
        assert!(
            usage_cost_breakdown(&none).is_empty(),
            "usage-totals.ts:68 filters rows with neither cost nor tokens"
        );
    }

    /// PROV-035 — the scan adapter really distinguishes pi's three entry classes. A compaction is a
    /// RESET (`cache-stats.ts:110-115`) and a tool result is NOT, which is the difference between
    /// "the context legitimately changed" and "we re-billed a prefix we already paid for".
    ///
    /// **Red before the fix:** `cache_scan_entries` did not exist and `cyrup_provider`'s
    /// `compute_cache_waste` had no caller anywhere in the workspace, so no session could be
    /// scanned at all — PROV-035's recorded residual.
    #[test]
    fn prov035_cache_scan_entries_maps_resets_assistants_and_ignores_the_rest() {
        use cyrup_provider::cache_stats::CacheScanEntry;
        let entries = vec![
            assistant("a1", "anthropic", "m", None, usage(10, 1, 0.01)),
            Entry::Known(KnownEntry::Compaction {
                base: base("c1"),
                summary: "s".to_string(),
                first_kept_entry_id: None,
                tokens_before: 0,
                details: None,
                usage: None,
                from_hook: None,
            }),
            Entry::Known(KnownEntry::Message {
                base: base("u1"),
                message: AgentMessage::Core(Message::User {
                    content: Vec::new(),
                    timestamp: 0,
                }),
            }),
        ];
        let scan = cache_scan_entries(&entries);
        assert!(matches!(scan[0], CacheScanEntry::Assistant(_)));
        assert!(
            matches!(scan[1], CacheScanEntry::Reset),
            "a compaction resets the scan"
        );
        assert!(
            matches!(scan[2], CacheScanEntry::Other),
            "a user message is ignored and specifically NOT a reset"
        );
    }
}
