//! The runtime catalog refresh fires in the modes Pi fires it in — and in no others (DRIFT-007).
//!
//! Pi has exactly TWO catalog-refresh triggers:
//!
//! - `packages/coding-agent/src/main.ts:863-866` —
//!   `if (!offlineMode && appMode === "rpc") { void modelRuntime.refresh().catch(() => {}); }`,
//!   guarded on the mode by name.
//! - `packages/coding-agent/src/modes/interactive/interactive-mode.ts` `run()` —
//!   `if (!process.env.PI_OFFLINE) { void this.session.modelRuntime.refresh().then(…).catch(…) }`,
//!   reached only from the interactive front-end.
//!
//! Runtime CREATION never fetches: every `ModelRuntime.create` call in `coding-agent` passes
//! `allowModelNetwork: false` (`main.ts:158`, `package-manager-cli.ts:401`) and
//! `model-runtime.ts:163` computes
//! `const refreshFromNetwork = runtime.modelNetworkEnabled && options.allowModelNetwork === true;`.
//! So a one-shot `pi -p "…"` and pi's JSON output mode issue no catalog request at all, and cyrup's
//! scripted/CI path must not either — adding an outbound `https://pi.dev` request there would be a
//! network trigger upstream does not have.
//!
//! **No network.** Every request in this file goes to a raw `tokio::net::TcpListener` on
//! `127.0.0.1:0`; the base URL is the only transport seam (Pi's `catalogBaseUrl` option) and an
//! empty `AuthContext` is injected so an ambient `HTTP_PROXY` on the developer's machine cannot
//! reroute a loopback request. The negative cases assert the listener's ACCEPT COUNT is exactly
//! zero, so a test cannot pass by silently reaching the real origin. Because
//! `spawn_model_catalog_refresh_with` hands back the `JoinHandle`, the positive case AWAITS the
//! task instead of sleeping — there is no timing window in either direction.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::AppMode;
use crate::provider::{mode_refreshes_catalogs, spawn_model_catalog_refresh_with};
use cyrup_config::policy::NetworkPolicy;
use cyrup_provider::auth::AuthContext;
use cyrup_provider::models_store::{InMemoryModelsStore, ModelsStore};
use cyrup_provider::remote_catalog::RemoteCatalog;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ------------------------------------------------------------------------------ loopback server --

/// A raw-TCP HTTP/1.1 responder on `127.0.0.1:0` that answers everything `404` and counts accepts,
/// so a test can prove that NO request was issued. Same technique as
/// `cyrup-provider/tests/remote_catalog.rs` and `cyrup-session-svc/tests/wasm_http.rs:56-80`.
struct MockOrigin {
    base_url: String,
    accepts: Arc<AtomicUsize>,
}

impl MockOrigin {
    async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let accepts = Arc::new(AtomicUsize::new(0));
        let count = accepts.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await;
                // 404 is Pi's "this provider has no remote catalog" branch: it clears the overlay
                // and is explicitly NOT an error, so the task always completes cleanly.
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await;
                let _ = sock.flush().await;
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            accepts,
        }
    }

    fn accept_count(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }
}

/// An `AuthContext` with an EMPTY environment: no `HTTP_PROXY`, no `NO_PROXY`, nothing. Injecting it
/// is what keeps these tests hermetic on a machine that has a proxy configured.
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

fn catalog(base_url: &str) -> Arc<RemoteCatalog> {
    let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::default());
    Arc::new(
        RemoteCatalog::new(store)
            .with_base_url(base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_request_timeout(std::time::Duration::from_secs(5)),
    )
}

/// A policy that permits outbound traffic, so the ONLY thing under test is the mode gate.
fn online() -> NetworkPolicy {
    let policy = NetworkPolicy {
        offline: false,
        update_check: true,
        install_telemetry: false,
        analytics: false,
    };
    assert!(
        policy.allow_model_catalog_refresh(),
        "sanity: the default policy must be online, or the mode gate is not what is under test"
    );
    policy
}

/// Drive the refresh for one mode and report how many requests actually reached the origin.
/// Awaits the spawned task when there is one, so the count is final and not a race.
async fn accepts_for(mode: AppMode) -> usize {
    let origin = MockOrigin::spawn().await;
    let handle = spawn_model_catalog_refresh_with(
        catalog(&origin.base_url),
        online(),
        mode,
        vec!["groq".to_string()],
    );
    if let Some(task) = handle {
        task.await.expect("the refresh task does not panic");
    }
    origin.accept_count()
}

// ------------------------------------------------------------------------------------- the gate --

/// The blocker: a scripted one-shot run must issue NO catalog request. Pi's `-p`/JSON path has no
/// refresh trigger at all, so cyrup's must not manufacture one.
#[tokio::test]
async fn print_and_json_modes_issue_no_catalog_request() {
    assert_eq!(
        accepts_for(AppMode::Print).await,
        0,
        "`cyrup -p \"…\"` must not reach the catalog origin: Pi's print path has no refresh trigger"
    );
    assert_eq!(
        accepts_for(AppMode::Json).await,
        0,
        "`cyrup --mode json` must not reach the catalog origin: Pi's JSON path has no refresh trigger"
    );
}

