//! EXT-027 — launching on a provider an EXTENSION registers (`cyrup --model llama.cpp/<id>`), and the
//! startup refresh of the extension providers' catalogs.
//!
//! pi resolves `--provider`/`--model` after the extensions loaded and registered their providers
//! (`buildSessionOptions`, `main.ts:468-479`, after `createAgentSessionServices`,
//! `core/agent-session-services.ts:190-206` @v0.99.2-17), and starts ONE catalog refresh, over every
//! provider of its registry, in rpc mode after the runtime exists (`main.ts:931-936`) and in
//! interactive mode from `run()` (`interactive-mode.ts:1115-1126`), never in print/json mode and
//! never with `PI_OFFLINE` set. cyrup selects its provider before any extension is loaded, so the
//! launch selection defers an unknown provider id to the session builder, and the startup refresh of
//! the extension providers lives beside the one for the pi.dev catalogs.
//!
//! **No network.** The scripted provider's "network" is a phase flag; no socket is opened.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_config::ModelFile;
use cyrup_config::policy::NetworkPolicy;
use cyrup_config::trust::AppMode;
use cyrup_ext::provider::ModelRegistrySink;
use cyrup_provider::collection::{AuthCheck, AuthType};
use cyrup_provider::{
    ApiKeyAuth, AuthContext, AuthError, AuthResult, Context, Credential, Model, ModelAuth,
    Provider, ProviderAuth, ProviderError, RefreshModelsContext, StreamEvent, StreamOptions,
};
use cyrup_sdk::core::{EventStream, ProviderId};
use cyrup_session_svc::GuestProviderRegistry;

use crate::provider::{
    UnknownProvider, qualify_deferred_pattern, requested_provider_id, select_launch_provider,
    select_provider, spawn_extension_provider_refresh,
};

// ------------------------------------------------------------------------- launch selection

/// A provider nothing built in names, with a `--model`: not an error at selection, because an
/// extension may register it. The placeholder is the zero-model provider, and the caller is told it
/// is a placeholder.
#[test]
fn an_unknown_provider_with_a_model_is_left_to_the_session() {
    let (provider, deferred) =
        select_launch_provider(None, Some("llama.cpp/qwen3"), None, &ModelFile::default())
            .expect("the choice is deferred, not refused");

    assert!(deferred);
    assert!(provider.models().is_empty(), "a placeholder with no models");
}

/// The strict selection still refuses the same id, so the deferral is the launch path's own
/// decision and not a loosening of the provider lookup every other caller (`/model`'s provider swap,
/// embedders) relies on.
#[test]
fn the_strict_selection_still_refuses_an_unknown_provider() {
    let error = select_provider(None, Some("llama.cpp/qwen3"), None, &ModelFile::default())
        .err()
        .expect("llama.cpp is not a built-in provider")
        .to_string();

    assert!(error.contains("not a known provider"), "{error}");
}

/// The refusal the launch path defers is a type of its own, so it can never be confused with a
/// different selection failure (which must reach the user with its own message): the strict
/// selection returns it, and its text is the one the user sees when nothing registers the id.
#[test]
fn the_deferred_refusal_is_the_unknown_provider_error_alone() {
    let error = select_provider(None, Some("llama.cpp/qwen3"), None, &ModelFile::default())
        .err()
        .expect("llama.cpp is not a built-in provider");

    let unknown = error
        .downcast_ref::<UnknownProvider>()
        .expect("an unknown provider is reported as `UnknownProvider`");
    assert_eq!(unknown.id, "llama.cpp");
    assert!(
        unknown.available.contains("openai"),
        "{}",
        unknown.available
    );
    assert!(
        error
            .to_string()
            .starts_with("model targets provider 'llama.cpp'"),
        "{error}"
    );
    // A plain error is not this refusal, so the launch path would not defer it.
    assert!(!anyhow::anyhow!("something else").is::<UnknownProvider>());
}

