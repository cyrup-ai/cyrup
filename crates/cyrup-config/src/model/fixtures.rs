//! Model values the `model/` test modules build their cases from. Moved verbatim out of the
//! single `mod tests` this module tree was split from.

use cyrup_provider::{ApiId, Modality, Model, ModelCost};

pub(super) fn model(provider: &str, id: &str, name: &str) -> Model {
    Model {
        id: id.into(),
        name: name.to_string(),
        api: ApiId::from("anthropic-messages"),
        provider: provider.into(),
        base_url: String::new(),
        reasoning: true,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 200_000,
        max_tokens: 8192,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

pub(super) fn oai(provider: &str, id: &str) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: ApiId::from("openai-completions"),
        provider: provider.into(),
        base_url: "https://builtin.example/v1".into(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}
