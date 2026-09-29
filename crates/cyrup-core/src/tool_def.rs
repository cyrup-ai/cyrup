//! The model-facing tool DECLARATION, [`ToolDef`], and the name-only [`ToolReference`]
//! (PROV-083a).
//!
//! # Why these types live in `cyrup-core` and not `cyrup-provider`
//!
//! Upstream a tool declaration travels *inside the transcript*: `SystemMessage.toolsAdded: Tool[]`
//! and `SystemMessage.toolsRemoved: ToolReference[]`
//! (`packages/ai/src/types.ts:502-506` @v0.87.1). The Rust analogue of that transcript entry is
//! [`crate::Message`], which lives here, and `cyrup-provider` DEPENDS ON `cyrup-core`, so a type
//! defined provider-side could never appear on a `Message` variant.
//!
//! `ToolDef` therefore moved down out of `cyrup-provider`'s `context` module, which re-exports it,
//! so `cyrup_provider::ToolDef` and `cyrup_provider::context::ToolDef` resolve to this exact type
//! for every caller. That is the same relocation-behind-a-re-export PROV-011 performed on the
//! constrained-sampling declaration types, for the same reason and in the same file; see
//! [`crate::constrained_sampling`].

use crate::constrained_sampling::ConstrainedSampling;

/// The model-facing tool definition (func-01 §4.6). Distinct from the runtime [`crate::Tool`]
/// trait: this is the serializable schema the model sees, not an executable.
///
/// Pi `Tool` — `packages/ai/src/types.ts:600-605` @**v0.87.1** (`:479-485` @v0.83.0).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON-Schema-compatible (func-01 §10).
    pub parameters: serde_json::Value,
    /// Pi `Tool.constrainedSampling` (`types.ts:604` @v0.87.1) — opt-in provider-side constrained
    /// sampling. `None` (field absent) and `ConstrainedSampling::Disabled` (pi's `false`) behave
    /// identically; see `cyrup_provider::utils::constrained_sampling` (PROV-011).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<ConstrainedSampling>,
}

/// Pi `ToolReference` — `packages/ai/src/types.ts:607-609` @v0.87.1. A tool identified by NAME
/// only, which is all a removal needs: `SystemMessage.toolsRemoved` says a tool stops being
/// available, and Anthropic's `tool_removal` block carries nothing but the name.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReference {
    pub name: String,
}

impl ToolReference {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl From<&ToolDef> for ToolReference {
    fn from(tool: &ToolDef) -> Self {
        Self {
            name: tool.name.clone(),
        }
    }
}
