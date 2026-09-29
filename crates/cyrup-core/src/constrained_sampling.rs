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
//! # The declaration is unconditional — pi v0.87.1
//!
//! At v0.83.0 no pi built-in declared the field: `git grep -n constrainedSampling v0.83.0 --
//! packages/coding-agent/src packages/agent/src` returned exactly three hits, the `ToolDefinition`
//! field declaration and the two `tool-definition-wrapper.ts` copies above. pi commit `7915cdac` —
//! *"feat(ai): add strict tool schema conversion"*, first tagged **v0.84.2** — added
//! `constrainedSampling: getExperimentalToolSampling()` to the coding built-ins, gated on
//! `PI_EXPERIMENTAL`. CHANGELOG 0.86.0 — *"Enabled strict-prefer JSON-schema sampling by default
//! for built-in read, bash, powershell, edit, and write tools, without requiring
//! PI_EXPERIMENTAL"* — dropped that gate and inlined the literal into each definition.
//!
//! So at **v0.87.1** the declaration is the inline literal
//! `constrainedSampling: { type: "json_schema", strict: "prefer" }` at `core/tools/read.ts:80`,
//! the shared `createShellToolDefinition` at `bash.ts:243` (so `powershell` inherits it from that
//! one line), `edit.ts:156` and `write.ts:57` — five tools, four declarations. Both
//! `getExperimentalToolSampling` and `server/create-harness.ts` are GONE from the repo at that tag
//! (`git grep -n getExperimentalToolSampling v0.87.1` → no hits; `git ls-tree v0.87.1
//! packages/coding-agent/src/server/` → empty; `core/experimental.ts` @v0.87.1 exports only
//! `areExperimentalFeaturesEnabled`), so do not carry a flag-gated description forward: re-derive
//! it at the tag you are reading.
//!
//! [`prefer_strict_tool_sampling`] below is that literal, and the four `Tool::constrained_sampling`
//! bodies in `cyrup-tools` — `tools/read.rs`, `tools/edit.rs`, `tools/write.rs` and the shared
//! `tools/bash.rs` `ShellTool` engine, hence `powershell` — return it unconditionally. The plumbing
//! this module also provides — an extension-registered or guest tool opting in and having the
//! declaration reach the wire — is unchanged.

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

/// Pi's literal `{ type: "json_schema", strict: "prefer" }` — the declaration the five coding
/// built-ins carry. Upstream held it as a shared `const PREFER_STRICT_TOOL_SAMPLING`
/// (`core/experimental.ts:1`) **@v0.85.1 only**; v0.86.0 deleted that const along with
/// `getExperimentalToolSampling` and inlined the object literal into each of the four definitions,
/// so `git grep -n PREFER_STRICT v0.87.1` returns nothing — upstream has no shared constant today.
///
/// [CYRUP-DELTA, mechanism only] cyrup holds ONE shared `static` where pi writes four inline object
/// literals, because [`crate::Tool::constrained_sampling`] hands out a reference and the value
/// cannot be constructed per call. The declaration that reaches the model is byte-identical either
/// way, so this is a mechanism difference at full feature parity.
static PREFER_STRICT_TOOL_SAMPLING: ConstrainedSampling =
    ConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema {
        strict: StrictSampling::Prefer,
    });

