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
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON-Schema-compatible (func-01 §10).
    pub parameters: serde_json::Value,
}
