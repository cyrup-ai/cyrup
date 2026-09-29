//! **CFG-020b** — the three builtin argument completers read the session LIVE, per keystroke.
//!
//! pi's `createBaseAutocompleteProvider` assigns `getArgumentCompletions` closures that read
//! `this.session` INSIDE the callback (`modes/interactive/interactive-mode.ts:689-736` @v0.87.1):
//! `this.session.scopedModels` / `this.session.modelRuntime.getAvailableSnapshot()` at `:689-691`,
//! `this.getLoginProviderOptions()` at `:729`, `this.session.getAvailableThinkingLevels()` at
//! `:715`. Nothing is captured by value, so a set that changes mid-session is offered on the very
//! next keystroke — including one that changes by a path no refresh hook watches.
//!
//! cyrup used to snapshot those three into `ArgumentSources` at four fixed points, which lost that
//! guarantee for exactly the case this file pins: an extension registering a provider mid-session.
//! The sources are now [`crate::LiveSource::Live`] callbacks over the session, read inside
//! `Autocomplete::compute`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_core::ExtensionId;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{Model, Provider};
use cyrup_session_svc::{AgentSession, ScopedModel, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use serde_json::json;
use tempfile::TempDir;

use super::harness::key;
use crate::{App, UiTheme};

/// A session over the faux provider — no socket, no credentials.
async fn session_fixture() -> (TempDir, Arc<AgentSession>) {
    let tmp = TempDir::new().expect("temp dir");
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).expect("cwd");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, config)
        .build()
        .await
        .expect("session");
    (tmp, Arc::new(session))
}

fn type_text(app: &mut App<TestBackend>, text: &str) {
    for c in text.chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
}

/// The value the argument popup would insert, if one is open.
fn selected_completion(app: &App<TestBackend>) -> Option<String> {
    app.state()
        .editor
        .autocomplete()
        .and_then(|ac| ac.selected().map(|c| c.value.clone()))
}

/// A model that no built-in catalog contains, so a hit can only come from the live read.
fn guest_model_config() -> serde_json::Value {
    json!({
        "name": "Zzq",
        "baseUrl": "https://zzq.test/v1",
        "api": "openai-completions",
        "apiKey": "sk-zzq-123",
        "models": [{
            "id": "zzqturbo-9",
            "name": "Zzq Turbo 9",
            "contextWindow": 64000,
            "maxTokens": 4096,
        }],
    })
}

/// **The red-without test.** A guest provider registered mid-session — the exact case the deleted
/// design note said "is not offered until the next refresh" — is offered on the next keystroke,
/// with NO further `refresh_argument_sources` call.
///
/// Snapshotting sources makes the second assertion fail: the popup stays closed because the
/// candidate list is the one captured before the registration.
#[tokio::test]
async fn guest_provider_registered_mid_session_is_offered_without_a_refresh() {
    let (_tmp, session) = session_fixture().await;
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).expect("app");
    // The ONE install, at boot — pi's single `createBaseAutocompleteProvider`.
    app.refresh_argument_sources(&session);

    // Before the registration the id matches nothing.
    type_text(&mut app, "/model zzqturbo");
    assert_eq!(
        selected_completion(&app),
        None,
        "no provider offers `zzqturbo-9` yet"
    );

    // A guest extension registers a provider — `pi.registerProvider()`, flushed into the session's
    // shared model-registry sink (`ModelRegistry.registerProvider`, model-registry.ts:917-940).
    let ext = cyrup_ext::registry::ExtensionRegistry::new();
    ext.register_provider(ExtensionId::from("zzq-ext"), "zzq", guest_model_config())
        .expect("registration");
    let sink: Arc<dyn cyrup_ext::provider::ModelRegistrySink> =
        session.services().guest_providers.clone();
    ext.bind_model_registry(sink).expect("bind");
    assert!(
        session
            .available_model_catalog()
            .iter()
            .any(|m| m.id.as_str() == "zzqturbo-9"),
        "the session itself now resolves the guest model"
    );

    // One more keystroke, no refresh call in between.
    app.handle_input(&key(KeyCode::Char('-')));
    assert_eq!(
        selected_completion(&app).as_deref(),
        Some("zzq/zzqturbo-9"),
        "the `/model` completer read the catalog live, as pi's closure does"
    );
}

/// The same liveness for `scopedModels` (`interactive-mode.ts:689-690`): setting the cycle set
/// mid-session narrows `/model` on the next keystroke, without a refresh.
#[tokio::test]
async fn scoped_models_set_mid_session_narrow_the_popup_without_a_refresh() {
    let (_tmp, session) = session_fixture().await;
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).expect("app");
    app.refresh_argument_sources(&session);

    let mut scoped: Model = FauxProvider::new().model().clone();
    scoped.provider = cyrup_core::ProviderId::from("zzq");
    scoped.id = cyrup_core::ModelId::from("zzqturbo-9");
    session.set_scoped_models(vec![ScopedModel {
        model: scoped,
        thinking_level: None,
    }]);

    type_text(&mut app, "/model zzqturbo");
    assert_eq!(
        selected_completion(&app).as_deref(),
        Some("zzq/zzqturbo-9"),
        "the scoped set is read inside the completer, not captured before it existed"
    );
}

/// The cost the deleted note claimed made per-keystroke reading impossible: `available_model_catalog()`
/// running `has_configured_auth` PER MODEL. CFG-020a replaced that with a cached registry snapshot
/// plus one auth evaluation per distinct PROVIDER, so a keystroke's read is bounded well under the
/// frame budget. Asserted loosely — this pins an order of magnitude, not a stopwatch.
#[tokio::test]
async fn live_model_source_is_cheap_enough_per_keystroke() {
    let (_tmp, session) = session_fixture().await;
    let catalog = session.available_model_catalog();
    // Warm the registry snapshot exactly as the first keystroke would.
    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _ = session.available_model_catalog();
    }
    let per_read = start.elapsed() / 100;
    // Printed, not just asserted: this is the number the design note that used to sit on
    // `refresh_argument_sources` never had. `cargo test -p cyrup-tui --lib
    // live_model_source_is_cheap_enough_per_keystroke -- --nocapture`.
    println!(
        "CFG-020b: {} models, {per_read:?} per live `/model` completer read",
        catalog.len()
    );
    assert!(
        per_read < std::time::Duration::from_millis(10),
        "a per-keystroke catalog read over {} models took {per_read:?}",
        catalog.len()
    );
}
