//! Extension hook payloads + the injected dispatcher trait (arch-05 §3.9, R-05-019..023). Payloads
//! are plain serde structs (they cross the WASM boundary as serialized events per ADR-0002); the
//! dispatcher is injected so `cyrup-session` does not depend on `cyrup-ext`.

use cyrup_core::{CancelToken, EntryId, Message};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
}

/// Input to the before-compact hook (R-05-019).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeCompactEvent {
    pub messages_to_summarize: Vec<Message>,
    pub turn_prefix_messages: Vec<Message>,
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
    },
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

/// The before-tree hook's decision (R-05-022).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "decision")]
pub enum BeforeTreeDecision {
    Proceed,
    Cancel,
    CustomSummary {
        summary: String,
        #[serde(default)]
        details: Option<Value>,
    },
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
        Ok(BeforeTreeDecision::Proceed)
    }
    async fn post_tree(&self, _ev: &PostTreeEvent) {}
}
