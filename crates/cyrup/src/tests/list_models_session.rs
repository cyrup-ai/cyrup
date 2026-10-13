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

// ============================================================ SEAM-139: the availability warning

/// A live provider whose auth strategy's `check` always errors.
struct FailingCheck;

#[async_trait::async_trait]
impl cyrup_provider::auth::ApiKeyAuth for FailingCheck {
    fn name(&self) -> &str {
        "failing"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        _ctx: &dyn cyrup_provider::AuthContext,
        _cred: Option<&cyrup_provider::Credential>,
    ) -> Result<Option<cyrup_provider::collection::AuthCheck>, cyrup_provider::AuthError> {
        Err(cyrup_provider::AuthError::api_key(
            cyrup_sdk::core::ProviderId::from("probe-llm"),
            "server probe refused",
        ))
    }
    async fn resolve(
        &self,
        _model: &cyrup_provider::Model,
        _ctx: &dyn cyrup_provider::AuthContext,
        _cred: Option<&cyrup_provider::Credential>,
    ) -> Result<Option<cyrup_provider::AuthResult>, cyrup_provider::AuthError> {
        Ok(None)
    }
}

struct ProbeProvider {
    id: cyrup_sdk::core::ProviderId,
    auth: cyrup_provider::auth::ProviderAuth,
    models: Vec<cyrup_provider::Model>,
}

#[async_trait::async_trait]
impl Provider for ProbeProvider {
    fn id(&self) -> &cyrup_sdk::core::ProviderId {
        &self.id
    }
    fn models(&self) -> &[cyrup_provider::Model] {
        &self.models
    }
    fn provider_auth(&self) -> Option<&cyrup_provider::auth::ProviderAuth> {
        Some(&self.auth)
    }
    fn stream(
        &self,
        _model: &cyrup_provider::Model,
        _context: &cyrup_provider::Context,
        _options: &cyrup_provider::StreamOptions,
    ) -> cyrup_sdk::core::EventStream<cyrup_provider::StreamEvent> {
        Box::pin(futures::stream::empty())
    }
}

struct ProbeExt(Arc<dyn Provider>);

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for ProbeExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("probe-ext")
    }
    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.register_provider_live("probe-llm", Arc::clone(&self.0));
        Ok(())
    }
    async fn on_event(
        &self,
        _ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        cyrup_ext::HookOutcome::Noop
    }
}

/// SEAM-139 Verify, on the bin's one visible surface for it: pi's `--list-models` opens with
/// `getError()` on stderr (`cli/list-models.ts:34-37` @v1.1.0), so a live provider whose strategy
/// errors yields `Warning: errors loading models.json:\nAvailability refresh: …` naming the
/// provider and the strategy's reason — where it used to vanish from the listing in silence.
#[tokio::test]
async fn list_models_warns_about_a_failing_live_auth_check() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let auth =
        Arc::new(AuthStore::at(agent_dir.join("auth.json")).with_ambient_env(HashMap::new()));
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    config.no_extensions = true;
    let reg = cyrup_ext::provider::ProviderRegistration {
        id: "probe-llm".to_string(),
        config: serde_json::from_value(serde_json::json!({
            "name": "probe-llm",
            "baseUrl": "http://127.0.0.1:9/v1",
            "api": "openai-completions",
            "models": [{ "id": "tiny", "name": "tiny" }],
        }))
        .unwrap(),
        resolved_api_key: None,
    };
    let live: Arc<dyn Provider> = Arc::new(ProbeProvider {
        id: cyrup_sdk::core::ProviderId::from("probe-llm"),
        auth: cyrup_provider::auth::ProviderAuth::with_api_key(Arc::new(FailingCheck)),
        models: reg.build_models(),
    });
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, config)
        .auth(auth)
        .with_native_extension(Arc::new(ProbeExt(live)) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();

    let models = session.configured_model_catalog();
    assert!(
        !models.iter().any(|m| m.provider.as_str() == "probe-llm"),
        "the failing provider is not listed"
    );
    let warning = crate::actions::list_models_warning(&session).expect("the failure is shown");
    assert!(
        warning.starts_with("Warning: errors loading models.json:\nAvailability refresh: "),
        "{warning}"
    );
    assert!(warning.contains("probe-llm"), "{warning}");
    assert!(warning.contains("server probe refused"), "{warning}");
}

