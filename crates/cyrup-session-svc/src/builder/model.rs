//! Model resolution (Pi `findInitialModel`/`resolveCliModel`) plus the thinking-level persisted-key
//! codec — the `--model`/resume/settings/catalog precedence, with no dependence on the builder.

use cyrup_config::{ModelResolver, SettingsManager};
use cyrup_core::{ModelRef, ModelThinkingLevel};
use cyrup_provider::{Model, Provider};

use crate::error::SessionServiceError;

use super::SessionConfig;

/// Serialize a [`ModelThinkingLevel`] to its persisted snake/camel key (`off`/`minimal`/…/`xhigh`/`max`).
pub(crate) fn thinking_level_to_str(level: ModelThinkingLevel) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "off".to_string())
}

/// Parse a persisted thinking-level key back into a [`ModelThinkingLevel`] (inverse of
/// [`thinking_level_to_str`]); unknown keys ⇒ `None`.
pub(crate) fn thinking_level_from_str(s: &str) -> Option<ModelThinkingLevel> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/// What [`resolve_model`] hands back: the resolved catalog `Model` and its address (both `None` on
/// a modelless launch, see below), the clamped thinking level, and pi's `modelFallbackMessage`.
type ResolvedModel = (Option<Model>, Option<ModelRef>, ModelThinkingLevel, Option<String>);

/// Resolve `(Option<Model>, Option<ModelRef>, ModelThinkingLevel, modelFallbackMessage)` from the
/// explicit pattern, the resumed session, settings, and finally the catalog (Pi sdk.ts:191-242;
/// R-07-019).
///
/// Precedence mirrors Pi: an explicit `--model` pattern wins; otherwise a resumed session's saved
/// model is restored when it is still resolvable in the catalog (else a `modelFallbackMessage` is
/// produced and we fall back to settings/catalog). The thinking level is likewise restored from the
/// session's `thinking_level_change` entry, then clamped to the chosen model's capabilities.
///
/// # The modelless result (SEAM-075)
/// The model is an `Option` because pi's is: `findInitialModel` legitimately returns
/// `{ model: undefined }` when nothing is configured (model-resolver.ts:648-650), and `sdk.ts:
/// 216-218` turns that into a `modelFallbackMessage` — **a banner, not an error**:
///
/// ```text
/// model = result.model;
/// if (!model) {
///     modelFallbackMessage = formatNoModelsAvailableMessage();
/// } else if (modelFallbackMessage) { … }
/// ```
///
/// The hard stop lives one tier up and is MODE-GATED — `if (appMode !== "interactive" &&
/// !session.model) { console.error(…); process.exit(1); }` (main.ts:852-855) — precisely so a
/// credential-less first run still gets a TUI to type `/login` and then `/model` into. Making an
/// empty catalog fatal HERE would kill that onboarding for every mode, which is exactly the
/// regression this signature closes.
pub(super) fn resolve_model(
    provider: &dyn Provider,
    cfg: &SessionConfig,
    settings: &SettingsManager,
    existing: &cyrup_session::context::SessionContext,
    has_existing_session: bool,
    has_thinking_entry: bool,
) -> Result<ResolvedModel, SessionServiceError> {
    let available = provider.models();
    let resolver = ModelResolver::new(available);
    let mut fallback: Option<String> = None;

    // 1. An explicit `--model` pattern (Pi `options.model`) takes precedence over restore.
    let (mut model, mut parsed_thinking): (Option<Model>, Option<ModelThinkingLevel>) =
        match &cfg.model_pattern {
            Some(pat) => {
                let parsed = resolver.parse_pattern(pat, true);
                match parsed.model {
                    Some(m) => (Some(m), parsed.thinking_level),
                    // Pi `resolveCliModel` fallback (model-resolver.ts:475-501): an unresolvable
                    // `--model` id on a *known* provider does NOT error — it builds a custom-id model
                    // from the provider's default and proceeds (the bin already emitted the
                    // "Using custom model id." warning). The provider is "known" when `--provider` was
                    // explicit OR the pattern carries a `provider/` prefix naming the resolved
                    // provider; a bare unresolvable id with neither stays a hard `ModelNotFound`.
                    None => match fallback_model(provider, cfg, pat) {
                        Some((m, level)) => (Some(m), level),
                        None => return Err(SessionServiceError::ModelNotFound(pat.clone())),
                    },
                }
            }
            None => (None, None),
        };

    // 2. Restore the model from the resumed session (Pi sdk.ts:194-203). The saved model is only
    //    honored when it still resolves in the live catalog (our auth proxy: a model the provider
    //    exposes is usable); otherwise we record the fallback message and keep searching.
    if model.is_none()
        && has_existing_session
        && let Some(saved) = existing.model.as_ref()
    {
        let restored = available.iter().find(|m| {
            m.provider == saved.provider && m.id == saved.model
        });
        match restored {
            Some(m) => model = Some(m.clone()),
            None => {
                fallback = Some(format!(
                    "Could not restore model {}/{}",
                    saved.provider.as_str(),
                    saved.model.as_str()
                ));
            }
        }
    }

    // 3. Settings default → first catalog entry (Pi `findInitialModel`, sdk.ts:205-221).
    if model.is_none() {
        let pat = settings.effective().default_model();
        let resolved = match pat {
            Some(p) => {
                let parsed = resolver.parse_pattern(&p, true);
                parsed_thinking = parsed_thinking.or(parsed.thinking_level);
                parsed.model
            }
            None => None,
        };
        match resolved.or_else(|| available.first().cloned()) {
            Some(m) => {
                if let Some(msg) = fallback.as_mut() {
                    msg.push_str(&format!(". Using {}/{}", m.provider.as_str(), m.id.as_str()));
                }
                model = Some(m);
            }
            // Pi sdk.ts:216-218 — `findInitialModel` returned `{model: undefined}`
            // (model-resolver.ts:648-650). The message REPLACES any "Could not restore model …"
            // text set in step 2, because pi's `if (!model)` branch assigns rather than appends.
            None => fallback = Some(crate::auth_guidance::format_no_models_available_message()),
        }
    }

    // 4. Thinking level: explicit option → restored from session → settings default; clamped to the
    //    chosen model's supported levels (Pi sdk.ts:223-242).
    //    CFG-056: `getDefaultThinkingLevel()` returns `ThinkingLevel | undefined` upstream
    //    (settings-manager.ts:740-742) and each of Pi's sites names the fallback explicitly —
    //    `settingsManager.getDefaultThinkingLevel() ?? DEFAULT_THINKING_LEVEL` (sdk.ts:230, :235).
    //    `DEFAULT_THINKING_LEVEL` is `"medium"` (core/defaults.ts:3), NOT the type's `Off` zero.
    let settings_default = || {
        settings
            .effective()
            .default_thinking_level()
            .unwrap_or(cyrup_config::DEFAULT_THINKING_LEVEL)
    };
    let mut thinking = cfg.thinking_level.or(parsed_thinking);
    if thinking.is_none() && has_existing_session {
        thinking = Some(if has_thinking_entry {
            thinking_level_from_str(&existing.thinking_level).unwrap_or_else(settings_default)
        } else {
            settings_default()
        });
    }
    let thinking = thinking.unwrap_or_else(settings_default);
    // Pi sdk.ts:238-242: `if (!model) { thinkingLevel = "off"; } else { thinkingLevel =
    // clampThinkingLevel(model, thinkingLevel); }` — a modelless session has nothing to clamp
    // against, so the level is forced off rather than carried from settings.
    let thinking = match model.as_ref() {
        Some(m) => cyrup_provider::clamp_thinking_level(m, thinking),
        None => ModelThinkingLevel::Off,
    };

    let model_ref = model.as_ref().map(|m| ModelRef {
        provider: m.provider.clone(),
        api: Some(m.api.clone()),
        model: m.id.clone(),
    });
    Ok((model, model_ref, thinking, fallback))
}

