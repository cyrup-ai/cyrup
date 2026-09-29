//! TUI-105 — `AgentSession::refresh_provider_catalog`, the SCOPED post-login catalog refresh.
//!
//! The row's own Verify: **a slow unrelated provider must not consume the deadline of a login to a
//! fast one.** pi's `completeProviderAuthentication` refreshes only the provider it just
//! authenticated — `session.modelRuntime.refresh({ providers: [providerId], signal: controller.signal })`
//! (`packages/coding-agent/src/modes/interactive/interactive-mode.ts:5953` @v0.87.1) — under one
//! `AbortController` (`:5950`) and one 15 s `setTimeout` (`:5951`). cyrup's login path used to call
//! `refresh_model_catalogs`, which resolves EVERY credentialed provider, so that single budget was
//! split across N providers and one slow stranger could eat it. When it was eaten the deferred model
//! selection never re-ran and the user was left modelless — the failure this row was filed for.
//!
//! Both halves are asserted here, under an identical two-provider setup, so the NARROWING is what is
//! proven rather than "a refresh happens":
//!
//! * `refresh_provider_catalog(short_deadline, fast)` settles clean, and
//! * `refresh_model_catalogs(short_deadline)` — the pre-existing whole-catalog call — times out.
//!
//! The second test pins the radius half of the fetch/overlay split
//! (`cyrup-provider/src/catalog_refresh.rs`, `refresh_and_install`'s doc): `radius` is excluded from
//! the FETCH list because the pi.dev route 404s for it, while the overlay is still reloaded.
//!
//! **No network.** Every request goes to a `tokio::net::TcpListener` on `127.0.0.1:0`, and the
//! catalog's auth context is an empty environment so an ambient `HTTP_PROXY` cannot reroute it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cyrup_config::AuthStore;
use cyrup_core::{CancelToken, ProviderId};
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_provider::{
    AuthContext, CatalogOverlaySlot, InMemoryModelsStore, ModelCatalogService, Provider,
    RADIUS_PROVIDER_ID, RefreshOptions, RemoteCatalog, all_providers,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::{AgentSession, SessionBuilder, SessionConfig};

/// The provider the user "just logged in to": answers its catalog request immediately.
const FAST: &str = "groq";
/// The unrelated provider that is already credentialed and whose origin never answers.
const SLOW: &str = "openai";

// ------------------------------------------------------------------------------ loopback origin --

/// An `AuthContext` over an EMPTY environment: no `HTTP_PROXY`, no `NO_PROXY`. Injecting it is what
/// keeps this hermetic on a machine that has a proxy configured.
struct EmptyEnv;

#[async_trait::async_trait]
impl AuthContext for EmptyEnv {
    async fn env(&self, _name: &str) -> Option<String> {
        None
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

/// A catalog origin that records which providers were asked for and STALLS one of them forever.
///
/// A task per connection, deliberately: the whole point is to have the stalled request and the fast
/// one in flight simultaneously. A serial accept loop would let the stall block the fast request and
/// the test would pass for the wrong reason.
struct Origin {
    base_url: String,
    requested: Arc<std::sync::Mutex<Vec<String>>>,
    accepts: Arc<AtomicUsize>,
}

impl Origin {
    async fn spawn(stall: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let requested = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let accepts = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requested);
        let count = Arc::clone(&accepts);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    // `GET /api/models/providers/<id> HTTP/1.1`
                    let provider = head
                        .split_whitespace()
                        .nth(1)
                        .and_then(|p| p.rsplit('/').next())
                        .unwrap_or_default()
                        .to_string();
                    seen.lock().unwrap().push(provider.clone());
                    if provider == stall {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                        return;
                    }
                    let body = "[]";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            requested,
            accepts,
        }
    }

    fn providers_requested(&self) -> Vec<String> {
        self.requested.lock().unwrap().clone()
    }

    fn accept_count(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }
}

// ------------------------------------------------------------------------------------- fixture --

struct Fx {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn installed() -> Arc<dyn Provider> {
    Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![FauxModelDefinition::new("faux-1")],
        ..FauxConfig::default()
    }))
}

