//! Model-registry `/model` selector parity tests (vs Pi `model-selector.ts` +
//! `model-registry.ts`): the interactive selector must enumerate the FULL multi-provider registry
//! filtered to CONFIGURED providers (`modelRegistry.getAvailable()`), NOT just the single injected
//! provider, and selecting a model from a DIFFERENT provider must swap the session's owning provider
//! live (Pi model+provider switch).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_config::AuthStore;
use cyrup_core::{ExtensionId, ProviderId};
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_provider::{CreateModelsOptions, Provider};
use cyrup_session_svc::{ProviderResolver, SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// Provider env keys that `cyrup-config`'s `env_keys` resolves to a CONFIGURED provider. A real
/// `ANTHROPIC_API_KEY` in the developer's environment makes `anthropic` genuinely configured, so a
/// test asserting it is filtered out as UNCONFIGURED is asserting a property of the ambient shell,
/// not of the code. Pi avoids this class wholesale by running its suite under `env -i` with an
/// explicit allowlist (`pi/test.sh`); cyrup has no such wrapper, so a test that depends on a
/// provider being unconfigured has to establish that itself.
const SCRUBBED_PROVIDER_ENV_KEYS: &[&str] = &["ANTHROPIC_API_KEY", "OPENAI_API_KEY"];

/// Serializes the env mutation below. `std::env` is process-global while `cargo test` runs a
/// binary's tests on multiple threads, so an unsynchronized scrub would race any sibling test.
static ENV_SCRUB_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Removes [`SCRUBBED_PROVIDER_ENV_KEYS`] for as long as the guard lives, then restores exactly
/// what was there — including restoring nothing when the var was absent, which is the common case
/// in CI and the case the previous version of this test silently depended on.
struct ScrubbedProviderEnv {
    _guard: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl ScrubbedProviderEnv {
    fn acquire() -> Self {
        let guard = ENV_SCRUB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = SCRUBBED_PROVIDER_ENV_KEYS
            .iter()
            .map(|key| {
                let previous = std::env::var(key).ok();
                // SAFETY: Rust 2024 requires unsafe for env mutation because it is not
                // thread-safe. ENV_SCRUB_LOCK serializes every mutation in this binary, and the
                // value is restored in Drop.
                unsafe { std::env::remove_var(key) };
                (*key, previous)
            })
            .collect();
        Self {
            _guard: guard,
            saved,
        }
    }
}

impl Drop for ScrubbedProviderEnv {
    fn drop(&mut self) {
        for (key, previous) in &self.saved {
            match previous {
                // SAFETY: as in acquire — still holding ENV_SCRUB_LOCK.
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

fn two_model_faux() -> Arc<FauxProvider> {
    let cfg = FauxConfig {
        models: vec![FauxModelDefinition::new("faux-1"), FauxModelDefinition::new("faux-2")],
        ..FauxConfig::default()
    };
    Arc::new(FauxProvider::with_config(cfg))
}

/// A [`ProviderResolver`] backed by the built-in registry (Pi `providers/all.ts`), the same source
/// the real bin's `select_provider` resolves against — so the swap test exercises the actual
/// registry rather than a hand-rolled stub.
struct RegistryResolver;
impl ProviderResolver for RegistryResolver {
    fn resolve(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        cyrup_provider::default_models(CreateModelsOptions {
            credentials: None,
            auth_context: None,
            catalog_overlay: None,
        })
            .get_provider(provider_id)
            .ok_or_else(|| format!("no built-in provider '{provider_id}'"))
    }
}

/// `available_model_catalog()` = Pi `getAvailable()`: the FULL registry filtered to configured
/// providers. With `together` configured (a runtime `--api-key`, the same tier a `TOGETHER_API_KEY`
/// env var resolves to via `cyrup-config` `env_keys`), together's models appear; the current faux
/// provider stays selectable; unconfigured providers (openai/anthropic) are filtered out.
#[tokio::test]
async fn selector_lists_configured_non_faux_provider_and_hides_unconfigured() {
    // The openai/anthropic assertions below are only meaningful if those providers are genuinely
    // unconfigured. Establish that rather than inheriting it from the shell.
    let _env = ScrubbedProviderEnv::acquire();
    let fx = fixture();
    let provider: Arc<dyn Provider> = two_model_faux();
    let auth = Arc::new(AuthStore::at(fx.agent_dir.join("auth.json")));
    auth.set_runtime_api_key(ProviderId::from("together"), "sk-together-test".to_string());
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-1".to_string());
    let session = SessionBuilder::new(provider, cfg).auth(auth).build().await.unwrap();

    let catalog = session.available_model_catalog();
    assert!(
        catalog.iter().any(|m| m.provider.as_str() == "together"),
        "together must be listed once its key is configured (Pi getAvailable)"
    );
    assert!(
        catalog.iter().any(|m| m.provider.as_str() == "faux"),
        "the current (faux) provider stays selectable"
    );
    assert!(
        !catalog.iter().any(|m| m.provider.as_str() == "openai"),
        "unconfigured providers must be filtered out (openai)"
    );
    assert!(
        !catalog.iter().any(|m| m.provider.as_str() == "anthropic"),
        "unconfigured providers must be filtered out (anthropic)"
    );
}

/// Selecting a model from a DIFFERENT provider swaps the session's owning provider in place (Pi
/// model+provider switch). The active model becomes the together model AND the currently-installed
/// provider's catalog is now together's, not faux's.
#[tokio::test]
async fn selecting_a_different_provider_swaps_the_session_provider() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = two_model_faux();
    let auth = Arc::new(AuthStore::at(fx.agent_dir.join("auth.json")));
    auth.set_runtime_api_key(ProviderId::from("together"), "sk-together-test".to_string());
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-1".to_string());
    let session = SessionBuilder::new(provider, cfg)
        .auth(auth)
        .provider_resolver(Arc::new(RegistryResolver))
        .build()
        .await
        .unwrap();

    // Starts on the injected faux provider.
    assert_eq!(session.model().expect("session must have a resolved model").provider.as_str(), "faux");
    assert!(session.model_catalog().iter().all(|m| m.provider.as_str() == "faux"));

    // Target a real together model (the fully-qualified `provider/id` the selector confirms).
    let target = session
        .available_model_catalog()
        .into_iter()
        .find(|m| m.provider.as_str() == "together")
        .expect("a together model is available");
    let pattern = format!("{}/{}", target.provider.as_str(), target.id.as_str());

    let new_ref = session.set_model(&pattern).await.expect("cross-provider set_model succeeds");
    assert_eq!(new_ref.provider.as_str(), "together", "active model switched to together");
    assert_eq!(session.model().expect("session must have a resolved model").provider.as_str(), "together");
    assert_eq!(session.model().expect("session must have a resolved model").model.as_str(), target.id.as_str());

    // The injected provider was swapped: the current catalog is together's, faux is gone.
    let catalog = session.model_catalog();
    assert!(
        catalog.iter().all(|m| m.provider.as_str() == "together"),
        "the swapped provider's catalog is together's"
    );
    assert!(
        !catalog.iter().any(|m| m.provider.as_str() == "faux"),
        "the faux catalog is no longer the current provider"
    );
}

/// L4 gap #4: a guest `pi.registerProvider()` routed through the assembled session's extension host
/// must reach the shared model registry so the registered model is SELECTABLE (in
/// `available_model_catalog()`) and its owning provider is INSTALLED on a matching `set_model` — Pi
/// `bindCore` → `ModelRegistry.registerProvider` folds the models into the one registry
/// `getAvailable`/`find`/`setModel` all read (model-registry.ts:917-940). No provider resolver is
/// wired: a guest provider is a realized `Provider` the session installs DIRECTLY.
#[tokio::test]
async fn guest_registered_provider_is_selectable_and_installed() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = two_model_faux();
    let auth = Arc::new(AuthStore::at(fx.agent_dir.join("auth.json")));
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-1".to_string());
    let session = SessionBuilder::new(provider, cfg).auth(auth).build().await.unwrap();

    // Before registration, the guest model is absent.
    assert!(
        !session.available_model_catalog().iter().any(|m| m.provider.as_str() == "acme"),
        "no guest provider registered yet"
    );

    // A guest extension registers a custom provider + model (Pi `pi.registerProvider`). This routes
    // through the SAME extension host + bound sink the builder wired, so it upserts live.
    session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("acme-ext"),
            "acme",
            serde_json::json!({
                "name": "Acme",
                "baseUrl": "https://acme.test/v1",
                "api": "openai-completions",
                "apiKey": "sk-acme-123",
                "models": [{
                    "id": "acme-fast",
                    "name": "Acme Fast",
                    "contextWindow": 64000,
                    "maxTokens": 4096,
                }],
            }),
        )
        .expect("guest registerProvider succeeds");

    // SELECTABLE: the registered model now appears in the cross-provider selector catalog.
    let target = session
        .available_model_catalog()
        .into_iter()
        .find(|m| m.provider.as_str() == "acme" && m.id.as_str() == "acme-fast")
        .expect("the guest-registered model is selectable");
    assert_eq!(target.base_url, "https://acme.test/v1");
    assert_eq!(target.context_window, 64000);

    // Starts on the injected faux provider.
    assert_eq!(session.model().expect("session must have a resolved model").provider.as_str(), "faux");

    // set_model resolves the guest model AND installs the guest provider in place (no resolver seam
    // needed — the guest provider is a realized `Provider`).
    let new_ref = session.set_model("acme/acme-fast").await.expect("guest set_model succeeds");
    assert_eq!(new_ref.provider.as_str(), "acme");
    assert_eq!(session.model().expect("session must have a resolved model").provider.as_str(), "acme");
    assert_eq!(session.model().expect("session must have a resolved model").model.as_str(), "acme-fast");

    // STREAMABLE: the installed provider is the guest one, exposing the registered model in its
    // catalog (what `ProviderStreamFn` resolves against when the agent loop streams).
    let catalog = session.model_catalog();
    assert!(
        catalog.iter().any(|m| m.provider.as_str() == "acme" && m.id.as_str() == "acme-fast"),
        "the installed (guest) provider exposes the registered model"
    );
    assert!(
        catalog.iter().all(|m| m.provider.as_str() == "acme"),
        "the swapped provider's catalog is the guest provider's"
    );
}

/// Without a resolver seam, a cross-provider select cannot silently stream against the wrong
/// provider: it surfaces a clear error (the same guard the offline/one-shot builds rely on).
#[tokio::test]
async fn cross_provider_select_without_resolver_errors() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = two_model_faux();
    let auth = Arc::new(AuthStore::at(fx.agent_dir.join("auth.json")));
    auth.set_runtime_api_key(ProviderId::from("together"), "sk-together-test".to_string());
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-1".to_string());
    // No `.provider_resolver(...)` wired.
    let session = SessionBuilder::new(provider, cfg).auth(auth).build().await.unwrap();

    let target = session
        .available_model_catalog()
        .into_iter()
        .find(|m| m.provider.as_str() == "together")
        .expect("a together model is available");
    let pattern = format!("{}/{}", target.provider.as_str(), target.id.as_str());
    assert!(
        session.set_model(&pattern).await.is_err(),
        "a cross-provider select with no resolver must error, not mis-stream"
    );
    // The active provider is unchanged.
    assert_eq!(session.model().expect("session must have a resolved model").provider.as_str(), "faux");
}
