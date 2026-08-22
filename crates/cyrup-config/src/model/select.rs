//! Which model a run actually starts on: the CLI `--model`/`--provider` resolution, the
//! initial-model priority ladder, and session restore with its auth-aware fallback.

use cyrup_core::ModelThinkingLevel;
use cyrup_provider::Model;

use super::defaults::{build_fallback_model, first_default_or_first};
use super::resolver::{ModelResolver, ScopedModel, parse_thinking_level};

/// `true` if two models refer to the same provider+id (Pi `modelsAreEqual`, models.ts:435).
fn models_are_equal(a: &Model, b: &Model) -> bool {
    a.id == b.id && a.provider == b.provider
}
/// Result of [`resolve_cli_model`] (Pi `ResolveCliModelResult`, model-resolver.ts:318-327).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub warning: Option<String>,
    /// CLI-display error; when set, `model` is `None`.
    pub error: Option<String>,
}

/// Resolve a single model from CLI flags (Pi `resolveCliModel`, model-resolver.ts:340-511).
///
/// `all` is the full model set (NOT just authed models, so `--api-key` first-time setup works).
/// `has_configured_auth` reports whether a model has usable auth (Pi `modelRegistry.hasConfiguredAuth`).
pub fn resolve_cli_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    cli_thinking: Option<ModelThinkingLevel>,
    all: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> CliModelResult {
    let Some(cli_model) = cli_model else {
        return CliModelResult::default();
    };

    if all.is_empty() {
        return CliModelResult {
            error: Some(
                "No models available. Check your installation or add models to models.json."
                    .to_string(),
            ),
            ..Default::default()
        };
    }

    // Canonical (case-insensitive) provider lookup.
    let canonical_provider = |name: &str| -> Option<String> {
        all.iter()
            .find(|m| m.provider.as_str().eq_ignore_ascii_case(name))
            .map(|m| m.provider.as_str().to_string())
    };

    let mut provider = cli_provider.and_then(canonical_provider);
    if cli_provider.is_some() && provider.is_none() {
        return CliModelResult {
            error: Some(format!(
                "Unknown provider \"{}\". Use --list-models to see available providers/models.",
                cli_provider.unwrap_or("")
            )),
            ..Default::default()
        };
    }

    let mut pattern = cli_model.to_string();
    let mut inferred_provider = false;

    // Infer `provider/model` when the prefix matches a known provider.
    if provider.is_none()
        && let Some(slash) = cli_model.find('/')
    {
        let maybe = &cli_model[..slash];
        if let Some(canonical) = canonical_provider(maybe) {
            provider = Some(canonical);
            pattern = cli_model[slash + 1..].to_string();
            inferred_provider = true;
        }
    }

    // No provider inferred: try exact id / provider/id match across all models.
    if provider.is_none() {
        let lower = cli_model.to_ascii_lowercase();
        if let Some(exact) = all.iter().find(|m| {
            m.id.as_str().to_ascii_lowercase() == lower
                || format!("{}/{}", m.provider, m.id).to_ascii_lowercase() == lower
        }) {
            return CliModelResult {
                model: Some(exact.clone()),
                ..Default::default()
            };
        }
    }

    // Both --provider and --model <provider>/<pattern>: strip the redundant prefix.
    if let (Some(cp), Some(p)) = (cli_provider, provider.as_deref()) {
        let _ = cp;
        let prefix = format!("{p}/");
        if cli_model
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            pattern = cli_model[prefix.len()..].to_string();
        }
    }

    let candidates: Vec<Model> = match provider.as_deref() {
        Some(p) => all
            .iter()
            .filter(|m| m.provider.as_str() == p)
            .cloned()
            .collect(),
        None => all.to_vec(),
    };
    let resolver = ModelResolver::new(&candidates);
    let parsed = resolver.parse_pattern(&pattern, true);

    if let Some(model) = parsed.model.clone() {
        // Provider inference matched an unauthenticated pair: prefer an authed raw id match.
        if inferred_provider {
            let raw_exact: Vec<&Model> = all
                .iter()
                .filter(|m| {
                    m.id.as_str().eq_ignore_ascii_case(cli_model) && !models_are_equal(m, &model)
                })
                .collect();
            if !raw_exact.is_empty() && !has_configured_auth(&model) {
                let authed: Vec<&Model> = raw_exact
                    .into_iter()
                    .filter(|m| has_configured_auth(m))
                    .collect();
                if authed.len() == 1
                    && let Some(m) = authed.first()
                {
                    return CliModelResult {
                        model: Some((*m).clone()),
                        ..Default::default()
                    };
                }
            }
        }
        return CliModelResult {
            model: Some(model),
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            error: None,
        };
    }

    // Inferred a provider but no match within it: fall back to a raw id match across all models.
    if inferred_provider {
        let lower = cli_model.to_ascii_lowercase();
        if let Some(exact) = all.iter().find(|m| {
            m.id.as_str().to_ascii_lowercase() == lower
                || format!("{}/{}", m.provider, m.id).to_ascii_lowercase() == lower
        }) {
            return CliModelResult {
                model: Some(exact.clone()),
                ..Default::default()
            };
        }
        let fallback = ModelResolver::new(all).parse_pattern(cli_model, true);
        if let Some(m) = fallback.model {
            return CliModelResult {
                model: Some(m),
                thinking_level: fallback.thinking_level,
                warning: fallback.warning,
                error: None,
            };
        }
    }

    if let Some(p) = provider.as_deref() {
        // Parse a `:level` suffix from the pattern before building the fallback model.
        let mut fallback_pattern = pattern.clone();
        let mut fallback_thinking: Option<ModelThinkingLevel> = None;
        if cli_thinking.is_none()
            && let Some(idx) = pattern.rfind(':')
        {
            let suffix = pattern.get(idx + 1..).unwrap_or("");
            if let Some(lvl) = parse_thinking_level(suffix) {
                fallback_pattern = pattern.get(..idx).unwrap_or(&pattern).to_string();
                fallback_thinking = Some(lvl);
            }
        }
        if let Some(mut fallback_model) = build_fallback_model(p, &fallback_pattern, all) {
            let requested = cli_thinking.or(fallback_thinking);
            if matches!(requested, Some(l) if l.is_on()) {
                fallback_model.reasoning = true;
            }
            let base_warn = format!(
                "Model \"{fallback_pattern}\" not found for provider \"{p}\". Using custom model id."
            );
            let warning = match parsed.warning {
                Some(w) => format!("{w} {base_warn}"),
                None => base_warn,
            };
            return CliModelResult {
                model: Some(fallback_model),
                thinking_level: fallback_thinking,
                warning: Some(warning),
                error: None,
            };
        }
    }

    let display = match provider.as_deref() {
        Some(p) => format!("{p}/{pattern}"),
        None => cli_model.to_string(),
    };
    CliModelResult {
        model: None,
        thinking_level: None,
        warning: parsed.warning,
        error: Some(format!(
            "Model \"{display}\" not found. Use --list-models to see available models."
        )),
    }
}

