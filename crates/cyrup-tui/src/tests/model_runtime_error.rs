//! SEAM-150 — pi's `ModelRuntime.getError()` in the interactive UI.
//!
//! pi @f1b2e77f5 reads `this.modelRuntime.getError()` in three interactive places, and cyrup's port
//! of it is `AgentSession::model_runtime_error` (SEAM-139): the `models.json` problems plus
//! `Availability refresh: <message>` for a live provider whose auth check failed.
//!
//! 1. the `/model` picker after a catalog refresh with no per-provider errors
//!    (`modes/interactive/components/model-selector.ts:202`), drawn in red (`:341-345`) and
//!    suppressing `Model catalogs refreshed.` (`:203-206`) —
//!    [`the_model_picker_shows_a_failing_live_auth_check_in_red_instead_of_refreshed`];
//! 2. startup (`interactive-mode.ts:1211-1214`) — in `crates/cyrup` (`tests/list_models_session.rs`),
//!    where the boot transcript is assembled;
//! 3. `/reload` (`:6501-6504`) — [`reload_shows_a_failing_live_auth_check_as_a_models_json_error`].
//!
//! The failing provider is a REAL live provider registered by a native extension, whose strategy's
//! `check` errors — the SEAM-139 fixture (`crates/cyrup/src/tests/list_models_session.rs`). Nothing
//! opens a socket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::harness::*;
use crate::{App, AppCommand, SelectorKind, UiTheme};
use cyrup_config::AuthStore;
use cyrup_core::{EventStream, ExtensionId, ProviderId};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{Context, Model, Provider, StreamEvent, StreamOptions};
use cyrup_session_svc::{
    AgentSession, AgentSessionRuntime, SessionBuilder, SessionConfig, SessionFactory, SessionTarget,
};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

const PROBE: &str = "probe-llm";
const REFUSED: &str = "server probe refused";

/// A live provider's auth strategy whose `check` errors (`fails`) or answers "not configured".
struct ProbeCheck {
    fails: bool,
}

#[async_trait::async_trait]
impl cyrup_provider::auth::ApiKeyAuth for ProbeCheck {
    fn name(&self) -> &str {
        "probe"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        _ctx: &dyn cyrup_provider::AuthContext,
        _cred: Option<&cyrup_provider::Credential>,
    ) -> Result<Option<cyrup_provider::collection::AuthCheck>, cyrup_provider::AuthError> {
        if self.fails {
            Err(cyrup_provider::AuthError::api_key(
                ProviderId::from(PROBE),
                REFUSED,
            ))
        } else {
            Ok(None)
        }
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn cyrup_provider::AuthContext,
        _cred: Option<&cyrup_provider::Credential>,
    ) -> Result<Option<cyrup_provider::AuthResult>, cyrup_provider::AuthError> {
        Ok(None)
    }
}

struct ProbeProvider {
    id: ProviderId,
    auth: cyrup_provider::auth::ProviderAuth,
    models: Vec<Model>,
}

#[async_trait::async_trait]
impl Provider for ProbeProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    fn provider_auth(&self) -> Option<&cyrup_provider::auth::ProviderAuth> {
        Some(&self.auth)
    }
    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        Box::pin(futures::stream::empty())
    }
}

/// The native extension that registers the probe as a LIVE provider.
struct ProbeExt {
    fails: bool,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for ProbeExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("probe-ext")
    }
    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        // A model of its own, or no catalog read would ever consult the provider's check.
        let registration = cyrup_ext::provider::ProviderRegistration {
            id: PROBE.to_string(),
            config: serde_json::from_value(serde_json::json!({
                "name": PROBE,
                "baseUrl": "http://127.0.0.1:9/v1",
                "api": "openai-completions",
                "models": [{ "id": "tiny", "name": "tiny" }],
            }))
            .unwrap(),
            resolved_api_key: None,
        };
        let provider: Arc<dyn Provider> = Arc::new(ProbeProvider {
            id: ProviderId::from(PROBE),
            auth: cyrup_provider::auth::ProviderAuth::with_api_key(Arc::new(ProbeCheck {
                fails: self.fails,
            })),
            models: registration.build_models(),
        });
        api.register_provider_live(PROBE, provider);
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

struct Dirs {
    _tmp: TempDir,
    config: SessionConfig,
    auth: Arc<AuthStore>,
}

/// No credentials anywhere (the ambient env tier pinned empty), so the `/model` refresh has no
/// credential-gated provider to fetch and settles clean, with no network.
fn dirs() -> Dirs {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir.clone());
    config.home = home;
    config.trust_override = Some(true);
    config.no_extensions = true;
    let auth =
        Arc::new(AuthStore::at(agent_dir.join("auth.json")).with_ambient_env(HashMap::new()));
    Dirs {
        _tmp: tmp,
        config,
        auth,
    }
}

async fn session(dirs: &Dirs, fails: bool) -> Arc<AgentSession> {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    Arc::new(
        SessionBuilder::new(provider, dirs.config.clone())
            .auth(Arc::clone(&dirs.auth))
            .with_native_extension(
                Arc::new(ProbeExt { fails }) as Arc<dyn cyrup_ext::NativeExtension>
            )
            .build()
            .await
            .unwrap(),
    )
}

/// Open `/model` through the real command arm and settle the refresh it starts, as the run loop's
/// `model_refresh_rx` arm does; then draw.
async fn open_model_picker_and_settle(session: &Arc<AgentSession>) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(140, 40), UiTheme::dark()).unwrap();
    let mut rx = app.install_model_refresh_channel();
    app.execute_command(AppCommand::ModelCommand(None), session, None)
        .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Model));
    let msg = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the refresh settles")
        .expect("channel open");
    assert!(
        msg.result.is_clean(),
        "fixture: the refresh itself must be clean so pi's `getError()` branch is the one taken: {:?}",
        msg.result
    );
    app.apply_model_refresh(session, msg);
    app.draw().unwrap();
    app
}