fn catalog_service(base_url: &str) -> Arc<ModelCatalogService> {
    let catalog = Arc::new(
        RemoteCatalog::new(Arc::new(InMemoryModelsStore::new()))
            .with_base_url(base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_request_timeout(Duration::from_secs(60)),
    );
    Arc::new(
        ModelCatalogService::new(catalog, Arc::new(CatalogOverlaySlot::new()))
            .with_overlay_providers(
                all_providers()
                    .iter()
                    .map(|p| p.id().as_str().to_string())
                    .collect(),
            )
            .with_options(RefreshOptions {
                allow_network: true,
                force: true,
            }),
    )
}

/// A session whose catalog service points at `origin`, with BOTH [`FAST`] and [`SLOW`] credentialed —
/// the state a user is in after logging in to a second provider.
async fn session_over(fx: &Fx, origin: &Origin) -> AgentSession {
    let auth = Arc::new(
        AuthStore::at(fx.agent_dir.join("auth.json"))
            .with_ambient_env(std::collections::HashMap::new()),
    );
    auth.set_runtime_api_key(ProviderId::from(FAST), "sk-fast-test".to_string());
    auth.set_runtime_api_key(ProviderId::from(SLOW), "sk-slow-test".to_string());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    SessionBuilder::new(installed(), cfg)
        .auth(auth)
        .model_catalog_service(catalog_service(&origin.base_url))
        .build()
        .await
        .expect("session builds")
}

/// pi's `setTimeout(() => controller.abort(), 15_000)` (`interactive-mode.ts:5951`), shrunk.
fn cancel_after(after: Duration) -> CancelToken {
    let token = CancelToken::new();
    let fire = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        fire.cancel();
    });
    token
}

// --------------------------------------------------------------------------------------- tests --

/// **The row's Verify.** A login to a fast provider completes inside its budget even though a slow
/// unrelated provider is credentialed; the whole-catalog call under the identical setup does not.
///
/// **Red without the change:** `begin_post_login_catalog_refresh` called `refresh_model_catalogs`,
/// which is what the second assertion below still measures — the scoped call did not exist, so the
/// login inherited the `timed_out` the whole-catalog path produces here.
#[tokio::test]
async fn a_slow_unrelated_provider_does_not_eat_a_login_s_deadline() {
    let origin = Origin::spawn(SLOW).await;
    let fx = fixture();

    // The post-login refresh: scoped to the provider just authenticated.
    let scoped = session_over(&fx, &origin).await;
    let scoped_result = scoped
        .refresh_provider_catalog(cancel_after(Duration::from_secs(3)), FAST)
        .await;
    assert!(
        !scoped_result.timed_out && !scoped_result.aborted,
        "a login to `{FAST}` must settle within its own budget while `{SLOW}` stalls; got \
         {scoped_result:?}"
    );
    assert!(
        origin.providers_requested().contains(&FAST.to_string()),
        "the scoped refresh must fetch the provider just authenticated, saw {:?}",
        origin.providers_requested()
    );
    assert!(
        !origin.providers_requested().contains(&SLOW.to_string()),
        "the scoped refresh must NOT fetch anyone else — that is the narrowing; saw {:?}",
        origin.providers_requested()
    );

    // The WHOLE-catalog call, same setup: the slow provider eats the budget. This is what the login
    // path used to do.
    let whole = session_over(&fx, &origin).await;
    let whole_result = whole
        .refresh_model_catalogs(cancel_after(Duration::from_secs(3)))
        .await;
    assert!(
        whole_result.timed_out,
        "the whole-catalog refresh spans every credentialed provider, so `{SLOW}` consumes the \
         deadline — this is the behaviour the scoped call exists to avoid; got {whole_result:?}"
    );
}

/// The radius half of the fetch/overlay split: `/login radius` issues NO catalog request (the pi.dev
/// route 404s for it) yet still returns cleanly, because the overlay is reloaded regardless.
///
/// **Red without the change:** drop the `RADIUS_PROVIDER_ID` filter in `refresh_provider_catalog` and
/// `radius` is fetched like any other provider.
#[tokio::test]
async fn a_radius_login_fetches_nothing_and_still_settles_clean() {
    let origin = Origin::spawn(SLOW).await;
    let fx = fixture();
    let session = session_over(&fx, &origin).await;

    let result = session
        .refresh_provider_catalog(cancel_after(Duration::from_secs(3)), RADIUS_PROVIDER_ID)
        .await;

    assert!(
        !result.timed_out && !result.aborted && result.errors.is_empty(),
        "an empty fetch list is not an error — the overlay is still reloaded; got {result:?}"
    );
    assert_eq!(
        origin.accept_count(),
        0,
        "radius must be excluded from the FETCH list, saw requests for {:?}",
        origin.providers_requested()
    );
}