/// Result of [`find_initial_model`] (Pi `InitialModelResult`, model-resolver.ts:513-517).
#[derive(Clone, Debug, PartialEq)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: ModelThinkingLevel,
    pub fallback_message: Option<String>,
    /// A CLI error surfaced from step 1 (`resolve_cli_model`). Pi calls `process.exit(1)`; the bin
    /// owns process exit, so we propagate the message instead (additive, non-panicking).
    pub error: Option<String>,
}

/// Find the initial model by priority (Pi `findInitialModel`, model-resolver.ts:527-607):
/// 1) CLI provider+model, 2) first scoped (unless continuing), 3) saved settings default,
/// 4) first available model matching a curated default (else first available), 5) none.
#[allow(clippy::too_many_arguments)]
pub fn find_initial_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    scoped_models: &[ScopedModel],
    is_continuing: bool,
    default_provider: Option<&str>,
    default_model_id: Option<&str>,
    default_thinking_level: Option<ModelThinkingLevel>,
    all: &[Model],
    available: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> InitialModelResult {
    // Pi `model-resolver.ts:594` @v0.83.0 — `let thinkingLevel: ThinkingLevel =
    // DEFAULT_THINKING_LEVEL;`, re-named at every one of this function's return arms (`:608`,
    // `:616`, `:642`, `:647`, `:651`). CFG-056: this was `ModelThinkingLevel::default()` (= `Off`),
    // which is the type's zero, not upstream's unset-fallback (`medium`).
    let default_level = crate::DEFAULT_THINKING_LEVEL;

    // 1. CLI args take priority.
    if let (Some(_), Some(_)) = (cli_provider, cli_model) {
        let resolved = resolve_cli_model(cli_provider, cli_model, None, all, has_configured_auth);
        if let Some(err) = resolved.error {
            return InitialModelResult {
                model: None,
                thinking_level: default_level,
                fallback_message: None,
                error: Some(err),
            };
        }
        if let Some(model) = resolved.model {
            return InitialModelResult {
                model: Some(model),
                thinking_level: default_level,
                fallback_message: None,
                error: None,
            };
        }
    }

    // 2. First scoped model (unless continuing/resuming a session).
    if let Some(first) = scoped_models.first()
        && !is_continuing
    {
        return InitialModelResult {
            model: Some(first.model.clone()),
            thinking_level: first
                .thinking_level
                .or(default_thinking_level)
                .unwrap_or(default_level),
            fallback_message: None,
            error: None,
        };
    }

    // 3. Saved default from settings if auth is configured (Pi model-resolver.ts:621-630
    //    @v0.83.0: `if (found && modelRuntime.hasConfiguredAuth(found.provider))`, falling through
    //    to step 4 at `:632` when the check fails).
    if let (Some(dp), Some(dm)) = (default_provider, default_model_id)
        && let Some(found) = all
            .iter()
            .find(|m| m.provider.as_str() == dp && m.id.as_str() == dm)
        && has_configured_auth(found)
    {
        return InitialModelResult {
            model: Some(found.clone()),
            thinking_level: default_thinking_level.unwrap_or(default_level),
            fallback_message: None,
            error: None,
        };
    }

    // 4. First available model with valid auth (curated-default first).
    if let Some(model) = first_default_or_first(available) {
        return InitialModelResult {
            model: Some(model),
            thinking_level: default_level,
            fallback_message: None,
            error: None,
        };
    }

    // 5. No model.
    InitialModelResult {
        model: None,
        thinking_level: default_level,
        fallback_message: None,
        error: None,
    }
}

