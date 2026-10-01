//! SEAM-135 — `--list-models` renders the BUILT session's catalog.
//!
//! pi runs the listing after runtime creation (`main.ts:866-871` @v0.87.1) and reads
//! `modelRuntime.getAvailable()` (`cli/list-models.ts:35`), so a model an extension registered with
//! `registerProvider` is a row. cyrup used to exit before the runtime existed and list the compiled
//! catalog, which cannot see guest providers. These drive the same two functions the launch path
//! composes — `AgentSession::configured_model_catalog` and `actions::render_model_listing` — over a
//! real `SessionBuilder` session.
//!
//! **Hermetic.** The provider is the scripted faux double, and every `AuthStore` here is built with an
//! empty ambient environment, so no host credential can move a row count.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::Arc;

use cyrup_config::{AuthStore, ModelFile};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_sdk::core::ExtensionId;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};

use crate::actions::render_model_listing;

struct Fx {
    _tmp: tempfile::TempDir,
    auth: Arc<AuthStore>,
    session: AgentSession,
}

/// A faux-provider session whose `auth.json` holds `stored` api keys.
async fn fixture(stored: &[&str]) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let creds: serde_json::Map<String, serde_json::Value> = stored
        .iter()
        .map(|id| {
            (
                (*id).to_string(),
                serde_json::json!({"type": "api_key", "key": format!("sk-{id}")}),
            )
        })
        .collect();
    std::fs::write(
        agent_dir.join("auth.json"),
        serde_json::to_string(&creds).unwrap(),
    )
    .unwrap();
    let auth =
        Arc::new(AuthStore::at(agent_dir.join("auth.json")).with_ambient_env(HashMap::new()));
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, config)
        .auth(Arc::clone(&auth))
        .build()
        .await
        .unwrap();
    Fx {
        _tmp: tmp,
        auth,
        session,
    }
}

/// The red test. A guest `registerProvider` carries its own credentials, so its model is available —
/// and only the session can know about it.
#[tokio::test]
async fn an_extension_registered_provider_is_a_listed_row() {
    let fx = fixture(&[]).await;
    let before = render_model_listing(&fx.session.configured_model_catalog(), "");
    assert!(
        !before.contains("acme"),
        "nothing registered yet:\n{before}"
    );

    fx.session
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

    let listing = render_model_listing(&fx.session.configured_model_catalog(), "acme");
    let row = listing
        .lines()
        .find(|l| l.starts_with("acme"))
        .unwrap_or_else(|| panic!("no acme row in:\n{listing}"));
    assert!(row.contains("acme-fast"), "{row}");
    assert!(row.contains("64K"), "context column: {row}");
}

/// The regression the session's different predicate invites: for a stored-credential provider the
/// session listing must be byte-identical to what the pre-SEAM-135 closure
/// (`provider::available_models` over `provider_is_configured`) printed.
#[tokio::test]
async fn a_stored_credential_provider_lists_the_same_rows_as_the_old_closure() {
    let fx = fixture(&["groq"]).await;
    let models_json = ModelFile::default();
    let old = render_model_listing(
        &crate::provider::available_models(&models_json, &|m| {
            cyrup_config::provider_is_configured(&fx.auth, &models_json, &m.provider, None)
        }),
        "",
    );
    let new = render_model_listing(&fx.session.configured_model_catalog(), "");
    assert!(
        old.contains("groq"),
        "the fixture credential lists groq:\n{old}"
    );
    assert_eq!(new, old);
}

/// The installed provider's own catalog is not an entitlement. `available_model_catalog` (the
/// `/model` selector's) keeps it selectable; `getAvailable()` does not, and neither may the listing.
/// This is what `cyrup --provider anthropic --list-models` with no credential runs into.
#[tokio::test]
async fn the_installed_providers_catalog_is_not_listed_without_auth() {
    let fx = fixture(&[]).await;
    let selector = fx.session.available_model_catalog();
    assert!(
        selector.iter().any(|m| m.provider.as_str() == "faux"),
        "the selector keeps the installed provider's models"
    );
    let listing = render_model_listing(&fx.session.configured_model_catalog(), "");
    assert_eq!(
        listing,
        format!("{}\n", crate::format_no_models_available_message())
    );
}
