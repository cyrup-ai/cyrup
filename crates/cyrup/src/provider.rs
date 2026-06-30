//! Provider selection seam (arch-11 §1, §3.6 note).
//!
//! Resolves a `(provider, model, apiKey)` triple to the owning [`Provider`] exactly as Pi's
//! `resolveCliModel` does (main.ts:352-448): an explicit `--provider` wins, else the `provider/...`
//! prefix on `--model`, else the default offline [`FauxProvider`]. A `--api-key` is installed as a
//! runtime credential for the resolved provider (Pi `options.apiKey`). The default / `faux/*` pattern
//! returns the in-process scripted [`FauxProvider`] (offline, runnable end-to-end); any explicit
//! provider is looked up in the Pi-faithful built-in registry ([`cyrup_provider::default_models`] —
//! the 1:1 port of Pi `providers/all.ts`). There is intentionally NO silent fallback: a prefix that
//! is not a built-in provider is a clear error listing the providers that ARE available.

use std::sync::Arc;

use anyhow::bail;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{
    default_models, Credential, CreateModelsOptions, InMemoryCredentialStore, Provider,
};
use cyrup_sdk::core::ProviderId;

/// Every model across ALL built-in providers — the faithful data source for `--list-models` (Pi
/// `modelRegistry.getAvailable()`, list-models.ts:35). Independent of `--provider`/`--model`: Pi's
/// `listModels` always enumerates the full multi-provider registry (`providers/all.ts`), never just
/// the session's selected provider. The offline scripted faux provider is intentionally excluded (it
/// is a cyrup run-time default, not a catalog entry, and has no analog in Pi's production registry).
pub fn all_available_models() -> Vec<cyrup_provider::Model> {
    let models = default_models(CreateModelsOptions { credentials: None, auth_context: None });
    models.get_models(None)
}

/// The provider id a model pattern addresses, if it carries an explicit `provider/...` prefix.
fn provider_prefix(model_pattern: Option<&str>) -> Option<&str> {
    let pattern = model_pattern?;
    let (prefix, _) = pattern.split_once('/')?;
    if prefix.is_empty() {
        None
    } else {
        Some(prefix)
    }
}

/// The resolved provider id (Pi `resolveCliModel` precedence): explicit `--provider`, else the
/// `--model` prefix, else `None` (⇒ the offline faux provider).
fn resolve_provider_id<'a>(
    provider_override: Option<&'a str>,
    model_pattern: Option<&'a str>,
) -> Option<&'a str> {
    provider_override.filter(|s| !s.is_empty()).or_else(|| provider_prefix(model_pattern))
}

/// Resolve a [`Provider`] for the requested `(provider, model, apiKey)` triple.
///
/// - No explicit provider/prefix, or an explicit `faux` ⇒ the in-process [`FauxProvider`].
/// - Any other explicit provider ⇒ looked up in the built-in registry (Pi `providers/all.ts`), with
///   `api_key` installed as a runtime credential when present (Pi `options.apiKey`).
/// - A provider that is not a built-in ⇒ a clear error listing the available built-ins.
pub fn select_provider(
    provider_override: Option<&str>,
    model_pattern: Option<&str>,
    api_key: Option<&str>,
) -> anyhow::Result<Arc<dyn Provider>> {
    match resolve_provider_id(provider_override, model_pattern) {
        None | Some("faux") => Ok(Arc::new(FauxProvider::new())),
        Some(id) => {
            // Install the runtime `--api-key` as a credential for the resolved provider so the
            // provider streams with it (Pi threads `apiKey` into the auth context). Absent a key the
            // env-backed default auth resolves the key at stream time.
            let credentials = api_key.map(|key| {
                let store = InMemoryCredentialStore::new()
                    .with_credential(ProviderId::from(id), Credential::api_key(key));
                Arc::new(store) as Arc<dyn cyrup_provider::CredentialStore>
            });
            let models = default_models(CreateModelsOptions { credentials, auth_context: None });
            match models.get_provider(id) {
                Some(provider) => Ok(provider),
                None => {
                    let mut available: Vec<String> = models
                        .get_providers()
                        .iter()
                        .map(|p| p.id().as_str().to_string())
                        .collect();
                    available.sort();
                    bail!(
                        "model targets provider '{id}', which is not a built-in provider. \
                         Available built-in providers: {}. \
                         (Use a 'faux/...' model for the offline scripted provider; there is \
                         intentionally no silent fallback.)",
                        available.join(", ")
                    )
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn all_available_models_span_the_full_registry() {
        // The full multi-provider registry (Pi `getAvailable`), not just one provider's catalog.
        let models = all_available_models();
        assert!(!models.is_empty());
        let providers: std::collections::BTreeSet<&str> =
            models.iter().map(|m| m.provider.as_str()).collect();
        // Several distinct built-in providers are represented (anthropic/openai/together at minimum).
        assert!(providers.len() > 1, "expected multiple providers, got {providers:?}");
        assert!(providers.contains("anthropic"));
        assert!(providers.contains("openai"));
        assert!(providers.contains("together"));
        // The offline scripted faux provider is NOT a catalog entry.
        assert!(!providers.contains("faux"));
    }

    #[test]
    fn defaults_and_faux_resolve_to_faux() {
        assert_eq!(select_provider(None, None, None).unwrap().id().as_str(), "faux");
        assert_eq!(select_provider(None, Some("faux-1"), None).unwrap().id().as_str(), "faux");
        assert_eq!(select_provider(None, Some("faux/faux-1"), None).unwrap().id().as_str(), "faux");
        assert_eq!(select_provider(Some("faux"), None, None).unwrap().id().as_str(), "faux");
    }

    #[test]
    fn explicit_provider_override_wins_over_model_prefix() {
        // `--provider openai` with a bare model resolves to openai (Pi precedence).
        let p = select_provider(Some("openai"), Some("gpt-4o"), None).expect("openai built-in");
        assert_eq!(p.id().as_str(), "openai");
    }

    #[test]
    fn built_in_real_providers_resolve_from_the_registry() {
        let anthropic =
            select_provider(None, Some("anthropic/claude-opus"), None).expect("anthropic built-in");
        assert_eq!(anthropic.id().as_str(), "anthropic");
        let openai = select_provider(None, Some("openai/gpt-4o"), None).expect("openai built-in");
        assert_eq!(openai.id().as_str(), "openai");
    }

    #[test]
    fn api_key_is_accepted_for_a_real_provider() {
        let p = select_provider(Some("openai"), Some("openai/gpt-4o"), Some("sk-runtime"))
            .expect("openai built-in with runtime key");
        assert_eq!(p.id().as_str(), "openai");
    }

    #[test]
    fn together_kimi_resolves_to_together_provider() {
        let together = select_provider(None, Some("together/moonshotai/Kimi-K2.6"), None)
            .expect("together is built-in");
        assert_eq!(together.id().as_str(), "together");
        assert!(together.models().iter().any(|m| m.id.as_str() == "moonshotai/Kimi-K2.6"));
    }

    #[test]
    fn truly_unknown_provider_errors_clearly() {
        let err = match select_provider(None, Some("definitely-not-a-provider/whatever"), None) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected an error for an unknown provider"),
        };
        assert!(err.contains("definitely-not-a-provider"));
        assert!(err.contains("not a built-in provider"));
        assert!(err.contains("together"));
        assert!(err.contains("anthropic"));
    }
}
