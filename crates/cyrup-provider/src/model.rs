//! Model + capability/cost metadata (arch-01 §4.2 / func-01 §4.2).

use cyrup_core::{ApiId, ModelId, ProviderId};

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
}

impl Model {
    pub fn supports_image_input(&self) -> bool {
        self.input.contains(&Modality::Image)
    }
}
