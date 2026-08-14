//! CFG-002 blocker proof at the BINARY seam: a `models.json`-declared provider must be launchable,
//! listable and selectable through the four entry points `main.rs` actually calls.
//!
//! Pi has exactly ONE registry and it is the composed one — `ModelRuntime.rebuildProviders`
//! (model-runtime.ts:225-231) registers a provider for every id in `providerIds()` = `builtins ∪ …
//! ∪ config.getProviderIds()` (:193-199), and `--list-models` / `find` / `setModel` / `stream` all
//! read it (list-models.ts:35). Composing only a `Vec<Model>` while the binary keeps resolving
//! providers out of the raw built-in registry leaves the whole custom-provider surface unreachable:
//! `--model mycorp/mycorp-large` hits the "not a known provider" bail, `--list-models` omits it, a
//! settings `defaultModel` naming it cannot resolve, and an in-session `/model` swap onto it fails.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use crate::provider::{
    BuiltinProviderResolver, all_available_models, default_launch_model, select_provider,
};
use cyrup_config::{AuthStore, ModelFile, load_models_file};
use cyrup_session_svc::ProviderResolver;

fn model_file(json: &str) -> ModelFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, json).unwrap();
    load_models_file(&path).expect("models.json parses")
}

const MYCORP: &str = r#"{
  "providers": {
    "mycorp": {
      "name": "MyCorp",
      "baseUrl": "https://gateway.mycorp.example/v1",
      "api": "openai-completions",
      "apiKey": "sk-mycorp-inline",
      "models": [{ "id": "mycorp-large", "name": "MyCorp Large", "contextWindow": 321000 }]
    }
  }
}"#;

/// `cyrup --model mycorp/mycorp-large` — the launch path (`select_provider`, main.rs).
#[test]
fn a_models_json_provider_can_be_launched_by_model_prefix() {
    let file = model_file(MYCORP);
    let provider = select_provider(None, Some("mycorp/mycorp-large"), None, &file)
        .expect("a models.json-declared provider must be launchable, not a hard bail");
    assert_eq!(provider.id().as_str(), "mycorp");
    let model = provider
        .models()
        .iter()
        .find(|m| m.id.as_str() == "mycorp-large")
        .expect("the launched provider owns the declared model (the builder resolves from here)");
    assert_eq!(model.base_url, "https://gateway.mycorp.example/v1");
    assert_eq!(model.context_window, 321_000);
}

/// The same id with NO models.json still bails clearly — the fix must not turn an unknown provider
/// into a silent fallback (this crate's stated "intentionally no silent fallback").
#[test]
fn an_undeclared_provider_still_errors_clearly() {
    let err = match select_provider(None, Some("mycorp/mycorp-large"), None, &ModelFile::default())
    {
        Err(e) => e.to_string(),
        Ok(_) => panic!("an undeclared provider must still be an error"),
    };
    assert!(err.contains("mycorp"), "{err}");
    assert!(err.contains("not a known provider"), "{err}");
    assert!(err.contains("models.json"), "the error points at the fix: {err}");
}

/// `cyrup --list-models` (Pi `modelRegistry.getAvailable()`, list-models.ts:35).
#[test]
fn a_models_json_model_is_listed_by_list_models() {
    let file = model_file(MYCORP);
    let all = all_available_models(&file);
    assert!(
        all.iter()
            .any(|m| m.provider.as_str() == "mycorp" && m.id.as_str() == "mycorp-large"),
        "--list-models enumerates the COMPOSED registry"
    );
    // and the built-ins are still all there.
    assert!(all.iter().any(|m| m.provider.as_str() == "anthropic"));
}

/// The predicate `main.rs` installs for the default-launch path, over an `auth.json` that does not
/// exist — i.e. a fresh install whose ONLY credential is the one written in `models.json`.
fn launch_predicate<'a>(
    auth: &'a AuthStore,
    file: &'a ModelFile,
) -> impl Fn(&cyrup_provider::Model) -> bool + 'a {
    move |m: &cyrup_provider::Model| {
        cyrup_config::provider_is_configured(auth, file, &m.provider, None)
    }
}

/// A saved settings `defaultModel` naming a custom provider (Pi `findInitialModel` step 3,
/// model-resolver.ts:620-630 — it searches the full composed registry).
///
/// REWRITTEN for CFG-022: this used to pass `|_| true` as the availability predicate, which made it
/// pass identically with `|_| false` — step 3 never consults the predicate, so the file's only
/// launch-path test could not fail on the predicate bug at all. It now drives the REAL predicate.
#[test]
fn a_settings_default_model_can_name_a_models_json_provider() {
    let file = model_file(MYCORP);
    let dir = tempfile::tempdir().unwrap();
    let auth = AuthStore::at(dir.path().join("auth.json"));
    let configured = launch_predicate(&auth, &file);
    let (provider, pattern) =
        default_launch_model(Some("mycorp"), Some("mycorp-large"), &configured, &file)
            .expect("the saved default must resolve against the composed registry");
    assert_eq!(provider, "mycorp");
    assert_eq!(pattern, "mycorp/mycorp-large");
}

