//! Extension hook payloads + the injected dispatcher trait (arch-05 §3.9, R-05-019..023). Payloads
//! are plain serde structs (they cross the WASM boundary as serialized events per ADR-0002); the
//! dispatcher is injected so `cyrup-session` does not depend on `cyrup-ext`.

use cyrup_core::{CancelToken, EntryId, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_message::AgentMessage;
use crate::compaction::error::CompactionError;
use crate::compaction::files::CompactionDetails;
use crate::compaction::settings::CompactionSettings;
use crate::entry::Entry;

/// Why compaction ran (R-05-019 `reason` field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

/// The appended compaction entry payload (R-05-009/021), restated for hook events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEntry {
    pub id: EntryId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<EntryId>,
    pub summary: String,
    pub first_kept_entry_id: EntryId,
    pub tokens_before: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_hook: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// Token spend of the summarization call(s) (Pi `CompactionEntry.usage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// The appended branch-summary entry payload (R-05-016/022).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryEntry {
    pub id: EntryId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<EntryId>,
    pub summary: String,
    pub from_id: EntryId,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_hook: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// Token spend of the branch-summarization call (Pi `BranchSummaryEntry.usage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Input to the before-compact hook (R-05-019).
///
/// The two message lists are RAW [`AgentMessage`]s — roles (`bashExecution`, `custom`,
/// `branchSummary`, `compactionSummary`) intact, `excludeFromContext` bash commands included — so a
/// guest sees exactly what Pi's `CompactionPreparation` carries
/// (`coding-agent/src/core/compaction/compaction.ts:690-700`) and can dispatch on `role` /
/// `customType` the way an extension ported from Pi expects.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeCompactEvent {
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub previous_summary: Option<String>,
    pub file_ops: CompactionDetails,
    pub tokens_before: u64,
    pub first_kept_entry_id: EntryId,
    pub settings: CompactionSettings,
    pub branch_entries: Vec<Entry>,
    pub custom_instructions: Option<String>,
    pub reason: CompactionReason,
    pub will_retry: bool,
}

/// The before-compact hook's decision (R-05-020).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "decision")]
pub enum BeforeCompactDecision {
    /// Run the default compaction.
    Proceed,
    /// Cancel compaction (R-05-020a).
    Cancel,
    /// Supply a custom compaction, using the extension's own model/format (R-05-020b).
    Custom {
        summary: String,
        first_kept_entry_id: EntryId,
        tokens_before: u64,
        #[serde(default)]
        details: Option<Value>,
        /// Usage the hook reports for its own summarization, if any (Pi threads
        /// `extensionCompaction.usage` into `appendCompaction`, `agent-session.ts:1844,1872`).
        #[serde(default)]
        usage: Option<Usage>,
    },
}

/// An extension-supplied compaction override (Pi `SessionBeforeCompactResult.compaction`, a
/// `CompactionResult`). Supplied by the session-service producer AFTER firing the external
/// `session_before_compact` extension hook against a real [`super::CompactionPreparation`], and fed
/// to [`super::Compactor::run_compaction_prepared`] so the override's `summary`/`details` replace the
/// default model summarization; the appended entry is marked `fromExtension`. Fields left `None`
/// inherit the prepared cut (`first_kept_entry_id`, `tokens_before`) or an empty details bag.
#[derive(Clone, Debug, Default)]
pub struct CompactionOverride {
    pub summary: String,
    pub first_kept_entry_id: Option<EntryId>,
    pub tokens_before: Option<u64>,
    pub details: Option<Value>,
    /// The guest's reported summarization usage (Pi `CompactionResult.usage`), persisted verbatim
    /// on the appended entry.
    pub usage: Option<Usage>,
}

/// Post-compact notification (R-05-021).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostCompactEvent {
    pub entry: CompactionEntry,
    pub from_extension: bool,
    pub reason: CompactionReason,
    pub will_retry: bool,
}

/// Input to the before-tree hook (R-05-022).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeTreeEvent {
    pub target_id: EntryId,
    pub old_leaf_id: Option<EntryId>,
    pub common_ancestor_id: Option<EntryId>,
    pub entries_to_summarize: Vec<Entry>,
    pub user_wants_summary: bool,
}

