//! Per-tool prompt snippets & guidelines (arch-06 §3.2, R-06-012/013).
//!
//! Tool contributions arrive as **data** so the pure builder never depends on `cyrup-tools` /
//! `cyrup-ext`. The caller (agent loop) gathers a `Vec<ToolPromptContribution>` from the
//! *currently active* tool set each turn; a dynamically registered tool simply appears (or
//! disappears) in that slice, which is detected by the inputs fingerprint (R-06-013).
//!
//! There is deliberately **no contributor trait** in this module. Pi has no such indirection: it
//! projects `definition.promptSnippet` / `definition.promptGuidelines` straight off the tool
//! definition into two name-keyed maps (`core/agent-session.ts:2497-2512`) which
//! `_rebuildSystemPrompt` (`:1023-1056`) then reads, and pi's per-tool
//! `*ToolSystemPromptContribution` exports (e.g. `core/tools/read.ts:27-30`) are plain **data** —
//! which is exactly what [`ToolPromptContribution`] mirrors. cyrup's equivalent projection is
//! `cyrup_session_svc::builder::tool_contribution` (`crates/cyrup-session-svc/src/builder.rs:1935`),
//! reading `cyrup_core::Tool::prompt_snippet` / `prompt_guidelines`.
//!
//! An all-default `PromptContributor` trait used to sit here beside that path with zero
//! implementors and zero readers, so anything implementing it (the doc named `Tool` and ext tools
//! as the intended implementors) would have contributed nothing, silently. It was deleted rather
//! than left exported as a live-looking seam. Do not reintroduce it: add the metadata to
//! `cyrup_core::Tool` and let `tool_contribution` project it.

use std::sync::Arc;

/// What one tool contributes to the prompt.
///
/// R-06-012: a tool appears in the "Available tools" list only if it has a `snippet`.
/// Per func-03 R-03-039, each guideline string MUST name its tool so it stays meaningful if the
/// tool is later disabled.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPromptContribution {
    pub tool: Arc<str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guidelines: Vec<Arc<str>>,
}

impl ToolPromptContribution {
    /// Convenience constructor: a tool with a one-line snippet and no extra guidelines.
    pub fn snippet(tool: impl Into<Arc<str>>, snippet: impl Into<Arc<str>>) -> Self {
        Self { tool: tool.into(), snippet: Some(snippet.into()), guidelines: Vec::new() }
    }

    /// Add a tool-specific guideline bullet (chainable).
    #[must_use]
    pub fn with_guideline(mut self, g: impl Into<Arc<str>>) -> Self {
        self.guidelines.push(g.into());
        self
    }
}
