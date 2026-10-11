//! PROV-159 — the production pairing, over the wire: a classifier-only provider's pi.dev catalog
//! revalidates against `<agent_dir>/models-store.json` and a `304` keeps its rows ON DISK.
//!
//! pi's validator is `stored && stored.models.length > 0 ? stored.etag : undefined`
//! (`packages/coding-agent/src/core/remote-catalog-provider.ts:98-100` @f1b2e77f5), over the ONE
//! persisted array of every type; its `304` branch persists `{ ...stored, checkedAt }` (`:117-122`).
//! `cyrup-provider`'s own `tests::remote_catalog` proves both against the in-memory store; this file
//! runs the same refresh through [`cyrup_config::models_store::FileModelsStore`], the store the
//! binary and the session builder pair `RemoteCatalog` with, and reopens the file afterwards.
//!
//! **No network.** The origin is a `tokio::net::TcpListener` on `127.0.0.1:0`, and the catalog's
//! auth context is an empty environment so an ambient `HTTP_PROXY` cannot reroute it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_config::models_store::{FileModelsStore, MODELS_STORE_FILE_NAME};
use cyrup_provider::models_store::ModelsStore;
use cyrup_provider::{AuthContext, RefreshOptions, RemoteCatalog};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// An `AuthContext` over an EMPTY environment.
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

/// Serves `responses` in order (each a full raw HTTP/1.1 response) and records every request head.
async fn origin(responses: Vec<String>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let heads = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&heads);
    tokio::spawn(async move {
        let mut index = 0usize;
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            seen.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).to_lowercase());
            let response = &responses[index.min(responses.len() - 1)];
            index += 1;
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    (format!("http://{addr}"), heads)
}

fn catalog(store: FileModelsStore, base_url: &str) -> Arc<RemoteCatalog> {
    Arc::new(
        RemoteCatalog::new(Arc::new(store))
            .with_base_url(base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_request_timeout(std::time::Duration::from_secs(5)),
    )
}

/// Red at the base: its validator asked only the (empty) chat half, so the second request carried
/// no `if-none-match` and re-downloaded the catalog.
#[tokio::test]
async fn a_classifier_only_catalog_revalidates_against_the_models_store_file() {
    let body = serde_json::json!([{
        "type": "classifier",
        "id": "jev-next",
        "name": "Jev Next",
        "api": "typesafe-system-one",
        "provider": "typesafe",
        "baseUrl": "https://api.typesafe.ai/v1/",
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 64000
    }])
    .to_string();
    let (base_url, heads) = origin(vec![
        format!(
            "HTTP/1.1 200 OK\r\netag: \"ts-1\"\r\nlast-modified: Sat, 11 Jul 2026 10:00:00 GMT\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ),
        "HTTP/1.1 304 Not Modified\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string(),
    ])
    .await;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(MODELS_STORE_FILE_NAME);

    let remote = catalog(FileModelsStore::new(path.clone()), &base_url);
    remote
        .refresh_provider("typesafe", RefreshOptions::network())
        .await
        .expect("the 200 refresh succeeds");
    remote
        .refresh_provider("typesafe", RefreshOptions::forced())
        .await
        .expect("a 304 is not an error");

    let heads = heads.lock().unwrap().clone();
    assert_eq!(heads.len(), 2);
    assert!(!heads[0].contains("if-none-match"), "{}", heads[0]);
    assert!(heads[1].contains("if-none-match: \"ts-1\""), "{}", heads[1]);

    // Reopen the file: the 304 moved only `checkedAt`, and the classifier row is still on disk.
    let reopened = FileModelsStore::new(path.clone());
    let entry = reopened
        .read("typesafe", None)
        .await
        .unwrap()
        .expect("entry kept");
    assert!(entry.models.is_empty(), "the premise: no chat half");
    assert_eq!(entry.etag.as_deref(), Some("\"ts-1\""));
    assert_eq!(entry.last_modified, Some(1_783_764_000_000));
    let rows = reopened
        .read_classifier_models("typesafe", None)
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["jev-next"]
    );
    let overlay = catalog(FileModelsStore::new(path), &base_url)
        .load_overlay(&["typesafe"])
        .await;
    assert_eq!(
        overlay
            .classifier_models_for("typesafe")
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["jev-next"]
    );
}