/// A built-in provider is selected as ever and is not deferred.
#[test]
fn a_built_in_provider_is_selected_immediately() {
    let (provider, deferred) =
        select_launch_provider(None, Some("openai/gpt-4o"), None, &ModelFile::default()).unwrap();

    assert!(!deferred);
    assert_eq!(provider.id().as_str(), "openai");
}

/// An unknown `--provider` with no `--model` has nothing to resolve later, so the original
/// diagnostic stands.
#[test]
fn an_unknown_provider_without_a_model_keeps_its_error() {
    let error = select_launch_provider(Some("llama.cpp"), None, None, &ModelFile::default())
        .err()
        .expect("nothing to defer")
        .to_string();

    assert!(error.contains("not a known provider"), "{error}");
}

/// `--provider llama.cpp --model qwen3` reaches the session as `llama.cpp/qwen3`; a pattern that
/// already carries the prefix (any case) is left alone, and with no provider nothing is added.
#[test]
fn an_explicit_provider_becomes_the_prefix_of_a_deferred_pattern() {
    assert_eq!(
        qualify_deferred_pattern(Some("llama.cpp"), "qwen3"),
        "llama.cpp/qwen3"
    );
    assert_eq!(
        qualify_deferred_pattern(Some("llama.cpp"), "llama.cpp/qwen3"),
        "llama.cpp/qwen3"
    );
    assert_eq!(
        qualify_deferred_pattern(Some("Llama.cpp"), "llama.CPP/qwen3:high"),
        "llama.CPP/qwen3:high"
    );
    assert_eq!(qualify_deferred_pattern(None, "qwen3"), "qwen3");
    assert_eq!(qualify_deferred_pattern(Some(""), "qwen3"), "qwen3");
}

/// The id a launch names: `--provider` first, else the `--model` prefix (what `--api-key` attaches to
/// when the choice was deferred).
#[test]
fn the_requested_provider_is_the_flag_then_the_prefix() {
    assert_eq!(
        requested_provider_id(Some("llama.cpp"), Some("other/x")),
        Some("llama.cpp")
    );
    assert_eq!(
        requested_provider_id(None, Some("llama.cpp/qwen3")),
        Some("llama.cpp")
    );
    assert_eq!(requested_provider_id(None, Some("qwen3")), None);
    assert_eq!(requested_provider_id(None, None), None);
}

// ------------------------------------------------------------------------ startup refresh

/// An API-key strategy that always resolves, so the registry's network phase runs with no stored
/// credential (pi `resolveRefreshCredential`, `models.ts:608-635`).
struct AlwaysResolves;

#[async_trait::async_trait]
impl ApiKeyAuth for AlwaysResolves {
    fn name(&self) -> &str {
        "scripted"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        Ok(Some(AuthCheck {
            auth_type: AuthType::ApiKey,
            source: None,
        }))
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        Ok(Some(AuthResult {
            auth: ModelAuth {
                api_key: Some("k".to_string()),
                headers: None,
                base_url: None,
            },
            env: None,
            source: None,
        }))
    }
}

/// An extension provider whose `refresh_models` records each phase's `allow_network`.
struct Recording {
    id: ProviderId,
    auth: ProviderAuth,
    phases: Arc<Mutex<Vec<bool>>>,
}

#[async_trait::async_trait]
impl Provider for Recording {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &[]
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.auth)
    }
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        self.phases.lock().unwrap().push(ctx.allow_network);
        Some(Ok(()))
    }
    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        Box::pin(futures_empty())
    }
}

fn futures_empty() -> impl futures::Stream<Item = StreamEvent> {
    futures::stream::empty()
}

fn registry() -> (Arc<GuestProviderRegistry>, Arc<Mutex<Vec<bool>>>) {
    let phases = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(GuestProviderRegistry::new());
    registry.upsert_live_provider(
        "llama.cpp",
        Arc::new(Recording {
            id: ProviderId::from("llama.cpp"),
            auth: ProviderAuth::with_api_key(Arc::new(AlwaysResolves)),
            phases: Arc::clone(&phases),
        }),
    );
    (registry, phases)
}

