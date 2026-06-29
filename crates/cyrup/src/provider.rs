//! Provider selection seam (arch-11 §1, §3.6 note).
//!
//! Resolves a model pattern to its owning [`Provider`] exactly as Pi does: the default / `faux/*`
//! pattern returns the in-process scripted [`FauxProvider`] (offline, runnable end-to-end), and any
//! explicit `provider/...` prefix is looked up in the Pi-faithful built-in registry
//! ([`cyrup_provider::default_models`] / [`cyrup_provider::all_providers`] — the 1:1 port of Pi
//! `providers/all.ts`). The resolved provider's catalog then drives model resolution downstream.
//! There is intentionally NO silent fallback: a prefix that is not a built-in provider is a clear
//! error listing the providers that ARE available.

use std::sync::Arc;

use anyhow::bail;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{default_models, CreateModelsOptions, Provider};

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

/// Resolve a [`Provider`] for the requested model pattern (`provider/id[:level]`).
///
/// - `None`, a slash-less pattern, or an explicit `faux/...` ⇒ the in-process [`FauxProvider`].
/// - Any other explicit provider ⇒ looked up in the built-in registry (Pi `providers/all.ts`). The
///   provider's own catalog drives model resolution.
/// - A prefix that is not a built-in provider ⇒ a clear error listing the available built-ins (no
///   silent fallback).
pub fn select_provider(model_pattern: Option<&str>) -> anyhow::Result<Arc<dyn Provider>> {
    match provider_prefix(model_pattern) {
        None | Some("faux") => Ok(Arc::new(FauxProvider::new())),
        Some(other) => {
            // The Pi-faithful registry: every implemented built-in provider, env-backed auth context
            // (so env API keys resolve when the provider streams).
            let models = default_models(CreateModelsOptions::default());
            match models.get_provider(other) {
                Some(provider) => Ok(provider),
                None => {
                    let mut available: Vec<String> = models
                        .get_providers()
                        .iter()
                        .map(|p| p.id().as_str().to_string())
                        .collect();
                    available.sort();
                    bail!(
                        "model targets provider '{other}', which is not a built-in provider. \
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
    fn defaults_and_faux_resolve_to_faux() {
        assert_eq!(select_provider(None).unwrap().id().as_str(), "faux");
        // A slash-less pattern is treated as a model id under the default (faux) provider.
        assert_eq!(select_provider(Some("faux-1")).unwrap().id().as_str(), "faux");
        assert_eq!(select_provider(Some("faux/faux-1")).unwrap().id().as_str(), "faux");
        assert_eq!(select_provider(Some("faux/faux-1:high")).unwrap().id().as_str(), "faux");
    }

    #[test]
    fn built_in_real_providers_resolve_from_the_registry() {
        // `anthropic/...` now resolves to the real anthropic provider (formerly an "unimplemented"
        // error). The catalog (not this seam) decides whether the specific model id exists.
        let anthropic = select_provider(Some("anthropic/claude-opus")).expect("anthropic is built-in");
        assert_eq!(anthropic.id().as_str(), "anthropic");

        // openai is likewise a built-in provider now.
        let openai = select_provider(Some("openai/gpt-4o")).expect("openai is built-in");
        assert_eq!(openai.id().as_str(), "openai");
    }

    #[test]
    fn together_kimi_resolves_to_together_provider() {
        let together =
            select_provider(Some("together/moonshotai/Kimi-K2.6")).expect("together is built-in");
        assert_eq!(together.id().as_str(), "together");
        // The together catalog owns the model id (proves the provider's catalog drives resolution).
        assert!(together.models().iter().any(|m| m.id.as_str() == "moonshotai/Kimi-K2.6"));
    }

    #[test]
    fn truly_unknown_provider_errors_clearly() {
        // `Ok` is `Arc<dyn Provider>` (not `Debug`), so match rather than `unwrap_err`.
        let err = match select_provider(Some("definitely-not-a-provider/whatever")) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected an error for an unknown provider"),
        };
        assert!(err.contains("definitely-not-a-provider"));
        assert!(err.contains("not a built-in provider"));
        // The error lists real available built-ins (no silent fallback).
        assert!(err.contains("together"));
        assert!(err.contains("anthropic"));
    }
}