/// The instruction/label overrides a before-tree hook may return ALONGSIDE its
/// proceed/cancel/custom-summary decision.
///
/// Pi's `session_before_tree` handler returns one `SessionBeforeTreeResult` object and the caller
/// reads four independent fields off it: `result.cancel`, `result.summary`,
/// `result.customInstructions`, `result.replaceInstructions` and `result.label`
/// (`agent-session.ts:2958-2976`) — the instruction/label reads are NOT gated on which of
/// `cancel`/`summary` was set, so a guest may steer the *default* summarizer's prompt without
/// supplying a summary of its own. `custom_instructions` / `replace_instructions` are honoured by
/// [`super::branch::generate_branch_summary_with_instructions`]
/// (`branch-summarization.ts:326-334`); `label` is attached to the produced summary entry, or to
/// the navigation target when no summary was produced (`agent-session.ts:3050-3064`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeTreeOverrides {
    /// Pi `result.customInstructions` (`agent-session.ts:2968-2970`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    /// Pi `result.replaceInstructions` (`agent-session.ts:2971-2973`). Only load-bearing when
    /// `custom_instructions` is also set — Pi's selector is
    /// `if (replaceInstructions && customInstructions)` (`branch-summarization.ts:328-329`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_instructions: Option<bool>,
    /// Pi `result.label` (`agent-session.ts:2974-2976`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The before-tree hook's decision (R-05-022).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "decision")]
pub enum BeforeTreeDecision {
    Proceed {
        /// Pi reads the instruction/label overrides off the same result object regardless of
        /// whether a summary was supplied (`agent-session.ts:2968-2976`).
        #[serde(default, flatten)]
        overrides: BeforeTreeOverrides,
    },
    Cancel,
    CustomSummary {
        summary: String,
        #[serde(default)]
        details: Option<Value>,
        #[serde(default, flatten)]
        overrides: BeforeTreeOverrides,
    },
}

impl BeforeTreeDecision {
    /// `Proceed` with no overrides — the common case, and what a hook that does not subscribe
    /// returns.
    pub fn proceed() -> Self {
        BeforeTreeDecision::Proceed {
            overrides: BeforeTreeOverrides::default(),
        }
    }
}

/// Post-tree notification (R-05-022).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostTreeEvent {
    pub entry: Option<BranchSummaryEntry>,
    pub target_id: EntryId,
    pub from_extension: bool,
}

/// Injected hook dispatcher. The production impl bridges to `cyrup-ext`; the test impl is a scripted
/// stub. `cyrup-session` does NOT depend on `cyrup-ext`.
#[allow(async_fn_in_trait)]
pub trait CompactionHooks: Send + Sync {
    /// Fired before automatic or manual compaction; may cancel or supply a custom compaction.
    async fn before_compact(
        &self,
        ev: &BeforeCompactEvent,
        cancel: CancelToken,
    ) -> Result<BeforeCompactDecision, CompactionError>;

    /// Fired after the compaction entry is appended (notification).
    async fn post_compact(&self, ev: &PostCompactEvent);

    /// Fired before tree navigation; may cancel navigation or supply a custom branch summary.
    async fn before_tree(
        &self,
        ev: &BeforeTreeEvent,
        cancel: CancelToken,
    ) -> Result<BeforeTreeDecision, CompactionError>;

    /// Fired after navigation completes (notification).
    async fn post_tree(&self, ev: &PostTreeEvent);
}

/// A no-op hook implementation: always proceeds, observes nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHooks;

impl CompactionHooks for NoHooks {
    async fn before_compact(
        &self,
        _ev: &BeforeCompactEvent,
        _cancel: CancelToken,
    ) -> Result<BeforeCompactDecision, CompactionError> {
        Ok(BeforeCompactDecision::Proceed)
    }
    async fn post_compact(&self, _ev: &PostCompactEvent) {}
    async fn before_tree(
        &self,
        _ev: &BeforeTreeEvent,
        _cancel: CancelToken,
    ) -> Result<BeforeTreeDecision, CompactionError> {
        Ok(BeforeTreeDecision::proceed())
    }
    async fn post_tree(&self, _ev: &PostTreeEvent) {}
}