fn policy(offline: bool) -> NetworkPolicy {
    NetworkPolicy {
        offline,
        update_check: true,
        install_telemetry: true,
        analytics: false,
    }
}

/// rpc and interactive refresh the extension providers, restore first and then the network phase,
/// exactly the two phases of `ModelsImpl.refresh` (`models.ts:570-577`).
#[tokio::test]
async fn rpc_and_interactive_refresh_the_extension_providers_in_the_background() {
    for mode in [AppMode::Rpc, AppMode::Interactive] {
        let (registry, phases) = registry();

        let task = spawn_extension_provider_refresh(registry, policy(false), mode)
            .unwrap_or_else(|| panic!("{mode:?} refreshes at startup"));
        task.await.unwrap();

        assert_eq!(
            *phases.lock().unwrap(),
            [false, true],
            "{mode:?}: the cache restore, then the network phase"
        );
    }
}

/// print and json run issue no request (pi refreshes in rpc and interactive only, `main.ts:931-936`),
/// and `--offline` / `CYRUP_OFFLINE` stops the refresh in every mode.
#[tokio::test]
async fn print_json_and_offline_runs_refresh_nothing() {
    for (mode, offline) in [
        (AppMode::Print, false),
        (AppMode::Json, false),
        (AppMode::Rpc, true),
        (AppMode::Interactive, true),
    ] {
        let (registry, phases) = registry();

        let task = spawn_extension_provider_refresh(registry, policy(offline), mode);

        assert!(task.is_none(), "{mode:?} offline={offline} spawns nothing");
        assert!(phases.lock().unwrap().is_empty());
    }
}

/// An extension provider whose network phase blocks until the caller aborts, then records that it saw
/// the abort. The cache-restore phase (`allow_network == false`) returns at once.
struct BlockedUntilAborted {
    id: ProviderId,
    auth: ProviderAuth,
    saw_abort: Arc<Mutex<bool>>,
}

#[async_trait::async_trait]
impl Provider for BlockedUntilAborted {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &[]
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.auth)
    }
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        if ctx.allow_network {
            ctx.cancelled().await;
            *self.saw_abort.lock().unwrap() = true;
        }
        Some(Ok(()))
    }
    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        Box::pin(futures_empty())
    }
}

/// pi's `setTimeout(() => controller.abort(), 15_000)` (`main.ts:933`, `interactive-mode.ts:1119`): a
/// provider that blocks forever is ABORTED after 15 s, not left to hang, and the abort reaches the
/// provider through the refresh context's signal. Runs on the paused clock, so the 15 s is virtual.
#[tokio::test(start_paused = true)]
async fn a_blocked_extension_refresh_is_aborted_after_fifteen_seconds() {
    let saw_abort = Arc::new(Mutex::new(false));
    let registry = Arc::new(GuestProviderRegistry::new());
    registry.upsert_live_provider(
        "llama.cpp",
        Arc::new(BlockedUntilAborted {
            id: ProviderId::from("llama.cpp"),
            auth: ProviderAuth::with_api_key(Arc::new(AlwaysResolves)),
            saw_abort: Arc::clone(&saw_abort),
        }),
    );

    let started = tokio::time::Instant::now();
    let task = spawn_extension_provider_refresh(registry, policy(false), AppMode::Rpc)
        .expect("rpc refreshes at startup");
    // The outer bound is what turns "never aborted" into a failure instead of a hang.
    tokio::time::timeout(std::time::Duration::from_secs(60), task)
        .await
        .expect("the refresh task ends: the 15 s timeout aborts the blocked provider")
        .unwrap();

    assert!(
        *saw_abort.lock().unwrap(),
        "the provider observed the abort through its refresh context"
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_secs(15)
            && elapsed < std::time::Duration::from_secs(16),
        "aborted at the 15 s mark, got {elapsed:?}"
    );
}
