//! Provider-side constrained-sampling *declaration* types (PROV-011 / EXT-024).
//!
//! These are the serializable shapes a tool uses to OPT IN to grammar- or strict-JSON-schema
//! constrained sampling. The resolvers that consume them live provider-side in
//! `cyrup-provider/src/utils/constrained_sampling.rs` (a port of pi
//! `packages/ai/src/api/constrained-sampling.ts` @v0.83.0).
//!
//! # Why these types live in `cyrup-core` and not `cyrup-provider`
//!
//! Upstream, the declaration travels `ToolDefinition.constrainedSampling`
//! (`packages/coding-agent/src/core/extensions/types.ts:463` @v0.83.0) →
//! `wrapToolDefinition` copies it onto the `AgentTool`
//! (`packages/coding-agent/src/core/tools/tool-definition-wrapper.ts:14`, and back at `:42` in
//! `createToolDefinitionFromAgentTool`) → the agent loop's `Context.tools` → `convertTools`. The
//! Rust analogue of `AgentTool` is [`crate::Tool`], which lives here; `cyrup-provider` depends on
//! `cyrup-core`, so the type has to be defined at this level for `Tool::constrained_sampling` to
//! exist at all. `cyrup-provider` re-exports every item in this module from its `context` module,
//! so the provider-facing paths are unchanged.
//!
//! # No built-in tool declares it — upstream or here
//!
//! `git grep -n constrainedSampling v0.83.0 -- packages/coding-agent/src packages/agent/src`
//! returns exactly three hits: the `ToolDefinition` field declaration and the two
//! `tool-definition-wrapper.ts` copies above. pi's Edit/Write/Read/Bash tools do **not** declare
//! it. The gap this module closes is therefore the *plumbing* — an extension-registered or guest
//! tool can opt in and have the declaration reach the wire — not a missing built-in opt-in.

/// Pi `Tool["constrainedSampling"]` — `false | ConstrainedSamplingConfig`
/// (`packages/ai/src/types.ts:484` @v0.83.0, and `extensions/types.ts:463` on the
/// `ToolDefinition` side). The `false` literal is kept as its own variant rather than collapsed
/// into `None` so a pi-authored tool definition round-trips byte-identically; upstream states it
/// "behaves the same as omitting the field" (`packages/ai/README.md:483`) and every resolver
/// treats it so.
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
    /// clause (`packages/ai/src/api/constrained-sampling.ts:85`, `:105` @v0.83.0).
    pub fn config(&self) -> Option<&ConstrainedSamplingConfig> {
        match self {
            ConstrainedSampling::Config(c) => Some(c),
            ConstrainedSampling::Disabled(_) => None,
        }
    }
}

/// Pi `ConstrainedSamplingConfig` — `packages/ai/src/types.ts:469-477` @v0.83.0.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstrainedSamplingConfig {
    JsonSchema { strict: StrictSampling },
    Grammar { variants: GrammarVariants },
}

/// Pi's `strict: "prefer" | "require"` (`packages/ai/src/types.ts:472` @v0.83.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StrictSampling {
    Prefer,
    Require,
}

/// Pi `GrammarVariants = Partial<Record<GrammarFormat, string>>` where
/// `GrammarFormat = "openai_lark" | "openai_regex"` (`packages/ai/src/types.ts:459-461`
/// @v0.83.0). The keys are snake_case upstream, so this struct deliberately carries no
/// `rename_all`.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrammarVariants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_lark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_regex: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// pi serializes `constrainedSampling: false` literally; the untagged `Disabled` arm must
    /// round-trip as the bare JSON `false`, not as an object.
    #[test]
    fn disabled_round_trips_as_the_bare_false_literal() {
        let v = serde_json::to_value(ConstrainedSampling::Disabled(false)).unwrap();
        assert_eq!(v, serde_json::json!(false));
        let back: ConstrainedSampling = serde_json::from_value(v).unwrap();
        assert_eq!(back, ConstrainedSampling::Disabled(false));
        assert!(back.config().is_none());
    }

    /// `{"type":"json_schema","strict":"require"}` — pi's discriminated union, snake_case tag.
    #[test]
    fn json_schema_config_uses_pis_snake_case_tag() {
        let v = serde_json::to_value(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema { strict: StrictSampling::Require },
        ))
        .unwrap();
        assert_eq!(v, serde_json::json!({"type": "json_schema", "strict": "require"}));
        let back: ConstrainedSampling = serde_json::from_value(v).unwrap();
        assert!(matches!(
            back.config(),
            Some(ConstrainedSamplingConfig::JsonSchema { strict: StrictSampling::Require })
        ));
    }

    /// `GrammarVariants` keys are snake_case upstream and absent keys are omitted.
    #[test]
    fn grammar_variant_keys_are_snake_case_and_absent_keys_are_omitted() {
        let v = serde_json::to_value(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: Some("start: /x/".into()),
                    openai_regex: None,
                },
            },
        ))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type": "grammar", "variants": {"openai_lark": "start: /x/"}})
        );
    }
}
