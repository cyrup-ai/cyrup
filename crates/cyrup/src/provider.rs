//! Provider selection seam (arch-11 §1, §3.6 note).
//!
//! Real HTTP providers (anthropic/openai/…) are **not implemented yet**. This seam returns the
//! scripted [`FauxProvider`] for the default / `faux/*` model so the binary is runnable end-to-end,
//! and returns a CLEAR error — never a silent fallback — when a model addressing an unimplemented
//! real provider is requested. When the real providers land they slot in behind this one function.

use std::sync::Arc;

use anyhow::bail;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;

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
/// - `None` or a slash-less pattern, or an explicit `faux/...` ⇒ the in-process [`FauxProvider`].
/// - Any other explicit provider ⇒ a clear error (no real HTTP provider is built yet).
pub fn select_provider(model_pattern: Option<&str>) -> anyhow::Result<Arc<dyn Provider>> {
    match provider_prefix(model_pattern) {
        None | Some("faux") => Ok(Arc::new(FauxProvider::new())),
        Some(other) => bail!(
            "model targets provider '{other}', which has no implementation yet \
             (no real HTTP provider is built). Use a 'faux/...' model for now, or wait for the \
             '{other}' provider to land — there is intentionally no silent fallback."
        ),
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
    fn unimplemented_real_provider_errors_clearly() {
        // `Ok` is `Arc<dyn Provider>` (not `Debug`), so match rather than `unwrap_err`.
        let err = match select_provider(Some("anthropic/claude-opus")) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected an error for an unimplemented real provider"),
        };
        assert!(err.contains("anthropic"));
        assert!(err.contains("no implementation"));
        // No silent fallback.
        assert!(select_provider(Some("openai/gpt-4o")).is_err());
    }
}
