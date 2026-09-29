//! SUBA-119 — the native-child model-verification check: does the model the child REPORTED match the
//! launch candidate the parent asked for?
//!
//! A clause-for-clause port of `formatSubagentModelVerificationError`
//! (`src/runs/shared/model-resolution.ts:14-34` @pi-subagents v0.71.0). The check runs on every
//! assistant `message_end` that carries a `model`, for a launch that named a candidate of its own,
//! and its failure is a hard run error: a child that silently ran a different model than the parent
//! selected has produced work the parent cannot account for.
//!
//! The error text, including its remediation paragraph, is upstream's, because it is what the
//! operator reads and what tells them how to declare a legitimate alias. The ONE substitution is
//! the config path it names: `~/.cyrup/agent/subagents/config.json`, the file this port really reads
//! `modelResponseAliases` from, in place of upstream's `~/.pi/agent/extensions/subagent/config.json`
//! — see the CYRUP-DELTA at the `return` itself.

use std::collections::BTreeMap;

use crate::exec::spawn_plan::split_known_thinking_suffix;
use crate::extension::models::AvailableModelEntry;

/// SUBA-119 — pi's `Record<string, string[]>` alias map (`model-resolution.ts:18`): keys are launch
/// candidates' BASE `provider/model` ids, values are the raw response ids that legitimately
/// identify them.
///
/// `BTreeMap` rather than `HashMap` so the serialized `config.json` / recovery-descriptor form has
/// a stable key order — a descriptor is compared and digested, so its bytes must not depend on hash
/// seeding.
pub type ModelResponseAliases = BTreeMap<String, Vec<String>>;

/// `validateModelResponseAliases(value, label)` — a full port of
/// `src/shared/model-response-aliases.ts` @v0.71.0, applied to the RAW JSON for the reason every
/// other raw-config validator in this crate is: serde alone would accept a wrong-shaped value by
/// dropping it, and a discarded alias map means the run the operator tried to unblock keeps failing
/// with no explanation.
///
/// Upstream's three refusals, verbatim, with `label` defaulted by the caller:
///
/// * a non-object (including an array and `null`): `<label> must be a JSON object`;
/// * a key that is not a non-empty `provider/model` pair — `indexOf("/") <= 0`, or either side
///   blank after trimming: `<label> key "<key>" must be a non-empty provider/model ID`;
/// * a value that is not an array of non-empty strings:
///   `<label>["<key>"] must be an array of non-empty response ID strings`.
///
/// Upstream's `JSON.stringify(candidate)` around the key is reproduced with
/// [`serde_json::Value::to_string`](ToString::to_string) on a string value, so a key containing a
/// quote or a backslash renders the same escaped form on both sides.
///
/// An absent value (`None`, upstream's `undefined`) is valid: the setting is optional.
///
/// # Errors
///
/// Upstream's own message for the first problem found.
///
/// CYRUP-DELTA (mechanism, full parity) — with TWO malformed keys the two sides may name different
/// ones: upstream walks `Object.entries` (JSON insertion order), this walks
/// [`serde_json::Map`]'s own order. Either way the file is refused, with the same message shape, for
/// the same reason; which of two equally-broken keys is quoted is not a behaviour the setting has.
pub fn validate_model_response_aliases(
    value: Option<&serde_json::Value>,
    label: &str,
) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    // `if (!value || typeof value !== "object" || Array.isArray(value))`. JSON `null` is falsy
    // upstream, so it takes this branch rather than being treated as absent.
    let Some(object) = value.as_object().filter(|_| !value.is_array()) else {
        return Err(format!("{label} must be a JSON object"));
    };
    for (candidate, aliases) in object {
        // `const slash = candidate.indexOf("/"); if (slash <= 0 || !candidate.slice(0,
        // slash).trim() || !candidate.slice(slash + 1).trim())`. `slash <= 0` rejects both "no
        // slash" (-1) and a leading slash (0) in one clause.
        let provider_and_model = candidate
            .split_once('/')
            .filter(|(provider, model)| !provider.trim().is_empty() && !model.trim().is_empty());
        if provider_and_model.is_none() {
            return Err(format!(
                "{label} key {} must be a non-empty provider/model ID",
                serde_json::Value::String(candidate.clone())
            ));
        }
        let ok = aliases.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| item.as_str().is_some_and(|s| !s.trim().is_empty()))
        });
        if !ok {
            return Err(format!(
                "{label}[{}] must be an array of non-empty response ID strings",
                serde_json::Value::String(candidate.clone())
            ));
        }
    }
    Ok(())
}

