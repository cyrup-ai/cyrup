//! Compaction & branch-summary settings (arch-05 §3.1, R-05-004). Resolved global+project by
//! `cyrup-config` (arch-07); compaction receives the resolved values.

use serde::{Deserialize, Serialize};

/// Triggers/budgets for context compaction.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    /// Whether automatic compaction is enabled (default true).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Headroom kept free for the model's next response (default 16384).
    #[serde(default = "default_reserve")]
    pub reserve_tokens: u32,
    /// Budget of recent messages preserved verbatim (default 20000).
    #[serde(default = "default_keep_recent")]
    pub keep_recent_tokens: u32,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            reserve_tokens: default_reserve(),
            keep_recent_tokens: default_keep_recent(),
        }
    }
}

/// Settings for branch summarization on `/tree` navigation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummarySettings {
    /// Message budget (newest-first) for the branch summary (default 16384). Per the corrected
    /// R-05-016 this IS the message budget, not `contextWindow − reserve`.
    #[serde(default = "default_reserve")]
    pub reserve_tokens: u32,
    /// Skip summarization when the user did not ask for it (default false, R-05-018).
    #[serde(default)]
    pub skip_prompt: bool,
}

impl Default for BranchSummarySettings {
    fn default() -> Self {
        Self { reserve_tokens: default_reserve(), skip_prompt: false }
    }
}

fn default_true() -> bool {
    true
}
fn default_reserve() -> u32 {
    16384
}
fn default_keep_recent() -> u32 {
    20000
}
