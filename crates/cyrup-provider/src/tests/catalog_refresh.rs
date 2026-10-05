//! `ModelCatalogService::refresh_scoped` — the SCOPED catalog refresh the post-login path uses
//! (TUI-105).
//!
//! pi's `completeProviderAuthentication` refreshes only the provider it just authenticated, and does
//! it by calling `session.modelRuntime.refresh({ providers: [providerId], signal: controller.signal })`
//! DIRECTLY (`packages/coding-agent/src/modes/interactive/interactive-mode.ts:5953`) rather than
//! through `refreshModelCatalogs`
//! (`packages/coding-agent/src/modes/interactive/model-catalog-refresh.ts:46-51`), the coordinator it
//! reserves for whole-catalog refreshes. These two tests pin both halves of that: the fetch really is
//! narrowed to the one provider, and a scoped caller does NOT adopt an in-flight whole-catalog
//! operation (which would discard its scope and spend its deadline on somebody else's providers).
//!
//! No network: every request goes to a `tokio::net::TcpListener` on `127.0.0.1:0`, the technique
//! `remote_catalog.rs` established in this crate, with the same `EmptyEnv` auth context so an ambient
//! `HTTP_PROXY` cannot reroute a loopback request.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use cyrup_core::CancelToken;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::auth::AuthContext;
use crate::catalog_refresh::{CatalogOverlaySlot, ModelCatalogService};
use crate::models_store::InMemoryModelsStore;
use crate::remote_catalog::{RefreshOptions, RemoteCatalog};

/// An `AuthContext` with an EMPTY environment — no proxy variables — so these stay hermetic.
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

/// A loopback origin that records the provider id of every catalog request and can STALL a chosen
/// provider indefinitely.
///
/// Unlike `remote_catalog.rs`'s `MockOrigin` this spawns a task per connection: these tests need two
/// requests in flight at once, and a serial accept loop would let the stalled one block the other,
/// which would make the test pass for the wrong reason.
struct ForkingOrigin {
    base_url: String,
    requested: Arc<std::sync::Mutex<Vec<String>>>,
}

impl ForkingOrigin {
    /// `stall` names the provider whose request never gets a response.
    async fn spawn(stall: Option<&'static str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let requested = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&requested);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    // `GET /api/models/providers/<id>?types=chat,image,classifier HTTP/1.1` —
                    // PROV-128 added the query string, so the id is the last path segment with
                    // the query stripped. Keeping the query in would have made every provider id
                    // read as `<id>?types=...` and silently broken the narrowing assertions.
                    let target = head.split_whitespace().nth(1).unwrap_or_default();
                    let path = target.split('?').next().unwrap_or_default();
                    let provider = path.rsplit('/').next().unwrap_or_default().to_string();
                    seen.lock().unwrap().push(provider.clone());
                    if stall == Some(provider.as_str()) {
                        // Never answer. The caller's deadline is the only thing that ends this.
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
        }
    }

    fn providers_requested(&self) -> Vec<String> {
        let mut v = self.requested.lock().unwrap().clone();
        v.sort_unstable();
        v
    }
}

fn service(base_url: &str) -> ModelCatalogService {
    let catalog = Arc::new(
        RemoteCatalog::new(Arc::new(InMemoryModelsStore::new()))
            .with_base_url(base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_request_timeout(Duration::from_secs(30)),
    );
    ModelCatalogService::new(catalog, Arc::new(CatalogOverlaySlot::new()))
        .with_overlay_providers(vec![
            "groq".to_string(),
            "openai".to_string(),
            "deepseek".to_string(),
        ])
        .with_options(RefreshOptions {
            allow_network: true,
            force: true,
        })
}

/// Cancel `token` after `after` — the test stand-in for pi's
/// `setTimeout(() => controller.abort(), 15_000)` (`interactive-mode.ts:5951`).
fn cancel_after(token: &CancelToken, after: Duration) {
    let token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        token.cancel();
    });
}

/// The narrowing itself: a scoped refresh fetches the ONE provider named and nobody else, even though
/// the service's overlay roster lists three.
///
/// **Red without the change:** there is no `refresh_scoped`; the post-login path's only option is
/// `refresh`, whose `fetch_providers` the caller above it resolves as every credentialed provider.
#[tokio::test]
async fn refresh_scoped_fetches_only_the_named_provider() {
    let origin = ForkingOrigin::spawn(None).await;
    let svc = service(&origin.base_url);

    let result = svc
        .refresh_scoped(CancelToken::new(), vec!["groq".to_string()])
        .await;

    assert!(
        !result.timed_out && !result.aborted,
        "a reachable origin must settle cleanly, got {result:?}"
    );
    assert_eq!(
        origin.providers_requested(),
        vec!["groq".to_string()],
        "the fetch must be narrowed to the provider just authenticated, not the overlay roster"
    );
}

/// The coordinator bypass: a scoped refresh must NOT adopt an in-flight whole-catalog refresh.
///
/// pi calls `modelRuntime.refresh({ providers: [providerId], signal })` directly at `:5953` instead of
/// routing through `refreshModelCatalogs` (`model-catalog-refresh.ts:46-51`) precisely because a
/// joined operation's provider list wins. Here the whole-catalog refresh is stalled on `openai`; the
/// scoped `groq` refresh has to settle on its own request well inside its own deadline.
///
/// **Red without the change:** route `refresh_scoped`'s body through `self.refresh(...)` — i.e. give
/// the scoped caller the coordinator — and this call adopts the stalled whole-catalog operation, its
/// 2 s deadline fires, and it returns `timed_out` having never fetched `groq`.
#[tokio::test]
async fn refresh_scoped_does_not_join_an_in_flight_whole_catalog_refresh() {
    let origin = ForkingOrigin::spawn(Some("openai")).await;
    let svc = Arc::new(service(&origin.base_url));

    // The whole-catalog refresh `/model` would start, stalled on an unrelated slow provider.
    let whole = Arc::clone(&svc);
    let whole_cancel = CancelToken::new();
    let whole_token = whole_cancel.clone();
    tokio::spawn(async move {
        whole
            .refresh(
                whole_token,
                vec!["openai".to_string(), "deepseek".to_string()],
            )
            .await
    });
    // Let it reach the coordinator's in-flight slot, so there is genuinely something to join.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The post-login refresh, on a deadline far shorter than the stall.
    let scoped_cancel = CancelToken::new();
    cancel_after(&scoped_cancel, Duration::from_secs(2));
    let result = svc
        .refresh_scoped(scoped_cancel, vec!["groq".to_string()])
        .await;

    assert!(
        !result.timed_out && !result.aborted,
        "the scoped refresh must settle on its OWN request, not inherit the stalled whole-catalog \
         operation's fate; got {result:?}"
    );
    assert!(
        origin.providers_requested().contains(&"groq".to_string()),
        "the scoped refresh must actually fetch its own provider, saw {:?}",
        origin.providers_requested()
    );

    whole_cancel.cancel();
}