/// **SEAM-150 Verify 1.** With a live provider whose auth check fails, a clean `/model` refresh
/// shows `Availability refresh: …` in red where the picker's error block goes, and NOT
/// `Model catalogs refreshed.` — pi's `this.errorMessage = this.modelRuntime.getError(); if
/// (!this.errorMessage) { … "Model catalogs refreshed." }` (`model-selector.ts:202-206`).
///
/// RED under the HEAD read restored (`Some(startup_diagnostics.models.join("\n\n"))`): the picker
/// said `Model catalogs refreshed.` and had no `Availability refresh:` row. NOT distinguished: reading
/// `model_runtime_error()` before `model_entries` stays green, because opening the picker already
/// read the catalog (and ran the check); the order in `apply_model_refresh` is kept as pi's
/// (`getError()` after the refresh) without a test that can tell.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_model_picker_shows_a_failing_live_auth_check_in_red_instead_of_refreshed() {
    let dirs = dirs();
    let session = session(&dirs, true).await;
    let app = open_model_picker_and_settle(&session).await;

    let text = buf_text(&app);
    assert!(
        !text.contains("Model catalogs refreshed."),
        "the success row is suppressed:\n{text}"
    );
    let (y, row) = row_with(&app, "Availability refresh:");
    assert!(row.contains(PROBE), "{row}");
    assert!(text.contains(REFUSED), "{text}");
    assert_eq!(
        fg_at(&app, col_of(&row, "Availability refresh:"), y),
        UiTheme::dark().error_style().fg,
        "drawn in `theme.fg(\"error\", …)`"
    );
}

/// SEAM-150 control — the same picker on a session whose live provider's check ANSWERS (here "not
/// configured") still says `Model catalogs refreshed.`, with no error row.
///
/// Non-regression guard: green at HEAD. Red under the mutation "`runtime_error` is always
/// `Some(\"x\")`" (the success row is replaced by an error row).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_model_picker_still_says_refreshed_when_there_is_no_error() {
    let dirs = dirs();
    let session = session(&dirs, false).await;
    let app = open_model_picker_and_settle(&session).await;

    let text = buf_text(&app);
    assert!(text.contains("Model catalogs refreshed."), "{text}");
    assert!(!text.contains("Availability refresh:"), "{text}");
}

/// Drive `/reload` exactly as the run loop does, then the re-bind the generation watch performs;
/// return the committed scrollback (the `reload_implicit_trust.rs` driver).
async fn reload(app: &mut App<TestBackend>, rt: &Arc<AgentSessionRuntime>) -> String {
    let session = rt.session().await;
    let generation = rt.generation().await;
    app.execute_command(AppCommand::Reload, &session, Some(rt))
        .await;
    assert_eq!(rt.generation().await, generation + 1, "/reload rebuilds");
    app.draw().unwrap();
    app.rebind_session();
    app.draw().unwrap();
    app.scrollback_text()
}

async fn runtime(dirs: &Dirs, fails: bool) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let factory = Arc::new(
        SessionFactory::new(provider, dirs.config.clone())
            .auth(Arc::clone(&dirs.auth))
            .with_native_extension(
                Arc::new(ProbeExt { fails }) as Arc<dyn cyrup_ext::NativeExtension>
            ),
    );
    AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap()
}

/// **SEAM-150 Verify 2.** `/reload` on a session whose live provider's check fails prints pi's
/// `showError(`models.json error: ${getError()}`)` (`interactive-mode.ts:6501-6504`), framed
/// `Error: ` by `showError` (`:4623-4626`) — read off the REBUILT session.
///
/// RED under the HEAD arm restored (`error: None`): `no \`Error: models.json error:\` line in:
/// Reloaded keybindings, …`; also red when `apply_lifecycle_outcome` drops the error, and when
/// `models_json_error` skips its catalog read (the rebuilt session has run no check yet). Reading
/// the OUTGOING session instead of `rt.session()` is not distinguished (both carry the probe).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_shows_a_failing_live_auth_check_as_a_models_json_error() {
    let dirs = dirs();
    let rt = runtime(&dirs, true).await;
    let mut app = App::new(TestBackend::new(200, 30), UiTheme::dark()).unwrap();

    let out = reload(&mut app, &rt).await;

    let line = out
        .lines()
        .find(|l| l.contains("Error: models.json error: Availability refresh: "))
        .unwrap_or_else(|| panic!("no `Error: models.json error:` line in:\n{out}"));
    assert!(line.contains(PROBE), "{line}");
    assert!(out.contains(REFUSED), "{out}");
    assert!(
        out.contains(
            "Reloaded keybindings, extensions, skills, prompts, themes, and context files"
        ),
        "the reload itself still reports:\n{out}"
    );
}

/// SEAM-150 control for `/reload`: no failing check → no `models.json error:` line.
///
/// Non-regression guard: green at HEAD. Red under the mutation "`models_json_error` always
/// answers a line".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_without_a_runtime_error_prints_no_models_json_error() {
    let dirs = dirs();
    let rt = runtime(&dirs, false).await;
    let mut app = App::new(TestBackend::new(200, 30), UiTheme::dark()).unwrap();

    let out = reload(&mut app, &rt).await;

    assert!(!out.contains("models.json error"), "{out}");
}