/// A real session whose live provider `probe-llm`'s auth check always fails (the fixture above).
async fn failing_probe_session() -> (tempfile::TempDir, Arc<AgentSession>) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let auth =
        Arc::new(AuthStore::at(agent_dir.join("auth.json")).with_ambient_env(HashMap::new()));
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    config.no_extensions = true;
    let reg = cyrup_ext::provider::ProviderRegistration {
        id: "probe-llm".to_string(),
        config: serde_json::from_value(serde_json::json!({
            "name": "probe-llm",
            "baseUrl": "http://127.0.0.1:9/v1",
            "api": "openai-completions",
            "models": [{ "id": "tiny", "name": "tiny" }],
        }))
        .unwrap(),
        resolved_api_key: None,
    };
    let live: Arc<dyn Provider> = Arc::new(ProbeProvider {
        id: cyrup_sdk::core::ProviderId::from("probe-llm"),
        auth: cyrup_provider::auth::ProviderAuth::with_api_key(Arc::new(FailingCheck)),
        models: reg.build_models(),
    });
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, config)
        .auth(auth)
        .with_native_extension(Arc::new(ProbeExt(live)) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();
    (tmp, Arc::new(session))
}

/// **SEAM-150, the startup consumer.** pi's `run()` follows the startup diagnostics and the
/// migrated-credential notice with `const modelsJsonError = this.session.modelRuntime.getError();
/// if (modelsJsonError) this.showError(`models.json error: ${modelsJsonError}`)`, then the
/// model-fallback warning (`interactive-mode.ts:1203-1218` @f1b2e77f5). With a live provider whose
/// check fails, the boot transcript carries `Error: models.json error: Availability refresh: …`
/// (the `Error: ` is `showError`'s, `:4623-4626`) in exactly that slot.
///
/// RED with the boot call removed (HEAD): `no models.json error line in [...]`; red with it moved
/// after the fallback warning (the order assertion); red when `models_json_error` skips its catalog
/// read (no check has run on this session yet, so `getError()` is still empty).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_boot_transcript_shows_a_failing_live_auth_check_as_a_models_json_error() {
    use cyrup_tui::{App, Entry, UiTheme};
    let (_tmp, session) = failing_probe_session().await;
    let mut app = App::new(ratatui::backend::TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    crate::interactive::render_boot_transcript(
        &mut app,
        &session,
        &[],
        &["anthropic".to_string()],
        Some("No models available.".to_string()),
        None,
    )
    .await;
    let got: Vec<String> = app
        .state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Warning(w) => Some(format!("warning|{w}")),
            Entry::Error(e) => Some(format!("error|{e}")),
            _ => None,
        })
        .collect();
    let error = got
        .iter()
        .position(|l| l.starts_with("error|Error: models.json error: Availability refresh: "))
        .unwrap_or_else(|| panic!("no models.json error line in {got:#?}"));
    assert!(got[error].contains("probe-llm"), "{got:#?}");
    assert!(got[error].contains("server probe refused"), "{got:#?}");
    let migrated = got
        .iter()
        .position(|l| l == "warning|Warning: Migrated credentials to auth.json: anthropic")
        .unwrap();
    let fallback = got
        .iter()
        .position(|l| l == "warning|Warning: No models available.")
        .unwrap();
    assert!(migrated < error && error < fallback, "pi's order: {got:#?}");
}

/// SEAM-150 startup control: a session with no `getError()` boots with no `models.json error:`
/// line. Non-regression guard: green at HEAD; red under the mutation "`models_json_error` always
/// answers a line".
#[tokio::test]
async fn a_clean_boot_transcript_has_no_models_json_error() {
    use cyrup_tui::{App, Entry, UiTheme};
    let fx = fixture(&[]).await;
    let session = Arc::new(fx.session);
    let mut app = App::new(ratatui::backend::TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    crate::interactive::render_boot_transcript(&mut app, &session, &[], &[], None, None).await;
    assert!(
        !app.state()
            .transcript
            .pending()
            .iter()
            .any(|e| matches!(e, Entry::Error(t) if t.contains("models.json error"))),
        "{:#?}",
        app.state().transcript.pending()
    );
}