/// Pi `resolveCliModel` custom-fallback (model-resolver.ts:475-501 + `buildFallbackModel`
/// 163-177): when a strict `--model` pattern does not resolve but the provider is *known*, clone the
/// provider's *curated* default (Pi `defaultModelPerProvider`, else its first model) and override
/// `id`/`name` with the requested model id, so an unknown-but-intended model id proceeds as a custom
/// model. The provider is "known" when `--provider` was explicit (`cli_provider_explicit`) or the
/// pattern carries a `provider/` prefix naming the resolved provider. Returns `(model,
/// thinking_level)` or `None` (⇒ the caller keeps Pi's hard `ModelNotFound`). A trailing `:level` is
/// honored only when `--thinking` was not given (Pi `fallbackThinking`, model-resolver.ts:481-490).
fn fallback_model(
    provider: &dyn Provider,
    cfg: &SessionConfig,
    pattern: &str,
) -> Option<(Model, Option<ModelThinkingLevel>)> {
    let provider_id = provider.id();
    let prefix = format!("{}/", provider_id.as_str());
    let has_matching_prefix =
        pattern.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase());
    if !cfg.cli_provider_explicit && !has_matching_prefix {
        return None;
    }
    // Strip the provider prefix (Pi `pattern = cliModel.substring(slashIndex + 1)`), then peel a
    // trailing `:level` thinking suffix when `--thinking` was not explicitly set.
    let stripped: &str =
        if has_matching_prefix { pattern.get(prefix.len()..).unwrap_or(pattern) } else { pattern };
    let (base_id, level): (&str, Option<ModelThinkingLevel>) = if cfg.thinking_level.is_some() {
        (stripped, None)
    } else if let Some(idx) = stripped.rfind(':') {
        let suffix = stripped.get(idx + 1..).unwrap_or("");
        match thinking_level_from_str(suffix) {
            Some(lvl) => (stripped.get(..idx).unwrap_or(stripped), Some(lvl)),
            None => (stripped, None),
        }
    } else {
        (stripped, None)
    };
    if base_id.is_empty() {
        return None;
    }
    // Clone the provider's *curated* default (Pi `defaultModelPerProvider` — e.g. anthropic ->
    // `claude-opus-4-8`), else its first model, overriding id + name (Pi `buildFallbackModel`,
    // model-resolver.ts:163-177). `cyrup_config::build_fallback_model` (model.rs:1033) is the shared
    // helper that mirrors that curated pick exactly. NOTE: `ModelResolver::provider_default` is the
    // WRONG base here — it is alias-preferred + raw-byte-descending (anthropic -> `claude-sonnet-5`),
    // which diverges the cloned model's cost (~2.5x) and compat flags from Pi.
    let model = cyrup_config::build_fallback_model(provider_id.as_str(), base_id, provider.models())?;
    Some((model, level))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    /// PROV-002: the persisted session key for the `max` rung. Both directions go through serde,
    /// so this pins that the enum change actually reaches session replay + the `model:max`
    /// fallback suffix path (`fallback_model` calls `thinking_level_from_str`).
    #[test]
    fn thinking_level_max_round_trips_through_the_persisted_key() {
        use super::{thinking_level_from_str, thinking_level_to_str};
        use cyrup_core::ModelThinkingLevel;

        assert_eq!(thinking_level_to_str(ModelThinkingLevel::Max), "max");
        assert_eq!(thinking_level_from_str("max"), Some(ModelThinkingLevel::Max));
        for level in [
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Minimal,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
            ModelThinkingLevel::Xhigh,
            ModelThinkingLevel::Max,
        ] {
            assert_eq!(
                thinking_level_from_str(&thinking_level_to_str(level)),
                Some(level),
                "{level:?} must survive a persist/restore round-trip"
            );
        }
        assert_eq!(thinking_level_from_str("ultra"), None);
    }

    // Pi `buildFallbackModel` (model-resolver.ts:163-177): a `--model <custom-id>` on a *known*
    // provider clones that provider's **curated** default (`defaultModelPerProvider` — anthropic ->
    // `claude-opus-4-8`), then overrides id/name. The buggy path cloned the alias-preferred,
    // raw-byte-descending pick (`resolver.provider_default` -> `claude-sonnet-5`), diverging cost
    // (~2.5x) and dropping the base's compat flags. This drives the real `fallback_model` site over
    // an assembled two-model anthropic catalog (opus cost 15/75 vs sonnet 6/30).
    #[test]
    fn fallback_model_clones_curated_default_not_alias_preferred_base() {
        use super::{fallback_model, SessionConfig};
        use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
        use cyrup_provider::ModelCost;

        let mk = |id: &str, input: f64, output: f64| {
            let mut d = FauxModelDefinition::new(id);
            d.cost = ModelCost { input, output, cache_read: 0.0, cache_write: 0.0, tiers: None };
            d
        };
        // Order the alias-preferred pick FIRST (byte-descending `s` > `o` -> sonnet), so the naive
        // `providerModels[0]` fallback is ALSO sonnet — only the curated-default lookup rescues opus.
        let provider = FauxProvider::with_config(FauxConfig {
            provider: "anthropic".into(),
            api: "anthropic".into(),
            models: vec![mk("claude-sonnet-5", 6.0, 30.0), mk("claude-opus-4-8", 15.0, 75.0)],
            ..Default::default()
        });
        let mut cfg = SessionConfig::new("/tmp", "/tmp/agent");
        cfg.cli_provider_explicit = true; // provider is "known" -> custom fallback is allowed

        let (model, _lvl) = fallback_model(&provider, &cfg, "my-custom-model")
            .expect("known provider yields a custom fallback model");

        // The requested custom id/name is applied on top of the base.
        assert_eq!(model.id.as_str(), "my-custom-model");
        assert_eq!(model.name, "my-custom-model");
        // The BASE must be the curated default (claude-opus-4-8), so cost matches opus, NOT the
        // alias-preferred claude-sonnet-5. On the buggy code this reads 6.0/30.0 and FAILS.
        assert_eq!(
            model.cost.input, 15.0,
            "fallback must clone curated default claude-opus-4-8, not alias-preferred claude-sonnet-5"
        );
        assert_eq!(model.cost.output, 75.0);
    }
}
