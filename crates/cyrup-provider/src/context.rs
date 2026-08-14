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

/// Pi `Tool["constrainedSampling"]` — `false | ConstrainedSamplingConfig` (`types.ts:484`
/// @v0.83.0). The `false` literal is kept as its own variant rather than collapsed into `None` so
/// a pi-authored tool definition round-trips byte-identically; upstream states it "behaves the same
/// as omitting the field" (`packages/ai/README.md:483`) and every resolver here treats it so.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ConstrainedSampling {
    Config(ConstrainedSamplingConfig),
    /// pi's `false`. `true` is not expressible upstream; it is accepted here and treated as
    /// `false`, because the resolvers key on `config.type` and neither bool has one.
    Disabled(bool),
}

impl ConstrainedSampling {
    /// The config, or `None` for pi's `false` — i.e. `!config || config.type !== …`'s first
    /// clause (`constrained-sampling.ts:85`, `:105` @v0.83.0).
    pub fn config(&self) -> Option<&ConstrainedSamplingConfig> {
        match self {
            ConstrainedSampling::Config(c) => Some(c),
            ConstrainedSampling::Disabled(_) => None,
        }
    }
}

/// Pi `ConstrainedSamplingConfig` — `types.ts:469-477` @v0.83.0.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstrainedSamplingConfig {
    JsonSchema { strict: StrictSampling },
    Grammar { variants: GrammarVariants },
}

/// Pi's `strict: "prefer" | "require"` (`types.ts:472` @v0.83.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StrictSampling {
    Prefer,
    Require,
}

/// Pi `GrammarVariants = Partial<Record<GrammarFormat, string>>` where
/// `GrammarFormat = "openai_lark" | "openai_regex"` (`types.ts:459-461` @v0.83.0). The keys are
/// snake_case upstream, so this struct deliberately carries no `rename_all`.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrammarVariants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_lark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_regex: Option<String>,
}
