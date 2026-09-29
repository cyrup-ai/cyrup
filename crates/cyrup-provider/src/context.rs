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

// PROV-083a — the model-facing tool DECLARATION types moved down into `cyrup-core`
// (`cyrup_core::tool_def`) and are re-exported here so every provider-facing path
// (`cyrup_provider::context::ToolDef`, `cyrup_provider::ToolDef`) is byte-identical to what it
// named before.
//
// They had to move because upstream a declaration travels INSIDE the transcript
// (`SystemMessage.toolsAdded: Tool[]` / `toolsRemoved: ToolReference[]`,
// `packages/ai/src/types.ts:502-506` @v0.87.1). The Rust analogue of that entry is
// `cyrup_core::Message`, and `cyrup-provider` DEPENDS ON `cyrup-core`, so `Message::System`
// could not carry a type defined at this level. Exact precedent below: `ConstrainedSampling`
// made the same move under PROV-011.
pub use cyrup_core::tool_def::{ToolDef, ToolReference};

/// The NORMALIZED request context every provider-facing function expects — re-exported from
/// [`crate::utils::transcript`], which owns it because its private field makes
/// [`normalize_context`](crate::utils::transcript::normalize_context) the only public constructor
/// (PROV-083a). Upstream declares it beside `Context` (`packages/ai/src/types.ts:631`), so
/// `cyrup_provider::context::TranscriptContext` names it here too.
pub use crate::utils::transcript::TranscriptContext;

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
