//! A credential `/login` stored in `auth.json` must reach the provider the binary streams with.
//!
//! Pi hands every provider the session's `RuntimeCredentials` (`ModelRuntime`), so a login is
//! usable on the next request. cyrup built its providers over an empty `InMemoryCredentialStore`
//! (`composed_registry` with no `--api-key`), so a stored OAuth or API-key credential was never
//! read: every prompt failed with "provider 'anthropic' is not configured (no credential or env
//! key)" while `cyrup auth check` (which reads `auth.json`) answered `ready`.
//!
//! `anthropic` is pointed at a port nothing listens on, so a request that gets past auth fails at
//! connect time and never leaves the machine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use crate::provider::{BuiltinProviderResolver, select_provider};
use cyrup_config::{AuthStore, Credential, ModelFile, load_models_file};
use cyrup_provider::stream::collect_message;
use cyrup_provider::{Context, Provider, StreamOptions};
use cyrup_sdk::core::ProviderId;
use cyrup_session_svc::ProviderResolver;

const NOT_CONFIGURED: &str = "is not configured";

fn closed_port_models(dir: &std::path::Path) -> ModelFile {
    let path = dir.join("models.json");
    std::fs::write(
        &path,
        r#"{"providers":{"anthropic":{"baseUrl":"http://127.0.0.1:9"}}}"#,
    )
    .unwrap();
    load_models_file(&path).expect("models.json parses")
}

/// Whether the ambient environment already configures `anthropic`, which would make the
/// "before" half of a test pass for the wrong reason.
fn env_configures_anthropic() -> bool {
    [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_OAUTH_TOKEN",
    ]
    .iter()
    .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

async fn stream_error(provider: &Arc<dyn Provider>) -> String {
    let model = provider.models().first().cloned().expect("a catalog model");
    let msg =
        collect_message(provider.stream(&model, &Context::default(), &StreamOptions::default()))
            .await;
    msg.error_message.unwrap_or_default()
}

/// The `/login` write: through the session's store, AFTER the provider was built.
async fn login(store: &AuthStore) {
    store
        .modify(&ProviderId::from("anthropic"), |_| async {
            Ok(Some(Credential::api_key("sk-stored")))
        })
        .await
        .expect("store the credential");
}

/// The launch path (`main.rs`): a provider built over the session's store streams with a
/// credential stored after it was built.
#[tokio::test]
async fn the_launched_provider_streams_with_a_login_made_after_it_was_built() {
    let dir = tempfile::tempdir().unwrap();
    let models = closed_port_models(dir.path());
    let store = Arc::new(AuthStore::at(dir.path().join("auth.json")));
    let provider = select_provider(
        Some("anthropic"),
        None,
        None,
        &models,
        Some(cyrup_config::login::runtime_credentials(store.clone())),
    )
    .expect("anthropic");

    if !env_configures_anthropic() {
        let before = stream_error(&provider).await;
        assert!(
            before.contains(NOT_CONFIGURED),
            "nothing stored yet: {before}"
        );
    }

    login(&store).await;
    let after = stream_error(&provider).await;
    assert!(
        !after.contains(NOT_CONFIGURED),
        "the stored credential must reach the provider: {after}"
    );
}

/// The in-session `/model` swap (`BuiltinProviderResolver`) builds its provider over the same store.
#[tokio::test]
async fn a_swapped_in_provider_streams_with_the_stored_credential() {
    let dir = tempfile::tempdir().unwrap();
    let models = closed_port_models(dir.path());
    let store = Arc::new(AuthStore::at(dir.path().join("auth.json")));
    login(&store).await;

    let resolver = BuiltinProviderResolver::new(
        Arc::new(models),
        cyrup_config::login::runtime_credentials(store),
    );
    let provider = resolver.resolve("anthropic").expect("anthropic");
    let error = stream_error(&provider).await;
    assert!(
        !error.contains(NOT_CONFIGURED),
        "the stored credential must reach the provider: {error}"
    );
}
