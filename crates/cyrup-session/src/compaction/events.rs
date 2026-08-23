//! Compaction payload types (arch-05 §3.9). Plain serde structs: they are the shape the appended
//! compaction/branch-summary entries take when handed back to a caller, and the shape the
//! `session_before_compact` / `session_tree` extension events carry across the WASM boundary
//! (ADR-0002). Extension dispatch itself lives in `cyrup-ext`; `cyrup-session` does NOT depend on
//! it.

use cyrup_core::{EntryId, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Why compaction ran (R-05-019 `reason` field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

/// The appended compaction entry payload (R-05-009/021).
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