/// CFG-022 — `findInitialModel` STEP 4 (model-resolver.ts:633-648), the ONLY step that consults the
/// availability predicate: with no `defaultModel` saved, launch picks the first model whose provider
/// `hasConfiguredAuth`. Pi builds that set from `configuredProviders`, which `models.checkAuth` fills
/// for EVERY composed provider — including a models.json-only one, whose check is the presence-only
/// closure at provider-composer.ts:314-332.
///
/// cyrup's launch predicate was `AuthStore::has_auth` alone, which knows only `--api-key`, an
/// `auth.json` entry, and the `env_keys` table of KNOWN provider ids. A user-declared provider
/// matches none of the three, so a fresh install whose only credential lives in `models.json`
/// filtered its own provider out and fell back to the offline faux provider.
#[test]
fn a_models_json_provider_is_available_at_launch_with_no_auth_json() {
    let file = model_file(MYCORP);
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    assert!(!auth_path.exists(), "fresh install: nothing stored");
    let auth = AuthStore::at(auth_path);
    let configured = launch_predicate(&auth, &file);

    // Step 4's input list: the composed registry filtered by the predicate.
    let available: Vec<_> = all_available_models(&file)
        .into_iter()
        .filter(&configured)
        .collect();
    assert!(
        available
            .iter()
            .any(|m| m.provider.as_str() == "mycorp" && m.id.as_str() == "mycorp-large"),
        "a models.json provider carrying its own apiKey is configured auth; step 4 saw only {:?}",
        available
            .iter()
            .map(|m| m.provider.as_str().to_string())
            .collect::<std::collections::BTreeSet<_>>()
    );

    // …so step 4 always yields a real provider rather than leaving the caller on faux.
    assert!(
        default_launch_model(None, None, &configured, &file).is_some(),
        "with an available model, step 4 must resolve a launch model"
    );
}

/// The negative half, so the predicate cannot be "always true": a models.json block with NO `apiKey`
/// (a `baseUrl`-only proxy overlay on a built-in) contributes no credential of its own.
#[test]
fn a_models_json_provider_without_an_api_key_is_not_configured() {
    let file = model_file(
        r#"{"providers":{"keyless":{"baseUrl":"https://keyless.example/v1","api":"openai-completions",
             "models":[{"id":"k1"}]}}}"#,
    );
    let dir = tempfile::tempdir().unwrap();
    let auth = AuthStore::at(dir.path().join("auth.json"));
    let configured = launch_predicate(&auth, &file);
    assert!(
        !all_available_models(&file)
            .iter()
            .filter(|m| m.provider.as_str() == "keyless")
            .any(&configured),
        "no apiKey, no stored credential, no known env var — not configured"
    );
}

/// An in-session `/model` swap onto the custom provider (`BuiltinProviderResolver` is what
/// `AgentSession::set_model_resolved` calls for a cross-provider switch).
#[test]
fn an_in_session_model_swap_resolves_a_models_json_provider() {
    let resolver = BuiltinProviderResolver::new(Arc::new(model_file(MYCORP)));
    let provider = resolver
        .resolve("mycorp")
        .expect("a /model selection targeting a declared provider must resolve");
    assert_eq!(provider.id().as_str(), "mycorp");
}

/// The overlay half at the binary seam: a `baseUrl` block on an EXISTING built-in must rewrite the
/// provider handed to the session, so the launched session streams at the proxy.
#[test]
fn a_base_url_block_rewrites_the_launched_builtin_provider() {
    let file = model_file(r#"{"providers":{"anthropic":{"baseUrl":"https://proxy.internal/v1"}}}"#);
    let provider = select_provider(Some("anthropic"), None, None, &file).expect("anthropic");
    assert!(!provider.models().is_empty());
    assert!(
        provider
            .models()
            .iter()
            .all(|m| m.base_url == "https://proxy.internal/v1"),
        "every model of the launched provider carries the override: {:?}",
        provider
            .models()
            .iter()
            .map(|m| m.base_url.clone())
            .take(3)
            .collect::<Vec<_>>()
    );
}

/// LOUD AND SAFE (constraint 6): a bad provider block is reported by name and costs nothing else —
/// the built-ins still resolve and the good sibling block still applies.
#[test]
fn a_bad_provider_block_is_reported_and_does_not_break_resolution() {
    let file = model_file(
        r#"{"providers":{
             "broken": {"models":[{"id":"nope"}]},
             "mycorp": {"baseUrl":"https://gateway.mycorp.example/v1","api":"openai-completions",
                        "apiKey":"sk-x","models":[{"id":"mycorp-large"}]}
           }}"#,
    );
    let errors = crate::provider::models_json_composition_errors(&file);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("broken"), "{}", errors[0]);
    // The rest of the world is untouched.
    assert!(select_provider(Some("anthropic"), None, None, &file).is_ok());
    assert!(select_provider(None, Some("mycorp/mycorp-large"), None, &file).is_ok());
    assert!(select_provider(None, Some("broken/nope"), None, &file).is_err());
}
