//! Runtime model-catalog refresh — the pi.dev overlay, end to end (DRIFT-007).
//!
//! Ports the case list of pi's own `packages/coding-agent/test/remote-catalog-provider.test.ts` and
//! adds the two invariants that matter more in cyrup than upstream, because cyrup ships the catalogs
//! compiled in:
//!
//! 1. **The floor.** `overlay_never_removes_an_embedded_model` and
//!    `every_failure_mode_leaves_the_embedded_catalogs_intact` assert that no refresh outcome —
//!    disabled, offline, timed out, 404, 500, garbage body, poisoned cache — can leave a user with
//!    fewer models than the embedded catalogs already give them.
//! 2. **No network in tests.** Every request in this file goes to a raw
//!    `tokio::net::TcpListener` bound on `127.0.0.1:0`, the established technique in this workspace
//!    (`cyrup-session-svc/tests/wasm_http.rs:56-80`, `cyrup-ext/src/caps/http.rs`,
//!    `cyrup-provider/src/wire.rs`). The base URL is the ONLY transport injection seam, exactly as
//!    upstream's `catalogBaseUrl` option is, and the proxy-resolution environment is injected too so
//!    an ambient `HTTP_PROXY` on the developer's machine cannot reroute a loopback request.
//!    A test that "passes" by silently reaching `https://pi.dev` would be worse than no test, so the
//!    listener double-checks: the no-request cases assert the accept count is exactly zero.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::auth::AuthContext;
use crate::models_store::{
    InMemoryModelsStore, ModelsStore, ModelsStoreEntry, ProviderModelsStore,
};
use crate::remote_catalog::{
    CatalogOverlay, DEFAULT_CATALOG_BASE_URL, REMOTE_CATALOG_REFRESH_INTERVAL_MS, RefreshOptions,
    RemoteCatalog, merge_models,
};
use crate::{
    CreateModelsOptions, Model, all_providers, all_providers_with_overlay,
    builtin_model_data_generated_at, default_models,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ------------------------------------------------------------------------------ loopback server --

/// One canned HTTP/1.1 response.
struct Canned {
    status_line: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
}

impl Canned {
    fn ok(body: &str) -> Self {
        Self {
            status_line: "HTTP/1.1 200 OK",
            headers: Vec::new(),
            body: body.to_string(),
        }
    }
    fn status(status_line: &'static str) -> Self {
        Self {
            status_line,
            headers: Vec::new(),
            body: String::new(),
        }
    }
    fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

/// A raw-TCP HTTP/1.1 responder on `127.0.0.1:0`. Serves `responses` in order (the last one repeats),
/// records every request head it received, and counts accepts so a test can prove that NO request
/// was issued.
struct MockOrigin {
    base_url: String,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    accepts: Arc<AtomicUsize>,
}

impl MockOrigin {
    async fn spawn(responses: Vec<Canned>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let accepts = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let count = accepts.clone();
        tokio::spawn(async move {
            let mut index = 0usize;
            while let Ok((mut sock, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                let canned = &responses[index.min(responses.len().saturating_sub(1))];
                index += 1;
                let mut head = format!("{}\r\n", canned.status_line);
                for (name, value) in &canned.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str(&format!("content-length: {}\r\n", canned.body.len()));
                head.push_str("connection: close\r\n\r\n");
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(canned.body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            requests,
            accepts,
        }
    }

    fn accept_count(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }

    fn request_heads(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
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

fn catalog(store: Arc<dyn ModelsStore>, base_url: &str) -> Arc<RemoteCatalog> {
    Arc::new(
        RemoteCatalog::new(store)
            .with_base_url(base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_request_timeout(std::time::Duration::from_secs(5)),
    )
}

/// A model body in the neutral camelCase serde form the catalogs already use.
fn model_json(id: &str, context_window: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": id,
        "api": "openai-completions",
        "provider": "groq",
        "baseUrl": "https://api.groq.com/openai/v1",
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
        "contextWindow": context_window,
        "maxTokens": 8192
    })
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_millis(),
    )
    .expect("fits i64")
}

fn embedded_groq_models() -> Vec<Model> {
    all_providers()
        .into_iter()
        .find(|p| p.id().as_str() == "groq")
        .expect("groq is a built-in provider")
        .models()
        .to_vec()
}

// ------------------------------------------------------------------------------- the happy path --

#[tokio::test]
async fn first_refresh_fetches_and_persists_body_etag_and_timestamps() {
    let origin = MockOrigin::spawn(vec![
        Canned::ok(&serde_json::json!([model_json("remote-only", 111)]).to_string())
            .header("etag", "\"v1\"")
            .header("last-modified", "Sat, 11 Jul 2026 10:00:00 GMT"),
    ])
    .await;
    let store = Arc::new(InMemoryModelsStore::new());
    let catalog = catalog(store.clone(), &origin.base_url);

    catalog
        .refresh_provider("groq", RefreshOptions::network())
        .await
        .expect("a 200 refresh succeeds");

    // The route matches pi's `/api/models/providers/<encodeURIComponent(id)>`.
    let head = &origin.request_heads()[0];
    assert!(
        head.starts_with("GET /api/models/providers/groq "),
        "unexpected request line: {head}"
    );
    assert!(head.to_lowercase().contains("accept: application/json"));
    assert!(head.to_lowercase().contains("user-agent: cyrup/"));
    // No cached body yet, so no validator may be sent.
    assert!(!head.to_lowercase().contains("if-none-match"));

    let stored = store.read("groq").await.unwrap().expect("entry persisted");
    assert_eq!(stored.models.len(), 1);
    assert_eq!(stored.models[0].id.as_str(), "remote-only");
    // The etag is stored VERBATIM, quotes included.
    assert_eq!(stored.etag.as_deref(), Some("\"v1\""));
    // `date -u -d 'Sat, 11 Jul 2026 10:00:00 GMT' +%s` = 1783764000.
    assert_eq!(stored.last_modified, Some(1_783_764_000_000));
    assert!(stored.checked_at.unwrap() > 0);
}

#[tokio::test]
async fn a_second_refresh_inside_the_four_hour_window_issues_no_request() {
    let origin = MockOrigin::spawn(vec![Canned::ok("[]")]).await;
    let store = Arc::new(InMemoryModelsStore::new());
    // A completed check moments ago, with a `lastModified` — Pi requires BOTH before skipping.
    store
        .write(
            "groq",
            ModelsStoreEntry {
                models: vec![serde_json::from_value(model_json("cached", 9)).unwrap()],
                last_modified: Some(1),
                checked_at: Some(now_ms()),
                etag: Some("\"v1\"".into()),
            },
        )
        .await
        .unwrap();
    let catalog = catalog(store.clone(), &origin.base_url);

    catalog
        .refresh_provider("groq", RefreshOptions::network())
        .await
        .expect("a fresh entry short-circuits");
    assert_eq!(
        origin.accept_count(),
        0,
        "the 4h freshness window must suppress the request entirely"
    );

    // Aging the check past the window re-enables the fetch...
    store
        .write(
            "groq",
            ModelsStoreEntry {
                checked_at: Some(now_ms() - REMOTE_CATALOG_REFRESH_INTERVAL_MS - 1),
                ..store.read("groq").await.unwrap().unwrap()
            },
        )
        .await
        .unwrap();
    catalog
        .refresh_provider("groq", RefreshOptions::network())
        .await
        .unwrap();
    assert_eq!(origin.accept_count(), 1);
}

#[tokio::test]
async fn force_bypasses_the_window_and_a_304_moves_only_checked_at() {
    let origin = MockOrigin::spawn(vec![Canned::status("HTTP/1.1 304 Not Modified")]).await;
    let store = Arc::new(InMemoryModelsStore::new());
    let before = ModelsStoreEntry {
        models: vec![serde_json::from_value(model_json("cached", 9)).unwrap()],
        last_modified: Some(42),
        checked_at: Some(1),
        etag: Some("W/\"v7\"".into()),
    };
    store.write("groq", before.clone()).await.unwrap();
    let catalog = catalog(store.clone(), &origin.base_url);

    catalog
        .refresh_provider("groq", RefreshOptions::forced())
        .await
        .expect("a 304 is not an error");

    // The stored validator is echoed VERBATIM, weak-etag prefix and all.
    let head = origin.request_heads()[0].to_lowercase();
    assert!(head.contains("if-none-match: w/\"v7\""), "{head}");

    let after = store.read("groq").await.unwrap().unwrap();
    assert_eq!(after.models, before.models, "a 304 must not touch the body");
    assert_eq!(after.etag, before.etag);
    assert_eq!(after.last_modified, before.last_modified);
    assert!(after.checked_at.unwrap() > 1, "only the window moves");
}

#[tokio::test]
async fn a_304_is_never_requested_without_a_cached_body() {
    // An entry that carries an etag but NO models: sending `if-none-match` here could return 304 and
    // leave the overlay permanently empty. Pi suppresses the validator; so must cyrup.
    let origin = MockOrigin::spawn(vec![Canned::ok("[]")]).await;
    let store = Arc::new(InMemoryModelsStore::new());
    store
        .write(
            "groq",
            ModelsStoreEntry {
                models: Vec::new(),
                last_modified: Some(0),
                checked_at: Some(1),
                etag: Some("\"stale\"".into()),
            },
        )
        .await
        .unwrap();
    catalog(store, &origin.base_url)
        .refresh_provider("groq", RefreshOptions::forced())
        .await
        .unwrap();
    assert!(
        !origin.request_heads()[0].to_lowercase().contains("if-none-match"),
        "validator must be suppressed when no cached body backs it"
    );
}

// ------------------------------------------------------------------------------ failure policy ---

#[tokio::test]
async fn a_404_or_501_clears_the_overlay_and_never_errors() {
    for status in ["HTTP/1.1 404 Not Found", "HTTP/1.1 501 Not Implemented"] {
        let origin = MockOrigin::spawn(vec![Canned::status(status)]).await;
        let store = Arc::new(InMemoryModelsStore::new());
        store
            .write(
                "groq",
                ModelsStoreEntry {
                    models: vec![serde_json::from_value(model_json("cached", 9)).unwrap()],
                    last_modified: Some(99),
                    checked_at: Some(1),
                    etag: Some("\"v1\"".into()),
                },
            )
            .await
            .unwrap();
        // Guard enabled, so the post-404 entry is judged against the builtin manifest stamp.
        let catalog = Arc::new(
            RemoteCatalog::new(store.clone())
                .with_base_url(&origin.base_url)
                .with_auth_context(Arc::new(EmptyEnv))
                .with_local_generated_at(builtin_model_data_generated_at()),
        );
        catalog
            .refresh_provider("groq", RefreshOptions::forced())
            .await
            .unwrap_or_else(|e| panic!("{status} must not error: {e}"));
        let after = store.read("groq").await.unwrap().unwrap();
        assert_eq!(after.last_modified, Some(0), "{status}");
        assert_eq!(after.etag, None, "{status}");
        // `lastModified: 0` is below the builtin stamp, so the staleness guard drops the overlay:
        // "route unimplemented" degrades to the embedded catalogs, never to a stale overlay.
        assert!(
            catalog.load_overlay(&["groq"]).await.is_empty(),
            "{status}: overlay must go away"
        );
    }
}

#[tokio::test]
async fn a_500_keeps_the_etag_and_surfaces_an_error() {
    let origin = MockOrigin::spawn(vec![Canned::status("HTTP/1.1 500 Internal Server Error")]).await;
    let store = Arc::new(InMemoryModelsStore::new());
    let before = ModelsStoreEntry {
        models: vec![serde_json::from_value(model_json("cached", 9)).unwrap()],
        last_modified: Some(99),
        checked_at: Some(1),
        etag: Some("\"v1\"".into()),
    };
    store.write("groq", before.clone()).await.unwrap();

    let err = catalog(store.clone(), &origin.base_url)
        .refresh_provider("groq", RefreshOptions::forced())
        .await
        .expect_err("an unexpected status is an error");
    assert!(
        err.to_string()
            .contains("Model catalog request failed for groq: 500"),
        "{err}"
    );

    let after = store.read("groq").await.unwrap().unwrap();
    assert_eq!(after.models, before.models, "the cached body survives");
    assert_eq!(
        after.etag, before.etag,
        "the validator is KEPT so the next attempt revalidates instead of re-downloading"
    );
    assert!(after.checked_at.unwrap() > 1);
}

#[tokio::test]
async fn a_garbage_body_errors_without_destroying_the_cached_overlay() {
    let origin = MockOrigin::spawn(vec![Canned::ok("<html>nope</html>")]).await;
    let store = Arc::new(InMemoryModelsStore::new());
    let before = ModelsStoreEntry {
        models: vec![serde_json::from_value(model_json("cached", 9)).unwrap()],
        last_modified: Some(99),
        checked_at: Some(1),
        etag: Some("\"v1\"".into()),
    };
    store.write("groq", before.clone()).await.unwrap();

    let err = catalog(store.clone(), &origin.base_url)
        .refresh_provider("groq", RefreshOptions::forced())
        .await
        .expect_err("an unparseable body is an error");
    assert!(err.to_string().contains("Invalid model catalog"), "{err}");
    assert_eq!(
        store.read("groq").await.unwrap().unwrap().models,
        before.models,
        "a bad body must never overwrite the good one"
    );
}

#[tokio::test]
async fn a_dead_origin_is_a_transport_error_and_writes_nothing() {
    // Bind then immediately drop the listener: the port is (almost certainly) closed.
    let addr = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    };
    let store = Arc::new(InMemoryModelsStore::new());
    let err = catalog(store.clone(), &format!("http://{addr}"))
        .refresh_provider("groq", RefreshOptions::forced())
        .await
        .expect_err("a dead origin is an error");
    assert_eq!(err.code(), "transport", "{err}");
    assert!(
        store.read("groq").await.unwrap().is_none(),
        "a transport failure must not fabricate a store entry"
    );
}

// ------------------------------------------------------------------------------ offline / gating --

#[tokio::test]
async fn cache_only_issues_no_request_yet_still_loads_the_persisted_overlay() {
    let origin = MockOrigin::spawn(vec![Canned::ok("[]")]).await;
    let store = Arc::new(InMemoryModelsStore::new());
    store
        .write(
            "groq",
            ModelsStoreEntry {
                models: vec![serde_json::from_value(model_json("from-cache", 7)).unwrap()],
                last_modified: Some(i64::MAX / 2),
                checked_at: Some(1), // long stale: only `allow_network` keeps the request away
                etag: Some("\"v1\"".into()),
            },
        )
        .await
        .unwrap();
    let catalog = catalog(store, &origin.base_url);

    catalog
        .refresh_provider("groq", RefreshOptions::CACHE_ONLY)
        .await
        .expect("cache-only never fails");
    assert_eq!(
        origin.accept_count(),
        0,
        "offline/cache-only must issue ZERO requests"
    );

    // ...and the persisted overlay is still available, which is what makes an offline run keep the
    // models it saw last time (Pi reads the store BEFORE the allowNetwork gate).
    let overlay = catalog.load_overlay(&["groq"]).await;
    assert_eq!(overlay.models_for("groq").len(), 1);
    assert_eq!(overlay.models_for("groq")[0].id.as_str(), "from-cache");
}

#[tokio::test]
async fn the_default_base_url_is_pi_dev_and_is_never_reached_by_these_tests() {
    // Documents the production default without contacting it: the constant is asserted, and every
    // other test in this file overrides it with a loopback URL.
    assert_eq!(DEFAULT_CATALOG_BASE_URL, "https://pi.dev");
    assert_eq!(REMOTE_CATALOG_REFRESH_INTERVAL_MS, 4 * 60 * 60 * 1000);
}

// -------------------------------------------------------------------------------- the FLOOR ------

#[tokio::test]
async fn overlay_never_removes_an_embedded_model_and_can_add_or_replace_one() {
    let embedded = embedded_groq_models();
    assert!(!embedded.is_empty(), "groq ships a non-empty catalog");
    let replaced_id = embedded[0].id.as_str().to_string();

    let origin = MockOrigin::spawn(vec![
        Canned::ok(
            &serde_json::json!([
                model_json(&replaced_id, 4_242_424),
                model_json("brand-new-remote-model", 123_456),
            ])
            .to_string(),
        )
        .header("etag", "\"v1\"")
        // Strictly newer than the builtin manifest stamp, so the staleness guard keeps it.
        .header("last-modified", "Fri, 31 Dec 2027 00:00:00 GMT"),
    ])
    .await;
    let store = Arc::new(InMemoryModelsStore::new());
    let catalog = Arc::new(
        RemoteCatalog::new(store.clone())
            .with_base_url(&origin.base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_local_generated_at(builtin_model_data_generated_at()),
    );
    catalog
        .refresh_provider("groq", RefreshOptions::forced())
        .await
        .unwrap();

    let overlay = Arc::new(catalog.load_overlay(&["groq"]).await);
    let models = default_models(CreateModelsOptions {
        credentials: None,
        auth_context: None,
        catalog_overlay: Some(overlay.clone()),
    })
    .get_models(Some("groq"));

    // FLOOR: every embedded id still resolves.
    for m in &embedded {
        assert!(
            models.iter().any(|e| e.id == m.id),
            "the overlay removed embedded model {}",
            m.id.as_str()
        );
    }
    // ADD: the remote-only model is present.
    assert!(
        models
            .iter()
            .any(|m| m.id.as_str() == "brand-new-remote-model"),
        "the remote-only model did not reach the registry"
    );
    // REPLACE: the shared id took the remote metadata, in place.
    let replaced = models
        .iter()
        .find(|m| m.id.as_str() == replaced_id)
        .expect("replaced model present");
    assert_eq!(replaced.context_window, 4_242_424);
    assert_eq!(models.len(), embedded.len() + 1);

    // And the untouched providers are byte-identical to the no-overlay registry.
    let plain = default_models(CreateModelsOptions::default()).get_models(Some("xai"));
    let with_overlay = default_models(CreateModelsOptions {
        catalog_overlay: Some(overlay),
        ..Default::default()
    })
    .get_models(Some("xai"));
    assert_eq!(plain, with_overlay);
}

#[tokio::test]
async fn every_failure_mode_leaves_the_embedded_catalogs_intact() {
    let embedded_all = default_models(CreateModelsOptions::default()).get_models(None);
    assert!(embedded_all.len() > 100, "sanity: the builtins are large");

    // 404, 501, 500, garbage, and a dead origin — each followed by a full registry build.
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let cases: Vec<(String, Option<MockOrigin>)> = vec![
        (dead, None),
        {
            let o = MockOrigin::spawn(vec![Canned::status("HTTP/1.1 404 Not Found")]).await;
            (o.base_url.clone(), Some(o))
        },
        {
            let o = MockOrigin::spawn(vec![Canned::status("HTTP/1.1 501 Not Implemented")]).await;
            (o.base_url.clone(), Some(o))
        },
        {
            let o =
                MockOrigin::spawn(vec![Canned::status("HTTP/1.1 500 Internal Server Error")]).await;
            (o.base_url.clone(), Some(o))
        },
        {
            let o = MockOrigin::spawn(vec![Canned::ok("not json at all")]).await;
            (o.base_url.clone(), Some(o))
        },
        {
            // A well-formed but EMPTY catalog — the case that would silently delete a provider if
            // the overlay were a replacement instead of a merge.
            let o = MockOrigin::spawn(vec![Canned::ok("[]").header("etag", "\"empty\"")]).await;
            (o.base_url.clone(), Some(o))
        },
    ];

    for (base_url, _origin) in cases {
        let store = Arc::new(InMemoryModelsStore::new());
        let catalog = catalog(store.clone(), &base_url);
        // Errors are expected and deliberately ignored — the point is the registry afterwards.
        let _ = catalog
            .refresh_provider("groq", RefreshOptions::forced())
            .await;
        let overlay = Arc::new(catalog.load_overlay(&["groq"]).await);
        let after = default_models(CreateModelsOptions {
            catalog_overlay: Some(overlay),
            ..Default::default()
        })
        .get_models(None);
        assert_eq!(
            after, embedded_all,
            "a failed refresh against {base_url} changed the registry"
        );
    }
}

#[test]
fn a_none_overlay_is_byte_identical_to_the_pre_drift_007_registry() {
    // `all_providers_with` is the legacy entry point; `all_providers_with_overlay(.., None)` must be
    // indistinguishable from it, so nothing about the default path changed.
    let ids = |ps: Vec<Arc<dyn crate::Provider>>| -> Vec<(String, Vec<String>)> {
        ps.into_iter()
            .map(|p| {
                (
                    p.id().as_str().to_string(),
                    p.models()
                        .iter()
                        .map(|m| m.id.as_str().to_string())
                        .collect(),
                )
            })
            .collect()
    };
    let store: Arc<dyn crate::CredentialStore> =
        Arc::new(crate::InMemoryCredentialStore::new());
    let registry = Arc::new(crate::builtin_registry());
    let legacy = ids(crate::all_providers_with(
        store.clone(),
        registry.clone(),
    ));
    let overlaid = ids(all_providers_with_overlay(store, registry, None));
    assert_eq!(legacy, overlaid);

    // An EMPTY overlay is likewise inert.
    let store2: Arc<dyn crate::CredentialStore> =
        Arc::new(crate::InMemoryCredentialStore::new());
    let empty = CatalogOverlay::default();
    let inert = ids(all_providers_with_overlay(
        store2,
        Arc::new(crate::builtin_registry()),
        Some(&empty),
    ));
    assert_eq!(legacy, inert);
}

// -------------------------------------------------------------------------- the staleness guard --

#[tokio::test]
async fn an_overlay_not_newer_than_the_builtin_manifest_is_discarded_whole() {
    let generated_at = builtin_model_data_generated_at().expect("the manifest parses");
    let store = Arc::new(InMemoryModelsStore::new());
    let entry = |last_modified: i64| ModelsStoreEntry {
        models: vec![serde_json::from_value(model_json("remote-only", 5)).unwrap()],
        last_modified: Some(last_modified),
        checked_at: Some(now_ms()),
        etag: None,
    };

    // Older than the embedded catalogs (an overlay persisted BEFORE an upgrade): discarded.
    store.write("groq", entry(generated_at - 1)).await.unwrap();
    let catalog = Arc::new(
        RemoteCatalog::new(store.clone()).with_local_generated_at(Some(generated_at)),
    );
    assert!(catalog.load_overlay(&["groq"]).await.is_empty());

    // Exactly equal: still discarded (Pi uses `<=`).
    store.write("groq", entry(generated_at)).await.unwrap();
    assert!(catalog.load_overlay(&["groq"]).await.is_empty());

    // Strictly newer: kept.
    store.write("groq", entry(generated_at + 1)).await.unwrap();
    assert_eq!(catalog.load_overlay(&["groq"]).await.models_for("groq").len(), 1);
}

#[test]
fn the_builtin_manifest_stamp_agrees_with_the_documented_provenance() {
    // `tests/catalog_data.rs` documents the embedded catalogs as pi @ `91585d9a` (2026-07-10 16:34).
    // Before DRIFT-007 that provenance existed only in prose; this pins the machine-readable copy.
    // `date -u -d '2026-07-10T16:34:43Z' +%s` = 1783701283.
    assert_eq!(
        builtin_model_data_generated_at(),
        Some(1_783_701_283_000),
        "bump catalog_manifest.json in the same commit that refreshes providers/catalog/*.json"
    );
    assert!(crate::BUILTIN_CATALOG_MANIFEST_JSON.contains("91585d9a"));
}

// --------------------------------------------------------------------------------- single flight --

#[tokio::test]
async fn concurrent_refreshes_of_one_provider_collapse_onto_a_single_fetch() {
    let origin = MockOrigin::spawn(vec![
        Canned::ok(&serde_json::json!([model_json("m", 1)]).to_string()).header("etag", "\"v1\""),
    ])
    .await;
    let store = Arc::new(InMemoryModelsStore::new());
    let catalog = catalog(store, &origin.base_url);
    let results = futures::future::join_all(
        (0..8).map(|_| catalog.refresh_provider("groq", RefreshOptions::forced())),
    )
    .await;
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(
        origin.accept_count(),
        1,
        "eight concurrent refreshes must share ONE in-flight fetch (Pi `inflightRefresh`)"
    );
}

#[tokio::test]
async fn refresh_providers_is_best_effort_and_collects_per_provider_errors() {
    let origin = MockOrigin::spawn(vec![Canned::status("HTTP/1.1 500 Internal Server Error")]).await;
    let store = Arc::new(InMemoryModelsStore::new());
    let errors = catalog(store, &origin.base_url)
        .refresh_providers(&["groq", "xai"], RefreshOptions::forced())
        .await;
    assert_eq!(errors.len(), 2, "{errors:?}");
    let ids: Vec<&str> = errors.iter().map(|(id, _)| id.as_str()).collect();
    assert!(ids.contains(&"groq") && ids.contains(&"xai"));
}

// ------------------------------------------------------------------------------- scoped store ----

#[tokio::test]
async fn a_provider_scoped_store_cannot_see_another_providers_catalog() {
    let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
    store
        .write(
            "groq",
            ModelsStoreEntry {
                models: vec![serde_json::from_value(model_json("secret", 1)).unwrap()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let scoped = ProviderModelsStore::new(store, "xai");
    assert!(scoped.read().await.unwrap().is_none());
}

#[test]
fn merge_is_the_floor_guarantee() {
    let baseline = embedded_groq_models();
    // Whatever the dynamic side is — empty, or a disjoint set — every baseline id survives.
    for dynamic in [Vec::new(), vec![baseline[0].clone()]] {
        let merged = merge_models(&baseline, &dynamic);
        let merged_ids: BTreeMap<&str, ()> =
            merged.iter().map(|m| (m.id.as_str(), ())).collect();
        for m in &baseline {
            assert!(merged_ids.contains_key(m.id.as_str()));
        }
        assert!(merged.len() >= baseline.len());
    }
}