/// Result of [`restore_model_from_session`] (Pi `restoreModelFromSession` return,
/// model-resolver.ts:612-681).
#[derive(Clone, Debug, PartialEq)]
pub struct RestoredModelResult {
    pub model: Option<Model>,
    pub fallback_message: Option<String>,
}

/// Restore a model saved in a session, re-checking auth and falling back (Pi
/// `restoreModelFromSession`, model-resolver.ts:612-681). The console messaging is a front-end
/// concern; only the model + fallback message are returned.
pub fn restore_model_from_session(
    saved_provider: &str,
    saved_model_id: &str,
    current_model: Option<&Model>,
    all: &[Model],
    available: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> RestoredModelResult {
    let restored = all
        .iter()
        .find(|m| m.provider.as_str() == saved_provider && m.id.as_str() == saved_model_id);
    let restored_has_auth = restored.is_some_and(has_configured_auth);

    if let Some(model) = restored
        && restored_has_auth
    {
        return RestoredModelResult {
            model: Some(model.clone()),
            fallback_message: None,
        };
    }

    let reason = if restored.is_none() {
        "model no longer exists"
    } else {
        "no auth configured"
    };

    if let Some(current) = current_model {
        return RestoredModelResult {
            model: Some(current.clone()),
            fallback_message: Some(format!(
                "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
                current.provider, current.id
            )),
        };
    }

    if let Some(fallback) = first_default_or_first(available) {
        let msg = format!(
            "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
            fallback.provider, fallback.id
        );
        return RestoredModelResult {
            model: Some(fallback),
            fallback_message: Some(msg),
        };
    }

    RestoredModelResult {
        model: None,
        fallback_message: None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::fixtures::model;

    #[test]
    fn resolve_cli_model_provider_and_pattern() {
        let models = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("openai", "gpt-5.5", "GPT"),
        ];
        let auth = |_: &Model| true;
        // --provider anthropic --model opus → fuzzy match.
        let r = resolve_cli_model(Some("anthropic"), Some("opus"), None, &models, &auth);
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        assert!(r.error.is_none());
        // provider/model inference.
        let r = resolve_cli_model(None, Some("openai/gpt-5.5"), None, &models, &auth);
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
        // unknown provider → error.
        let r = resolve_cli_model(Some("nope"), Some("x"), None, &models, &auth);
        assert!(r.error.as_ref().unwrap().contains("Unknown provider"));
    }

    #[test]
    fn resolve_cli_model_builds_fallback_custom_id() {
        // A custom model id under a known provider builds a fallback from the provider default.
        let models = vec![model("anthropic", "claude-opus-4-8", "Opus")];
        let auth = |_: &Model| true;
        let r = resolve_cli_model(
            Some("anthropic"),
            Some("my-custom-id"),
            None,
            &models,
            &auth,
        );
        let m = r.model.as_ref().unwrap();
        assert_eq!(m.id.as_str(), "my-custom-id");
        assert_eq!(m.provider.as_str(), "anthropic");
        assert!(
            r.warning
                .as_ref()
                .unwrap()
                .contains("Using custom model id")
        );
    }

    #[test]
    fn find_initial_model_priority() {
        let all = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("openai", "gpt-5.5", "GPT"),
        ];
        let available = all.clone();
        let auth = |_: &Model| true;
        // CLI args win.
        let r = find_initial_model(
            Some("openai"),
            Some("gpt-5.5"),
            &[],
            false,
            None,
            None,
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
        // No CLI, no scoped, no saved → curated default (anthropic first in table → opus).
        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            None,
            None,
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        // Saved settings default beats curated default.
        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            Some("openai"),
            Some("gpt-5.5"),
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
    }

    #[test]
    fn restore_model_falls_back_when_no_auth() {
        let all = vec![model("anthropic", "claude-opus-4-8", "Opus")];
        let available = all.clone();
        // saved model has no auth → fall back to curated default with a message.
        let no_auth = |_: &Model| false;
        let r = restore_model_from_session(
            "anthropic",
            "claude-opus-4-8",
            None,
            &all,
            &available,
            &no_auth,
        );
        assert!(
            r.fallback_message
                .as_ref()
                .unwrap()
                .contains("no auth configured")
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        // saved model with auth → restored, no message.
        let yes_auth = |_: &Model| true;
        let r = restore_model_from_session(
            "anthropic",
            "claude-opus-4-8",
            None,
            &all,
            &available,
            &yes_auth,
        );
        assert!(r.fallback_message.is_none());
    }

    /// CFG-023: step 3 accepts the saved default ONLY when its provider has configured auth
    /// (`if (found && modelRuntime.hasConfiguredAuth(found.provider))`, model-resolver.ts:621-630
    /// @v0.83.0), otherwise falling through to step 4 (`:632`).
    ///
    /// Red at HEAD: step 3 returned the saved default unconditionally, so a user who removed a
    /// provider's credentials kept launching into it and got an auth error per turn.
    #[test]
    fn saved_default_is_skipped_when_its_provider_has_no_configured_auth() {
        let all = vec![
            model("anthropic", "claude-opus-4-8", "Claude Opus"),
            model("openai", "gpt-5.5", "GPT 5.5"),
        ];
        let available = vec![model("openai", "gpt-5.5", "GPT 5.5")];
        let has_auth = |m: &Model| m.provider.as_str() == "openai";

        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            Some("anthropic"),
            Some("claude-opus-4-8"),
            None,
            &all,
            &available,
            &has_auth,
        );
        // Step 4's curated default for the only configured provider, NOT the saved anthropic one.
        assert_eq!(r.model.as_ref().unwrap().provider.as_str(), "openai");

        // With auth present the saved default still wins.
        let has_auth_all = |_: &Model| true;
        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            Some("anthropic"),
            Some("claude-opus-4-8"),
            None,
            &all,
            &available,
            &has_auth_all,
        );
        assert_eq!(r.model.as_ref().unwrap().provider.as_str(), "anthropic");
    }
}
