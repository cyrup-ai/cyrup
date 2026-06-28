//! Model + capability/cost metadata (arch-01 §4.2 / func-01 §4.2).

use crate::api::compat::OpenAiCompletionsCompat;
use cyrup_core::{ApiId, ModelId, ProviderId};

/// Maps pi thinking levels (`off`/`minimal`/`low`/`medium`/`high`/`xhigh`) to provider/model
/// specific reasoning values. Mirrors Pi's `ThinkingLevelMap = Partial<Record<ModelThinkingLevel,
/// string | null>>`: a missing key uses the provider default, a `null` value marks the level
/// unsupported, and a string overrides the wire value sent for that level.
pub type ThinkingLevelMap = std::collections::BTreeMap<String, Option<String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    /// USD per 1e6 tokens.
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: ModelId,
    pub name: String,
    pub api: ApiId,
    pub provider: ProviderId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub base_url: Option<String>,
    pub reasoning: bool,
    pub input: Vec<Modality>,
    #[serde(default)]
    pub output: Vec<Modality>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    /// Per-level reasoning value overrides (Pi `Model.thinkingLevelMap`). Additive, defaulted.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    /// OpenAI-completions compatibility overrides (Pi `Model.compat`). When unset, the wire impl
    /// auto-detects from `provider` + `base_url`. Additive, defaulted.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compat: Option<OpenAiCompletionsCompat>,
}

impl Model {
    pub fn supports_image_input(&self) -> bool {
        self.input.contains(&Modality::Image)
    }
}
