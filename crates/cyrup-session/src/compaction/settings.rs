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
    /// Headroom RESERVED out of the model context window; the newest-first message budget is
    /// `(model.contextWindow || 128000) − reserve_tokens`, not this value
    /// (`branch-summarization.ts:312-313` @v0.83.0, default `reserveTokens = 16384` at `:305`).
    /// Implemented by [`super::branch::branch_token_budget`]. An earlier doc here claimed this
    /// field WAS the budget; it is not, and a reader trusting that re-introduces the bug SESS-006
    /// closed.
    #[serde(default = "default_reserve")]
    pub reserve_tokens: u32,
    /// FRONT-END-ONLY: whether the TUI skips *asking* the user whether to summarize before a
    /// `/tree` navigation. Pi's sole consumer repo-wide is `interactive-mode.ts:4672`; the word
    /// `skipPrompt` does not appear in `agent-session.ts` at all, and `navigateTree`'s summarizer
    /// gate is the user's choice alone (`agent-session.ts:2983`). The core branch-summary path
    /// therefore does NOT consult this — see [`super::Compactor::run_branch_summary`].
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