/// `formatSubagentModelVerificationError(expectedModel, observedModel, availableModels,
/// modelResponseAliases)` (`model-resolution.ts:14-34` @v0.71.0), clause for clause and in upstream's
/// order:
///
/// 1. `if (!availableModels || availableModels.length === 0) return undefined;` (`:20`) — with no
///    registry there is nothing to verify against, so the check is OFF rather than failing closed.
/// 2. `expectedBase` is `expected` with its thinking suffix stripped (`:21`).
/// 3. An alias declared under `expectedBase` whose list `.includes(observedModel)` passes (`:22-23`).
///    The alias is matched against the RAW observed id, NOT its base — upstream's doc comment at
///    `:12` says exactly this: *"Aliases apply only to the resolved launch candidate (without its
///    thinking suffix) and the exact raw response ID."* So the alias map's KEYS are base ids and its
///    VALUES are raw response ids.
/// 4. `expectedBase === observedBase` passes (`:24-25`).
/// 5. ONLY if the registry holds `expectedBase` as a `fullId` (`:26`): `entry.id === observedBase`
///    passes (`:28`), and so does either ID-LEAF — `id` or `fullId` after the last `/` (`:29-31`).
///    When the registry does NOT know `expectedBase`, none of these three apply.
/// 6. Otherwise the verification error (`:33`).
///
/// CYRUP-DELTA: `available` comes from [`crate::extension::models::registry_available_models`], which
/// is the credential-BLIND catalog (pi's `getModels()`) rather than pi's credential-filtered
/// `getAvailable()` — the delta already recorded on that accessor. Here the direction is benign and
/// worth stating: a wider registry can only make clause 5 match MORE often, so this check is strictly
/// more permissive than pi's and cannot fail a run pi would pass.
#[must_use]
pub(crate) fn format_subagent_model_verification_error(
    expected_model: &str,
    observed_model: &str,
    available: &[AvailableModelEntry],
    aliases: Option<&BTreeMap<String, Vec<String>>>,
) -> Option<String> {
    // `:20` — an empty or absent registry turns the check off entirely.
    if available.is_empty() {
        return None;
    }
    let (expected_base, _) = split_known_thinking_suffix(expected_model);
    // `:22-23` — `Object.hasOwn(modelResponseAliases, expectedBase) && …includes(observedModel)`.
    // Matched against the RAW observed id (`:12`), not its base.
    if let Some(map) = aliases
        && let Some(declared) = map.get(expected_base)
        && declared.iter().any(|a| a == observed_model)
    {
        return None;
    }
    let (observed_base, _) = split_known_thinking_suffix(observed_model);
    // `:24-25`.
    if expected_base == observed_base {
        return None;
    }
    // `:26` — `availableModels.find((entry) => entry.fullId === expectedBase)`. Every clause below is
    // INSIDE this guard: an unknown expected id gets none of them.
    if let Some(entry) = available.iter().find(|e| e.full_id() == expected_base) {
        // `:28`.
        if entry.id() == observed_base {
            return None;
        }
        // `:29-31` — `slice(lastIndexOf("/") + 1)`, which for a string with no `/` is the whole
        // string (JS `lastIndexOf` returns -1, so the slice starts at 0). `rsplit_once` returning
        // `None` and falling back to the whole string is the same thing.
        let id_leaf = entry.id().rsplit_once('/').map_or(entry.id(), |(_, l)| l);
        let full_id_leaf = entry
            .full_id()
            .rsplit_once('/')
            .map_or(entry.full_id(), |(_, l)| l);
        if id_leaf == observed_base || full_id_leaf == observed_base {
            return None;
        }
    }
    // `:33` — upstream's sentence structure and every clause of its remediation paragraph, with
    // ONE substitution: the config path.
    //
    // CYRUP-DELTA (mechanism, full parity) — upstream names
    // `~/.pi/agent/extensions/subagent/config.json`; this port names
    // `~/.cyrup/agent/subagents/config.json`, which is where
    // `cyrup::subagent_config::load_subagent_extension_config` actually reads
    // `modelResponseAliases` from (`<cyrup_agent_dir>/subagents/config.json`). The paragraph exists
    // to tell the operator which file to edit to unblock the run; naming pi's path would make it
    // false instruction, and a run error whose remedy does not work is worse than no remedy. The
    // docs anchor is left as upstream's because it names the setting, not a path.
    Some(format!(
        "model_verification_failed: native Pi child reported a different model than the launch \
         candidate. Expected '{expected_model}' but observed '{observed_model}'. If you have \
         independently verified this response ID identifies the requested model, declare the exact \
         mapping in modelResponseAliases in \
         ~/.cyrup/agent/subagents/config.json (see \
         docs/configuration.md#modelresponsealiases). Use the resolved provider/model ID without \
         its thinking suffix as the key. This leaves the outgoing request unchanged. Configuration \
         changes affect new independent native runs; resumed native runs retain their launch-time \
         declaration. External CLI adapters do not use this setting."
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn registry() -> Vec<AvailableModelEntry> {
        vec![
            AvailableModelEntry::new("prov", "model"),
            AvailableModelEntry::new("other", "model"),
            AvailableModelEntry::new("vendor", "family/leaf"),
        ]
    }

    /// Upstream's text (`model-resolution.ts:33` @v0.71.0) with the two placeholders filled and the
    /// config path pointed at cyrup's own file. Asserted in FULL rather than by substring, so a
    /// paraphrase — of the message or of its remediation paragraph, which is the only thing telling
    /// an operator how to declare a legitimate alias — fails this test, and so does a path the
    /// operator cannot act on.
    const EXPECTED_ERROR: &str = "model_verification_failed: native Pi child reported a different model than the launch candidate. Expected 'prov/model' but observed 'other/model'. If you have independently verified this response ID identifies the requested model, declare the exact mapping in modelResponseAliases in ~/.cyrup/agent/subagents/config.json (see docs/configuration.md#modelresponsealiases). Use the resolved provider/model ID without its thinking suffix as the key. This leaves the outgoing request unchanged. Configuration changes affect new independent native runs; resumed native runs retain their launch-time declaration. External CLI adapters do not use this setting.";

    /// `:20` — with no registry there is nothing to verify against, so the check is OFF. It does NOT
    /// fail closed: a session whose registry has not loaded must not start failing every native run.
    #[test]
    fn an_empty_registry_turns_the_check_off() {
        assert_eq!(
            format_subagent_model_verification_error("prov/model", "other/model", &[], None),
            None
        );
    }

    /// `:33` — the mismatch error, byte for byte.
    #[test]
    fn a_genuine_mismatch_produces_upstreams_exact_text() {
        let err = format_subagent_model_verification_error(
            "prov/model",
            "other/model",
            &registry(),
            None,
        )
        .expect("a mismatch against a populated registry is an error");
        assert_eq!(err, EXPECTED_ERROR);
    }

    /// `:21,24-25` — the thinking suffix is stripped off the EXPECTED id before comparison, so a
    /// launch of `prov/model:high` answered by a bare `prov/model` is not a mismatch.
    #[test]
    fn a_thinking_suffix_on_the_expected_id_is_stripped_before_comparison() {
        assert_eq!(
            format_subagent_model_verification_error(
                "prov/model:high",
                "prov/model",
                &registry(),
                None
            ),
            None
        );
        // …and symmetrically on the observed id (`:24`).
        assert_eq!(
            format_subagent_model_verification_error(
                "prov/model",
                "prov/model:low",
                &registry(),
                None
            ),
            None
        );
    }

    /// `:28` — the registry entry's bare `id` matching the observed base passes, but ONLY because the
    /// registry holds `expectedBase` as a `fullId` (`:26`).
    #[test]
    fn a_registry_entrys_bare_id_matches_the_observed_base() {
        assert_eq!(
            format_subagent_model_verification_error("prov/model", "model", &registry(), None),
            None
        );
    }

    /// `:29-31` — either ID-LEAF, `id` or `fullId` after the last `/`.
    #[test]
    fn a_registry_entrys_id_leaf_matches_the_observed_base() {
        // `vendor/family/leaf` is the `fullId`; its `id` is `family/leaf`, whose leaf is `leaf`.
        assert_eq!(
            format_subagent_model_verification_error(
                "vendor/family/leaf",
                "leaf",
                &registry(),
                None
            ),
            None
        );
    }

    /// `:26` is a GUARD, not a convenience: when the registry does not know `expectedBase`, none of
    /// the id/leaf clauses apply and a mismatch stays a mismatch even though the observed id happens
    /// to be a leaf of it.
    #[test]
    fn the_leaf_clauses_do_not_apply_to_an_expected_id_the_registry_does_not_know() {
        assert!(
            format_subagent_model_verification_error("unknown/thing", "thing", &registry(), None)
                .is_some(),
            "an expected id absent from the registry gets no leaf leniency"
        );
    }

    /// `:22-23` — a declared alias under the expected BASE id, listing the RAW observed id, passes.
    #[test]
    fn a_declared_response_alias_accepts_the_observed_id() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "prov/model".to_string(),
            vec!["other/model".to_string(), "third/model".to_string()],
        );
        assert_eq!(
            format_subagent_model_verification_error(
                "prov/model",
                "other/model",
                &registry(),
                Some(&aliases)
            ),
            None
        );
        // An observed id the alias list does NOT name still fails.
        assert!(
            format_subagent_model_verification_error(
                "prov/model",
                "fourth/model",
                &registry(),
                Some(&aliases)
            )
            .is_some()
        );
    }

    /// The alias map's KEYS are BASE ids (`:21-22` computes `expectedBase` before the lookup, and the
    /// doc comment at `:12` and the error text both say "without its thinking suffix"), so a map keyed
    /// on the SUFFIXED id never matches.
    #[test]
    fn an_alias_keyed_on_the_suffixed_id_does_not_apply() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "prov/model:high".to_string(),
            vec!["other/model".to_string()],
        );
        assert!(
            format_subagent_model_verification_error(
                "prov/model:high",
                "other/model",
                &registry(),
                Some(&aliases)
            )
            .is_some(),
            "the key must be the base id"
        );
        // Keyed on the base id, the same launch passes.
        let mut base_keyed = BTreeMap::new();
        base_keyed.insert("prov/model".to_string(), vec!["other/model".to_string()]);
        assert_eq!(
            format_subagent_model_verification_error(
                "prov/model:high",
                "other/model",
                &registry(),
                Some(&base_keyed)
            ),
            None
        );
    }

    /// The alias list holds RAW response ids, not bases (`:12`, and `:23` compares against
    /// `observedModel` BEFORE `observedBase` is computed at `:24`).
    #[test]
    fn the_alias_list_is_matched_against_the_raw_observed_id() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "prov/model".to_string(),
            vec!["other/model:high".to_string()],
        );
        assert_eq!(
            format_subagent_model_verification_error(
                "prov/model",
                "other/model:high",
                &registry(),
                Some(&aliases)
            ),
            None
        );
        // The BASE of that raw id is not what the list declares, so it is not accepted by the alias
        // clause. (It is still a mismatch, since nothing else matches it either.)
        assert!(
            format_subagent_model_verification_error(
                "prov/model",
                "other/model",
                &registry(),
                Some(&aliases)
            )
            .is_some()
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod validator_tests {
    use super::*;

    /// SUBA-119 — every refusal `validateModelResponseAliases` makes
    /// (`src/shared/model-response-aliases.ts` @v0.71.0), message for message. Upstream runs this
    /// at `extension/config.ts:176` on every config read and at `async-resume.ts:347` on every
    /// recovery descriptor, so a malformed alias map is a hard refusal on both paths rather than a
    /// silently-dropped key.
    #[test]
    fn upstreams_three_refusals_are_reproduced_verbatim() {
        // An absent value is valid — the setting is optional (`if (value === undefined) return`).
        assert_eq!(
            validate_model_response_aliases(None, "config.modelResponseAliases"),
            Ok(())
        );

        // `!value || typeof value !== "object" || Array.isArray(value)`. JSON `null` is FALSY
        // upstream, so it refuses rather than being treated as absent.
        for bad in [
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!(["prov/model"]),
            serde_json::json!("prov/model"),
            serde_json::json!(7),
            serde_json::json!(true),
        ] {
            assert_eq!(
                validate_model_response_aliases(Some(&bad), "config.modelResponseAliases"),
                Err("config.modelResponseAliases must be a JSON object".to_string()),
                "{bad}"
            );
        }

        // `slash <= 0 || !candidate.slice(0, slash).trim() || !candidate.slice(slash + 1).trim()`.
        for key in ["nomodel", "", "/model", "prov/", "  /model", "prov/   "] {
            let bad = serde_json::json!({ key: ["other/model"] });
            assert_eq!(
                validate_model_response_aliases(Some(&bad), "config.modelResponseAliases"),
                Err(format!(
                    "config.modelResponseAliases key {} must be a non-empty provider/model ID",
                    serde_json::Value::String(key.to_string())
                )),
                "{key:?}"
            );
        }

        // `!Array.isArray(aliases) || aliases.some(alias => typeof alias !== "string" ||
        // !alias.trim())`.
        for value in [
            serde_json::json!("other/model"),
            serde_json::json!(null),
            serde_json::json!({}),
            serde_json::json!([""]),
            serde_json::json!(["   "]),
            serde_json::json!(["other/model", 7]),
            serde_json::json!(["other/model", null]),
        ] {
            let bad = serde_json::json!({ "prov/model": value.clone() });
            assert_eq!(
                validate_model_response_aliases(Some(&bad), "config.modelResponseAliases"),
                Err(
                    "config.modelResponseAliases[\"prov/model\"] must be an array of non-empty \
                     response ID strings"
                        .to_string()
                ),
                "{value}"
            );
        }

        // Well-formed values pass, including an empty map and an empty alias list (upstream's
        // `.some` is false for an empty array).
        for good in [
            serde_json::json!({}),
            serde_json::json!({ "prov/model": [] }),
            serde_json::json!({ "prov/model": ["other/model", "third/model:high"] }),
            serde_json::json!({ "vendor/family/leaf": ["leaf-2025"] }),
        ] {
            assert_eq!(
                validate_model_response_aliases(Some(&good), "config.modelResponseAliases"),
                Ok(()),
                "{good}"
            );
        }
    }

    /// The `label` argument is upstream's second parameter, and `async-resume.ts:347` passes a
    /// descriptor-specific one (`async recovery descriptor '<path>' modelResponseAliases`) so the
    /// operator learns WHICH file is malformed. A hardcoded label would name the wrong file on the
    /// resume path.
    #[test]
    fn the_label_names_the_file_that_is_malformed() {
        let err = validate_model_response_aliases(
            Some(&serde_json::json!("nope")),
            "async recovery descriptor '/runs/r1/recovery-descriptor.json' modelResponseAliases",
        )
        .expect_err("a string is not a JSON object");
        assert_eq!(
            err,
            "async recovery descriptor '/runs/r1/recovery-descriptor.json' modelResponseAliases \
             must be a JSON object"
        );
    }

    /// `JSON.stringify(candidate)` — a key carrying a quote or a backslash must render the same
    /// escaped form on both sides, or the two messages are not the same message.
    #[test]
    fn a_key_with_quotes_is_json_escaped_like_upstream() {
        let bad = serde_json::json!({ "no\"slash\\here": ["x"] });
        assert_eq!(
            validate_model_response_aliases(Some(&bad), "config.modelResponseAliases"),
            Err(
                r#"config.modelResponseAliases key "no\"slash\\here" must be a non-empty provider/model ID"#
                    .to_string()
            )
        );
    }
}