/// The other half of the gate: the two modes Pi DOES refresh in still refresh, so gating the
/// scripted path off cannot have been achieved by disabling the feature.
#[tokio::test]
async fn rpc_and_interactive_modes_do_refresh() {
    assert_eq!(
        accepts_for(AppMode::Rpc).await,
        1,
        "RPC must still refresh (Pi main.ts:864, `appMode === \"rpc\"`)"
    );
    assert_eq!(
        accepts_for(AppMode::Interactive).await,
        1,
        "interactive must still refresh (Pi interactive-mode.ts `run()`)"
    );
}

/// Offline still wins in the refreshing modes — the mode gate is an ADDITIONAL narrowing, never a
/// replacement for `PI_OFFLINE`/`CYRUP_OFFLINE` (Pi guards on both: `!offlineMode && appMode === "rpc"`).
#[tokio::test]
async fn offline_still_suppresses_the_refreshing_modes() {
    for mode in [AppMode::Rpc, AppMode::Interactive] {
        let origin = MockOrigin::spawn().await;
        let mut policy = online();
        policy.offline = true;
        let handle = spawn_model_catalog_refresh_with(
            catalog(&origin.base_url),
            policy,
            mode,
            vec!["groq".to_string()],
        );
        assert!(
            handle.is_none(),
            "offline must decline to spawn at all in {mode:?}"
        );
        assert_eq!(
            origin.accept_count(),
            0,
            "offline must issue no request in {mode:?}"
        );
    }
}

// ------------------------------------------------------------------- `cyrup update --models` --

/// The THIRD trigger — the only foreground one, and the only one a user asks for by name.
///
/// `cyrup update --models` did not exist at all before this pass (the binary answered `Unknown
/// option --models for "update"`), so there was no CLI route to refresh the model catalogs; pi has
/// had `refreshModelCatalogs` since `package-manager-cli.ts:397-423` @v0.83.0.
///
/// Three properties, all against the loopback origin:
///
/// - it FETCHES for each configured provider (`force: true`, `:409`) — no mode gate, no freshness
///   window, unlike both background triggers;
/// - a second call fetches AGAIN, which is what `force` means: the 4h window
///   (`REMOTE_CATALOG_REFRESH_INTERVAL_MS`) would otherwise make this a no-op;
/// - a provider with no credential is never in the set the caller passes, so an empty set issues
///   ZERO requests and still succeeds — pi's per-provider `if (!credential) return;`
///   (`packages/ai/src/models.ts:296`) reaching the same end state.
#[tokio::test]
async fn update_models_forces_a_fetch_for_every_configured_provider() {
    let origin = MockOrigin::spawn().await;
    let catalog = catalog(&origin.base_url);

    crate::provider::refresh_model_catalogs_with(
        catalog.clone(),
        vec!["groq".to_string(), "openai".to_string()],
    )
    .await
    .expect("a 404 from the origin is pi's `no remote catalog` branch, not an error");
    assert_eq!(
        origin.accept_count(),
        2,
        "one request per configured provider"
    );

    // `force: true` bypasses the freshness window, so the SAME call fetches again immediately.
    crate::provider::refresh_model_catalogs_with(catalog, vec!["groq".to_string()])
        .await
        .expect("forced refresh");
    assert_eq!(
        origin.accept_count(),
        3,
        "`cyrup update --models` must bypass the 4h window (RefreshOptions::forced)"
    );
}

/// PROV-014 — `radius` is never fetched from pi.dev (`model-runtime.ts:183-189` @v0.84.4:
/// `provider.id === "radius" ? provider : withRemoteCatalog(…)`): a configured radius alongside a
/// configured groq issues exactly ONE request, and a lone radius issues none.
#[tokio::test]
async fn update_models_never_fetches_radius_from_pi_dev() {
    let origin = MockOrigin::spawn().await;
    let catalog = catalog(&origin.base_url);

    crate::provider::refresh_model_catalogs_with(
        catalog.clone(),
        vec!["radius".to_string(), "groq".to_string()],
    )
    .await
    .expect("radius is skipped, groq's 404 is pi's no-remote-catalog branch");
    assert_eq!(origin.accept_count(), 1, "only groq reaches pi.dev");

    crate::provider::refresh_model_catalogs_with(catalog, vec!["radius".to_string()])
        .await
        .expect("a radius-only set is a successful no-op");
    assert_eq!(origin.accept_count(), 1, "radius alone issues no request");

    assert_eq!(
        crate::provider::pi_dev_catalog_providers(&[
            "radius".to_string(),
            "openai".to_string(),
            "radius".to_string()
        ]),
        vec!["openai".to_string()]
    );
}

/// No credential anywhere ⇒ no request, and still a success: a fresh install can run
/// `cyrup update --models` without an `auth.json` and gets `Model catalogs refreshed`, exactly as
/// upstream does.
#[tokio::test]
async fn update_models_with_no_configured_provider_issues_no_request() {
    let origin = MockOrigin::spawn().await;
    crate::provider::refresh_model_catalogs_with(catalog(&origin.base_url), Vec::new())
        .await
        .expect("an empty provider set is a successful no-op");
    assert_eq!(origin.accept_count(), 0);
}

/// The predicate itself, stated once against Pi's two sites so the intent is greppable.
#[test]
fn only_rpc_and_interactive_refresh_catalogs() {
    assert!(mode_refreshes_catalogs(AppMode::Rpc));
    assert!(mode_refreshes_catalogs(AppMode::Interactive));
    assert!(!mode_refreshes_catalogs(AppMode::Print));
    assert!(!mode_refreshes_catalogs(AppMode::Json));
}
