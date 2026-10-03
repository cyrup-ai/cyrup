//! SEAM-138: the `/model` refresh (`AgentSession::refresh_model_catalogs`) refreshes the one
//! composed collection, as pi's `refreshModelCatalogs` does (`modes/interactive/
//! model-catalog-refresh.ts:46-51` @v0.99.2-17), so a provider a native extension registered LIVE
//! sees its `refresh_models` called, and its failure comes back in the result.
//!
//! The session here has NO pi.dev catalog service wired, which is the case that used to return
//! early before anything was refreshed. The providers are the in-memory ones of
//! `live_provider_auth.rs`; **no network** is touched.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, EventStream, ProviderId};
use cyrup_ext::NativeExtension;
use cyrup_provider::auth::ProviderAuth;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{
    Context, Model, Provider, ProviderError, RefreshModelsContext, StreamEvent, StreamOptions,
};
use tempfile::TempDir;

use super::live_provider_auth::{LLAMA, LiveNative, SERVER, URL_ENV, live};
use crate::{AgentSession, SessionBuilder, SessionConfig};

/// A live provider whose `refresh_models` records the phase it was asked for and answers `outcome`.
struct Recorder {
    inner: Arc<dyn Provider>,
    id: ProviderId,
    phases: Arc<Mutex<Vec<bool>>>,
    fail: bool,
}

#[async_trait::async_trait]
impl Provider for Recorder {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        self.inner.models()
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.inner.provider_auth()
    }
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        self.phases.lock().unwrap().push(ctx.allow_network);
        Some(if self.fail && ctx.allow_network {
            Err(ProviderError::ModelSource("server is down".into()))
        } else {
            Ok(())
        })
    }
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }
}

/// A session with no catalog service, whose only live provider is a [`Recorder`]; the build-time
/// restore has already run and been forgotten.
async fn session_over_recorder(fail: bool) -> (TempDir, AgentSession, Arc<Mutex<Vec<bool>>>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let ambient: HashMap<String, String> = [(URL_ENV.to_string(), SERVER.to_string())].into();
    let auth = Arc::new(
        cyrup_config::AuthStore::at(agent_dir.join("auth.json")).with_ambient_env(ambient),
    );
    let phases: Arc<Mutex<Vec<bool>>> = Arc::default();
    let provider: Arc<dyn Provider> = Arc::new(Recorder {
        inner: live(LLAMA, &["tiny"], true),
        id: ProviderId::from(LLAMA),
        phases: Arc::clone(&phases),
        fail,
    });
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.model_network_enabled = true;
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .auth(auth)
        .with_native_extension(Arc::new(LiveNative(vec![provider])) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("session builds");
    assert!(session.services().model_catalog.is_none());
    phases.lock().unwrap().clear();
    (tmp, session, phases)
}

/// A `/model` refresh reaches a live provider's `refresh_models`: the cache-only restore, then the
/// network phase (the provider's auth resolves, and the registry's switch is on).
///
/// **Red** without the change: `refresh_model_catalogs` returns at once for a session with no
/// catalog service and the provider is never asked.
#[tokio::test]
async fn a_model_refresh_reaches_a_live_providers_refresh_models() {
    let (_tmp, session, phases) = session_over_recorder(false).await;

    let result = session.refresh_model_catalogs(CancelToken::new()).await;

    assert!(
        !result.timed_out && !result.aborted && result.errors.is_empty(),
        "{result:?}"
    );
    assert_eq!(*phases.lock().unwrap(), vec![false, true]);
}

/// The live half's failure is the `/model` refresh's failure, keyed by provider id.
///
/// **Red** without the change: the failure is never observed, so `errors` stays empty.
#[tokio::test]
async fn a_live_providers_refresh_failure_is_reported_by_the_model_refresh() {
    let (_tmp, session, _phases) = session_over_recorder(true).await;

    let result = session.refresh_model_catalogs(CancelToken::new()).await;

    assert!(!result.timed_out && !result.aborted, "{result:?}");
    let message = result.errors.get(LLAMA).expect("the provider's failure");
    assert!(message.contains("server is down"), "{result:?}");
}

/// A token that has already fired aborts the live half too: nothing is asked of the provider and
/// the result says the refresh timed out.
#[tokio::test]
async fn a_cancelled_model_refresh_asks_no_live_provider() {
    let (_tmp, session, phases) = session_over_recorder(false).await;
    let cancel = CancelToken::new();
    cancel.cancel();

    let result = session.refresh_model_catalogs(cancel).await;

    assert!(result.timed_out, "{result:?}");
    assert!(phases.lock().unwrap().is_empty());
}
