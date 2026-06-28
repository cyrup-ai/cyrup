//! Per-tool prompt snippets & guidelines (arch-06 §3.2, R-06-012/013).
//!
//! Tool contributions arrive as **data** so the pure builder never depends on `cyrup-tools` /
//! `cyrup-ext`. The caller (agent loop) gathers a `Vec<ToolPromptContribution>` from the
//! *currently active* tool set each turn; a dynamically registered tool simply appears (or
//! disappears) in that slice, which is detected by the inputs fingerprint (R-06-013).

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

/// Trait the runtime-facing `Tool` (arch-03) and dynamic ext tools (arch-08) impl so the active
/// set can be projected to contributions without the builder depending on those crates. Default =
/// no contribution.
pub trait PromptContributor {
    fn prompt_contribution(&self) -> Option<ToolPromptContribution> {
        None
    }
}
