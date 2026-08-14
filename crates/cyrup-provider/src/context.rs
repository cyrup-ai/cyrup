//! The request `Context` and LLM tool definition (arch-01 §4.3 / func-01 §4).

use cyrup_core::Message;

/// Input to a single model call (func-01 §4.1).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
}

/// The model-facing tool definition (func-01 §4.6). Distinct from the runtime `cyrup_core::Tool`
/// trait: this is the serializable schema the model sees, not an executable.
///
/// Pi `Tool` — `types.ts:479-485` @**v0.83.0**.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON-Schema-compatible (func-01 §10).
    pub parameters: serde_json::Value,
    /// Pi `Tool.constrainedSampling` (`types.ts:484` @v0.83.0) — opt-in provider-side constrained
    /// sampling. `None` (field absent) and [`ConstrainedSampling::Disabled`] (pi's `false`) behave
    /// identically; see [`crate::utils::constrained_sampling`] (PROV-011).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<ConstrainedSampling>,
}

// PROV-011 — the constrained-sampling DECLARATION types moved down into `cyrup-core`
// (`cyrup_core::constrained_sampling`) and are re-exported here so every provider-facing path
// (`cyrup_provider::context::ConstrainedSampling`, `cyrup_provider::ConstrainedSampling`) is
// unchanged.
//
// They had to move because upstream the declaration is copied off the tool onto the runtime
// `AgentTool` (`tool-definition-wrapper.ts:14` @v0.83.0) and read back out of `Context.tools`;
// the Rust `AgentTool` analogue is `cyrup_core::Tool`, and `cyrup-provider` DEPENDS ON
// `cyrup-core`, so a type defined here could never appear on that trait. With the definition in
// core, `Tool::constrained_sampling()` exists and a tool can finally opt in.
pub use cyrup_core::constrained_sampling::{
    ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants, StrictSampling,
};