/// The strict-`prefer` declaration itself, by reference.
///
/// The `static` stays private so there is exactly one way to reach it: `cyrup-tools`' four built-in
/// bodies, and any other tool opting in, all hand out this same object.
pub fn prefer_strict_tool_sampling() -> &'static ConstrainedSampling {
    &PREFER_STRICT_TOOL_SAMPLING
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const SRC: &str = include_str!("constrained_sampling.rs");

    /// The module header, and nothing else in this file.
    fn header_block() -> &'static str {
        const OPENS: &str = "# Why these types live in";
        const CLOSES: &str = "/// Pi `Tool[\"constrainedSampling\"]`";
        let Some((_, after_open)) = SRC.split_once(OPENS) else {
            panic!("`{OPENS}` no longer opens the header — retarget this scan");
        };
        let Some((block, _)) = after_open.split_once(CLOSES) else {
            panic!("`{CLOSES}` no longer follows the header — retarget this scan");
        };
        block
    }

    /// TOOL-046 — a source scan over this module's header, the record of what pi declares.
    ///
    /// The header was written for **v0.84.2**, where the field was
    /// `constrainedSampling: getExperimentalToolSampling()` with per-tool line citations and
    /// `server/create-harness.ts`. pi v0.86.0 dropped the `PI_EXPERIMENTAL` gate and inlined the
    /// literal; by v0.87.1 every one of those citations is dead. Same shape, and for the same
    /// reason, as `crate::tool`'s
    /// `constrained_sampling_doc_tests::the_constrained_sampling_doc_cites_v0_87_1_and_not_the_dead_v0_84_2_lines`.
    ///
    /// Only the LINE citations are forbidden: the rewritten prose legitimately NAMES
    /// `getExperimentalToolSampling` and `server/create-harness.ts` in order to record that pi
    /// deleted them, and a blunt name scan cannot tell that from a live claim.
    #[test]
    fn the_module_header_cites_v0_87_1_and_not_the_dead_v0_84_2_lines() {
        let block = header_block();
        // Non-vacuity, both directions: prove the slice really is the header, so an over- or
        // under-read cannot pass the forbidden half by matching nothing.
        assert!(
            block.contains("cyrup-provider"),
            "the extracted slice is not the module header: {block:?}"
        );
        assert!(
            !block.contains("mod tests"),
            "the slice over-ran into this test module, so the assertions below would be vacuous"
        );

        for dead in [
            "read.ts:222",
            "bash.ts:354",
            "edit.ts:329",
            "write.ts:200",
            "create-harness.ts:34",
            "experimental.ts:7-9",
        ] {
            assert!(
                !block.contains(dead),
                "`{dead}` is a v0.84.2 fact that is dead at v0.87.1 — pi v0.86.0 dropped the \
                 `PI_EXPERIMENTAL` gate, and `getExperimentalToolSampling` and \
                 `server/create-harness.ts` no longer exist upstream. Re-derive the citation at \
                 the tag you read instead of carrying this one forward."
            );
        }

        for live in [
            "v0.87.1",
            "read.ts:80",
            "bash.ts:243",
            "edit.ts:156",
            "write.ts:57",
            // The whole substance of pi 0.86.0: no flag gates the declaration any more.
            "unconditional",
        ] {
            assert!(
                block.contains(live),
                "the header must cite `{live}` — the tag and lines where pi actually declares \
                 `constrainedSampling: {{ type: \"json_schema\", strict: \"prefer\" }}`"
            );
        }
    }

    /// TOOL-046 — the flag-gated ancestor is DELETED, not quarantined.
    ///
    /// `experimental_tool_sampling` / `experimental_tool_sampling_from` were pi
    /// `getExperimentalToolSampling`'s Rust counterpart, latching `CYRUP_EXPERIMENTAL` in a
    /// `OnceLock`. Upstream deleted that function at v0.86.0 and nothing in this workspace called
    /// cyrup's copy, so it was dead production API. A deletion's regression pin is a scan
    /// forbidding the symbol's return — the only artifact left to test.
    #[test]
    fn the_dead_flag_gated_api_is_gone() {
        // The production half only — this module's own test prose names the dead symbol in order
        // to forbid it, exactly as `crate::tool`'s doc scan has to.
        let Some((production, _)) = SRC.split_once("#[cfg(test)]") else {
            panic!("`#[cfg(test)]` no longer closes the production half — retarget this scan");
        };
        assert!(
            production.contains("fn prefer_strict_tool_sampling"),
            "non-vacuity: the slice must be this module's production source"
        );
        assert!(
            !production.contains("fn experimental_tool_sampling"),
            "pi deleted `getExperimentalToolSampling` at v0.86.0 and the declaration is \
             unconditional; the flag-gated accessor must not come back"
        );
    }

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
}
