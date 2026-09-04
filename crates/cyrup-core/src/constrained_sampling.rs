//! Provider-side constrained-sampling *declaration* types (PROV-011 / EXT-024).
//!
//! These are the serializable shapes a tool uses to OPT IN to grammar- or strict-JSON-schema
//! constrained sampling. The resolvers that consume them live provider-side in
//! `cyrup-provider/src/utils/constrained_sampling.rs` (a port of pi
//! `packages/ai/src/api/constrained-sampling.ts` @v0.84.2).
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
//! # The four coding built-ins declare it — as of pi v0.84.2
//!
//! At v0.83.0 no pi built-in declared the field: `git grep -n constrainedSampling v0.83.0 --
//! packages/coding-agent/src packages/agent/src` returned exactly three hits, the `ToolDefinition`
//! field declaration and the two `tool-definition-wrapper.ts` copies above. That changed with pi
//! commit `7915cdac` — *"feat(ai): add strict tool schema conversion"*, first tagged **v0.84.2** —
//! which added `constrainedSampling: getExperimentalToolSampling()` to `read`
//! (`core/tools/read.ts:222`), the shared shell definition (`bash.ts:354`, so `powershell`
//! inherits it), `edit.ts:329` and `write.ts:200`, plus `server/create-harness.ts:34`.
//!
//! [`experimental_tool_sampling`] below is `getExperimentalToolSampling`'s Rust counterpart, and
//! `cyrup-tools` returns it from `Tool::constrained_sampling` on the same four tools. The
//! plumbing this module also provides — an extension-registered or guest tool opting in and having
//! the declaration reach the wire — is unchanged.

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

/// Pi `PREFER_STRICT_TOOL_SAMPLING` (`core/experimental.ts:1`) — the single value every coding
/// built-in declares. A `static` because [`crate::Tool::constrained_sampling`] hands out a
/// reference, so the value cannot be constructed per call.
static PREFER_STRICT_TOOL_SAMPLING: ConstrainedSampling =
    ConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema {
        strict: StrictSampling::Prefer,
    });

/// [`experimental_tool_sampling`] against an injected environment, so the `||` precedence is
/// exercisable without touching process state. Same shape as
/// `cyrup_tui::status::experimental_features_enabled_from`.
pub fn experimental_tool_sampling_from(
    get: impl Fn(&str) -> Option<String>,
) -> Option<&'static ConstrainedSampling> {
    let enabled = get("CYRUP_EXPERIMENTAL").as_deref() == Some("1")
        || get("PI_EXPERIMENTAL").as_deref() == Some("1");
    enabled.then_some(&PREFER_STRICT_TOOL_SAMPLING)
}

/// Pi `getExperimentalToolSampling` (`core/experimental.ts:7-9`): the strict-`prefer` JSON-schema
/// declaration when the experimental flag is on, and nothing otherwise.
///
/// `CYRUP_EXPERIMENTAL` is the renamed primary and `PI_EXPERIMENTAL` survives as the
/// lower-precedence fallback — the same pair, in the same order, as
/// `cyrup::startup::are_experimental_features_enabled` (`startup.rs:76-84`) and
/// `cyrup_tui::status::experimental_features_enabled` (`status.rs:474-483`). Upstream re-reads
/// `process.env` on every call but only ever calls it while BUILDING a tool definition; the env is
/// read once here and latched, because cyrup likewise builds its tool set once
/// (`ToolRegistry::with_builtins`). A caller that mutates the process env and then rebuilds the
/// registry would observe the latch as stale; [`experimental_tool_sampling_from`] is the escape
/// hatch.
pub fn experimental_tool_sampling() -> Option<&'static ConstrainedSampling> {
    static RESOLVED: std::sync::OnceLock<Option<&'static ConstrainedSampling>> =
        std::sync::OnceLock::new();
    *RESOLVED.get_or_init(|| experimental_tool_sampling_from(|k| std::env::var(k).ok()))
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
            ConstrainedSamplingConfig::JsonSchema {
                strict: StrictSampling::Require,
            },
        ))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type": "json_schema", "strict": "require"})
        );
        let back: ConstrainedSampling = serde_json::from_value(v).unwrap();
        assert!(matches!(
            back.config(),
            Some(ConstrainedSamplingConfig::JsonSchema {
                strict: StrictSampling::Require
            })
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
    /// DoD 2 — either flag at the literal `"1"` yields pi's `PREFER_STRICT_TOOL_SAMPLING`; the
    /// `CYRUP_*` primary and the `PI_*` fallback are independent, and nothing else turns it on.
    #[test]
    fn experimental_tool_sampling_reads_both_flags_and_nothing_else() {
        let prefer = ConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema {
            strict: StrictSampling::Prefer,
        });
        for key in ["CYRUP_EXPERIMENTAL", "PI_EXPERIMENTAL"] {
            let got = experimental_tool_sampling_from(|k| (k == key).then(|| "1".to_string()));
            assert_eq!(got, Some(&prefer), "{key}=1 must enable it");
        }
        assert_eq!(experimental_tool_sampling_from(|_| None), None);
        for value in ["", "0", "true", "yes"] {
            assert_eq!(
                experimental_tool_sampling_from(|_| Some(value.to_string())),
                None,
                "only the literal \"1\" enables it, not {value:?}"
            );
        }
    }

    /// The declaration serializes as pi's literal `{ type: "json_schema", strict: "prefer" }`
    /// (`core/experimental.ts:1`).
    #[test]
    fn the_experimental_declaration_is_pis_prefer_literal() {
        let got = experimental_tool_sampling_from(|_| Some("1".to_string())).unwrap();
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            serde_json::json!({ "type": "json_schema", "strict": "prefer" })
        );
    }
}
