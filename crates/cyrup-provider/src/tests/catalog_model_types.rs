//! PROV-128 — the pi.dev catalog's `?types=chat,image,classifier` parameter, and what arrives
//! because of it.
//!
//! # The fixtures are real responses, not hand-written ideals
//!
//! Both files under `fixtures/` are the VERBATIM bodies `https://pi.dev` served on 2026-10-05,
//! captured with the `accept: application/json` and `cyrup/…` User-Agent headers
//! [`crate::remote_catalog`] sends:
//!
//! | file | request | top-level shape | rows |
//! |---|---|---|---|
//! | `pi-dev-openrouter-no-types.json` | `/api/models/providers/openrouter` | **JSON object keyed by model id** | 400, all `chat` |
//! | `pi-dev-openrouter-all-types.json` | `…/openrouter?types=chat,image,classifier` | **JSON array** | 469 — 400 chat, 59 image, 10 classifier |
//!
//! (`etag: "bedb24e02fb4567a5cedfffcc7687845"` / `"9dcecf81d4848ee00f8e8ac9e8531adb"`,
//! `last-modified: Sat, 03 Oct 2026 16:17:44/46 GMT`.)
//!
//! **The two shards are different top-level JSON shapes.** That is the part the ledger row does not
//! say and the part that breaks a naive fix: adding the parameter without accepting the array form
//! turns every refresh into `Invalid model catalog for provider "openrouter"` on day one.
//! [`crate::remote_catalog::parse_catalog`] accepts both because pi's `parseCatalog` does
//! (`remote-catalog-provider.ts:37-51` @v1.0.1), and these tests are what keeps that true.
//!
//! # No network
//!
//! Every request goes to a `tokio::net::TcpListener` on `127.0.0.1:0` serving a fixture, with an
//! empty proxy environment injected — the same technique and the same reason as
//! [`crate::tests::remote_catalog`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::auth::AuthContext;
use crate::classifier::{AnyModel, ModelType};
use crate::images::{ImagesContext, ImagesOptions, ImagesStopReason};
use crate::models_store::{InMemoryModelsStore, ModelsStore};
use crate::remote_catalog::{
    REMOTE_CATALOG_MODEL_TYPES, RefreshOptions, RemoteCatalog, parse_catalog,
};
use crate::{CreateModelsOptions, all_providers_with_overlay, create_models, default_models};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// `GET /api/models/providers/openrouter` with no `types` parameter — the chat-only shard.
const NO_TYPES_BODY: &str = include_str!("fixtures/pi-dev-openrouter-no-types.json");
/// `GET /api/models/providers/openrouter?types=chat,image,classifier` — the full-type shard.
const ALL_TYPES_BODY: &str = include_str!("fixtures/pi-dev-openrouter-all-types.json");

const OPENROUTER: &str = "openrouter";

// ------------------------------------------------------------------------------ loopback origin --

struct Origin {
    base_url: String,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    accepts: Arc<AtomicUsize>,
}

impl Origin {
    /// Serve `body` to every request, recording each request head.
    async fn spawn(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let accepts = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let count = accepts.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                let head = format!(
                    "HTTP/1.1 200 OK\r\nlast-modified: Sat, 03 Oct 2026 16:17:46 \
                     GMT\r\netag: \"fixture\"\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            requests,
            accepts,
        }
    }

    fn request_heads(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn accept_count(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }
}

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

/// A client pointed at `base_url` with the hermetic (empty) proxy environment and a staleness
/// floor, built as the `Arc` `refresh_provider` takes.
fn catalog(store: Arc<dyn ModelsStore>, base_url: &str, floor_ms: i64) -> Arc<RemoteCatalog> {
    Arc::new(
        RemoteCatalog::new(store)
            .with_base_url(base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_request_timeout(std::time::Duration::from_secs(5))
            .with_local_generated_at(Some(floor_ms)),
    )
}

/// A floor far enough in the past that the fixture's `last-modified` is strictly newer, so the
/// staleness guard (`remote_models` / pi #7016) accepts the overlay.
const FLOOR_MS: i64 = 1_600_000_000_000;

/// Fetch the fixture through the real refresh path and return the loaded overlay.
async fn refresh_and_load(
    body: &'static str,
) -> (
    Origin,
    Arc<dyn ModelsStore>,
    crate::remote_catalog::CatalogOverlay,
) {
    let origin = Origin::spawn(body).await;
    let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
    let client = catalog(store.clone(), &origin.base_url, FLOOR_MS);
    client
        .refresh_provider(OPENROUTER, RefreshOptions::forced())
        .await
        .expect("the fixture is a 200 with a parseable body");
    let overlay = client.load_overlay(&[OPENROUTER]).await;
    (origin, store, overlay)
}

// ------------------------------------------------------------------------------------ the URL ----

/// **The parameter.** pi sets `types=chat,image,classifier` on every catalog request
/// (`remote-catalog-provider.ts:101-102`, the constant at `:22`); cyrup's URL carried no `types` at
/// all, which is why no image row had ever arrived. Asserted on the wire, from the request line the
/// origin actually received, not from a URL-building helper.
#[tokio::test]
async fn the_catalog_url_carries_every_model_type() {
    let (origin, _store, _overlay) = refresh_and_load(ALL_TYPES_BODY).await;
    let heads = origin.request_heads();
    assert_eq!(origin.accept_count(), 1, "exactly one request");
    let request_line = heads[0].lines().next().expect("a request line").to_string();
    assert_eq!(
        request_line, "GET /api/models/providers/openrouter?types=chat,image,classifier HTTP/1.1",
        "the catalog request must ask for every model type this build knows"
    );
    // The constant is the client's declared capability set, in pi's declaration order.
    assert_eq!(
        REMOTE_CATALOG_MODEL_TYPES,
        [ModelType::Chat, ModelType::Image, ModelType::Classifier]
    );
}

// --------------------------------------------------------------------------- both wire shapes ----

/// **Shape 1: the model-id-keyed object**, which pi.dev serves to a request with no `types`. 400
/// rows, every one chat. This is the shape cyrup has always received, and the control that the
/// `?types=` work did not break the old shard: a server that ignores the parameter still answers
/// this, and upstream's comment says so explicitly (`:17-21`).
#[test]
fn the_model_id_keyed_object_shape_parses() {
    let body: serde_json::Value = serde_json::from_str(NO_TYPES_BODY).expect("fixture is JSON");
    assert!(
        body.is_object(),
        "the no-types shard is a JSON OBJECT keyed by model id"
    );
    let parsed = parse_catalog(OPENROUTER, &body).expect("the dict shape must parse");
    assert_eq!(parsed.len(), 400, "every row of the dict shard parses");
    assert!(
        parsed.iter().all(|m| m.model_type() == ModelType::Chat),
        "the no-types shard is chat-only"
    );
    assert!(parsed.iter().all(|m| m.provider().as_str() == OPENROUTER));
}

/// **Shape 2: the JSON array**, which pi.dev serves once `?types=` is present — 469 rows across all
/// three types. Parsing this is the whole point of the parameter, and a parser that only knew the
/// object form would fail the body outright.
#[test]
fn the_json_array_shape_parses_with_all_three_types() {
    let body: serde_json::Value = serde_json::from_str(ALL_TYPES_BODY).expect("fixture is JSON");
    assert!(
        body.is_array(),
        "the all-types shard is a JSON ARRAY, not the dict the chat-only shard is"
    );
    let parsed = parse_catalog(OPENROUTER, &body).expect("the list shape must parse");
    assert_eq!(parsed.len(), 469, "every row of the all-types shard parses");

    let count = |kind: ModelType| parsed.iter().filter(|m| m.model_type() == kind).count();
    assert_eq!(count(ModelType::Chat), 400);
    assert_eq!(count(ModelType::Image), 59);
    assert_eq!(count(ModelType::Classifier), 10);

    // An image row is an `ImageModel` with its own members, not a `Model` with holes: `output` is
    // required and there is no `contextWindow` to be missing.
    let image = parsed
        .iter()
        .find_map(AnyModel::as_image)
        .expect("the shard carries image rows");
    assert_eq!(image.api.as_str(), crate::images::OPENROUTER_IMAGES);
    assert!(!image.output.is_empty());

    // The three-way split is the step that turns pi's one structurally-typed array into three
    // nominally-typed lists, after which a chat caller cannot be handed an image row at all.
    let overlay = crate::remote_catalog::ProviderOverlay::from_any(parsed);
    assert_eq!(overlay.chat().len(), 400);
    assert_eq!(overlay.images().len(), 59);
    assert_eq!(overlay.classifiers().len(), 10);
    assert!(overlay.images().iter().all(|m| !m.output.is_empty()));
    assert!(overlay.classifiers().iter().all(|m| m.context_window > 0));
}

/// The two shards agree on the chat rows: the array form's 400 chat rows are the dict form's 400,
/// id for id. If they ever diverge, the `?types=` switch is changing more than the envelope and
/// this test is the place that says so.
#[test]
fn the_two_shards_carry_the_same_chat_rows() {
    let dict = parse_catalog(
        OPENROUTER,
        &serde_json::from_str(NO_TYPES_BODY).expect("JSON"),
    )
    .expect("parses");
    let list = parse_catalog(
        OPENROUTER,
        &serde_json::from_str(ALL_TYPES_BODY).expect("JSON"),
    )
    .expect("parses");

    let mut from_dict: Vec<String> = dict.iter().map(|m| m.id().to_string()).collect();
    let mut from_list: Vec<String> = list
        .iter()
        .filter(|m| m.model_type() == ModelType::Chat)
        .map(|m| m.id().to_string())
        .collect();
    from_dict.sort();
    from_list.sort();
    assert_eq!(from_dict, from_list);
}

/// **The negative control the row asks for.** A row whose `type` this build does not know is
/// DROPPED, never an error — pi's `.filter(isSupportedModelType)` (`:24-30`, `:49`). Without the
/// filter, `AnyModel`'s own deserializer would reject the row (`unknown model type: video`) and,
/// because `parse_catalog` drops a row that fails to deserialize, the outcome would look the same
/// here — so the test also pins that the KNOWN rows beside it survive, which is what a
/// whole-body failure would take out.
#[test]
fn a_model_type_this_build_does_not_know_is_dropped_not_fatal() {
    let mut rows: Vec<serde_json::Value> =
        serde_json::from_str(ALL_TYPES_BODY).expect("fixture is a JSON array");
    rows.push(serde_json::json!({
        "type": "video",
        "id": "some/video-model",
        "name": "Some Video Model",
        "api": "some-video-api",
        "provider": OPENROUTER,
        "baseUrl": "https://openrouter.ai/api/v1",
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
    }));
    let parsed = parse_catalog(OPENROUTER, &serde_json::Value::Array(rows))
        .expect("an unknown type must not fail the body");
    assert_eq!(
        parsed.len(),
        469,
        "the unknown row is dropped, the rest stay"
    );
    assert!(parsed.iter().all(|m| m.id() != "some/video-model"));
}

// ------------------------------------------------------- the row reaches a Provider / Models ----

/// **The deliverable.** An image row served by the live endpoint's `?types=` shard becomes an
/// [`AnyModel::Image`] that is reachable from a [`crate::Provider`] and from the [`crate::Models`]
/// collection — the test that proves the variant is no longer test-only. It walks the WHOLE
/// production path: fetch → `parse_catalog` → store → `load_overlay` →
/// `all_providers_with_overlay` → `Models::get_all_models`.
#[tokio::test]
async fn an_image_row_from_the_catalog_reaches_the_provider_and_the_collection() {
    let (_origin, _store, overlay) = refresh_and_load(ALL_TYPES_BODY).await;
    assert_eq!(
        overlay.image_models_for(OPENROUTER).len(),
        59,
        "the overlay must carry the shard's image rows"
    );

    let providers = all_providers_with_overlay(
        Arc::new(crate::auth::InMemoryCredentialStore::new()),
        Arc::new(crate::api::builtin_registry()),
        Some(&overlay),
    );
    let openrouter = providers
        .iter()
        .find(|p| p.id().as_str() == OPENROUTER)
        .expect("openrouter is a built-in");

    // On the Provider: the overlay's rows merged over the 55 embedded ones.
    let from_provider: Vec<_> = openrouter
        .get_all_models()
        .into_iter()
        .filter_map(AnyModel::into_image)
        .collect();
    let overlay_ids: Vec<&str> = overlay
        .image_models_for(OPENROUTER)
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    for id in &overlay_ids {
        assert!(
            from_provider.iter().any(|m| m.id.as_str() == *id),
            "{id} must be listed by the provider"
        );
    }
    // The floor invariant for the image leg: the embedded rows are never removed by an overlay.
    for embedded in crate::providers::openrouter::openrouter_image_models() {
        assert!(
            from_provider.iter().any(|m| m.id == embedded.id),
            "{} was dropped by the overlay",
            embedded.id
        );
    }
    assert!(openrouter.supports_image_generation());

    // Through the Models collection, which is what a caller actually holds.
    let mut models = create_models(CreateModelsOptions::default());
    for provider in providers {
        models.set_provider(provider);
    }
    let typed = models.get_models_of_type(ModelType::Image, Some(OPENROUTER));
    assert_eq!(typed.len(), from_provider.len());
    assert!(typed.iter().all(|m| m.model_type() == ModelType::Image));

    let one = overlay_ids
        .iter()
        .find(|id| crate::providers::openrouter::openrouter_image_model(id).is_none())
        .map(|id| (*id).to_string())
        .expect("the shard has at least one row the embedded catalog does not");
    let resolved = models
        .get_image_model(OPENROUTER, &one)
        .unwrap_or_else(|| panic!("{one} must resolve as an image model out of the collection"));
    assert_eq!(resolved.provider.as_str(), OPENROUTER);

    // And it is dispatchable: `Models::generate_images` reaches the provider's `images` map rather
    // than reporting "does not support image generation". No credential is configured here, so the
    // terminal envelope is the auth one — which is itself the proof that support was checked first
    // and passed (pi checks support BEFORE auth, `models.ts:956-959`).
    let out = models
        .generate_images(
            &resolved,
            &ImagesContext::default(),
            &ImagesOptions::default(),
        )
        .await;
    assert_eq!(out.stop_reason, ImagesStopReason::Error);
    let message = out
        .error_message
        .expect("an error envelope carries a message");
    assert!(
        !message.contains("does not support image generation"),
        "the image leg must be reachable; got: {message}"
    );
    assert!(
        message.contains("Provider is not configured"),
        "expected the auth-stage message, got: {message}"
    );
}

/// **The control that matters most: 400 of the 469 rows are chat.** With the parameter in place,
/// the chat half of the overlay must be exactly what it was — same rows, merged the same way, with
/// the embedded floor intact and chat model resolution unchanged.
#[tokio::test]
async fn a_chat_row_still_becomes_a_chat_model_and_resolution_is_unchanged() {
    let (_origin, _store, overlay) = refresh_and_load(ALL_TYPES_BODY).await;
    assert_eq!(
        overlay.models_for(OPENROUTER).len(),
        400,
        "the chat half of the overlay is the shard's 400 chat rows"
    );
    assert!(
        overlay
            .models_for(OPENROUTER)
            .iter()
            .all(|m| m.provider.as_str() == OPENROUTER),
        "`models_for` is typed `&[Model]`, so an image row cannot be in it"
    );

    let baseline = default_models(CreateModelsOptions::default());
    let embedded = baseline.get_models(Some(OPENROUTER));
    assert!(!embedded.is_empty(), "openrouter ships embedded chat rows");

    let providers = all_providers_with_overlay(
        Arc::new(crate::auth::InMemoryCredentialStore::new()),
        Arc::new(crate::api::builtin_registry()),
        Some(&overlay),
    );
    let mut models = create_models(CreateModelsOptions::default());
    for provider in providers {
        models.set_provider(provider);
    }
    let merged = models.get_models(Some(OPENROUTER));

    // The floor: every embedded chat row still resolves, by id, through `get_model`.
    for row in &embedded {
        let found = models
            .get_model(OPENROUTER, row.id.as_str())
            .unwrap_or_else(|| panic!("{} was lost from the chat catalog", row.id));
        assert_eq!(found.provider.as_str(), OPENROUTER);
    }
    assert!(
        merged.len() >= embedded.len(),
        "the overlay can only add or replace: {} -> {}",
        embedded.len(),
        merged.len()
    );
    // And no image row leaked into the chat catalog, which `models(): &[Model]` makes a type-level
    // fact rather than something to filter.
    assert!(
        merged
            .iter()
            .all(|m| m.api.as_str() != crate::images::OPENROUTER_IMAGES)
    );
    // Every chat row is `AnyModel::Chat` on the multi-type read too.
    let chat = models.get_models_of_type(ModelType::Chat, Some(OPENROUTER));
    assert_eq!(chat.len(), merged.len());
}

/// **A classifier row still resolves**, because the same parameter brings ten of those. cyrup
/// implements no `typesafe-system-one` api (PROV-104 owns it), so the row must LIST and RESOLVE —
/// which is what PROV-128 is responsible for — and `classify()` on it must answer pi's
/// no-implementation message rather than vanishing.
#[tokio::test]
async fn a_classifier_row_from_the_catalog_still_resolves() {
    let (_origin, _store, overlay) = refresh_and_load(ALL_TYPES_BODY).await;
    assert_eq!(overlay.classifier_models_for(OPENROUTER).len(), 10);

    let providers = all_providers_with_overlay(
        Arc::new(crate::auth::InMemoryCredentialStore::new()),
        Arc::new(crate::api::builtin_registry()),
        Some(&overlay),
    );
    let mut models = create_models(CreateModelsOptions::default());
    for provider in providers {
        models.set_provider(provider);
    }
    let classifiers = models.get_classifier_models(Some(OPENROUTER));
    assert_eq!(classifiers.len(), 10);
    let one = &classifiers[0];
    assert!(
        models
            .get_classifier_model(OPENROUTER, one.id.as_str())
            .is_some(),
        "a classifier row must resolve by id"
    );
    assert!(
        one.context_window > 0,
        "pi's `ClassifierModel` declares `contextWindow`, unlike `ImageModel`"
    );
}

/// The chat-only shard still works end to end — a server that ignores `?types=` must behave exactly
/// as before: 400 chat rows, no image rows, no classifier rows, nothing persisted that was not
/// there.
#[tokio::test]
async fn the_chat_only_shard_still_overlays_exactly_as_before() {
    let (_origin, _store, overlay) = refresh_and_load(NO_TYPES_BODY).await;
    assert_eq!(overlay.models_for(OPENROUTER).len(), 400);
    assert!(overlay.image_models_for(OPENROUTER).is_empty());
    assert!(overlay.classifier_models_for(OPENROUTER).is_empty());

    // The embedded image rows are untouched by a chat-only overlay.
    let providers = all_providers_with_overlay(
        Arc::new(crate::auth::InMemoryCredentialStore::new()),
        Arc::new(crate::api::builtin_registry()),
        Some(&overlay),
    );
    let openrouter = providers
        .iter()
        .find(|p| p.id().as_str() == OPENROUTER)
        .expect("openrouter is a built-in");
    assert_eq!(
        openrouter
            .get_all_models()
            .iter()
            .filter(|m| m.as_image().is_some())
            .count(),
        55
    );
}

/// The three halves of one fetched catalog are persisted as ONE store operation and restored
/// together (pi writes one `models: AnyModel[]`). A reader must never see this refresh's chat rows
/// beside a previous one's image rows.
#[tokio::test]
async fn all_three_halves_are_persisted_and_restored_together() {
    let origin = Origin::spawn(ALL_TYPES_BODY).await;
    let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
    let client = catalog(store.clone(), &origin.base_url, FLOOR_MS);
    client
        .refresh_provider(OPENROUTER, RefreshOptions::forced())
        .await
        .expect("200");

    let entry = store
        .read(OPENROUTER, None)
        .await
        .expect("store read")
        .expect("an entry was written");
    assert_eq!(entry.models.len(), 400, "the chat half");
    assert_eq!(
        store
            .read_image_models(OPENROUTER, None)
            .await
            .unwrap()
            .len(),
        59,
        "the image half"
    );
    assert_eq!(
        store
            .read_classifier_models(OPENROUTER, None)
            .await
            .unwrap()
            .len(),
        10,
        "the classifier half"
    );

    // A SECOND client over the same store, with no network at all, restores all three — which is
    // the restart path.
    let offline = catalog(store.clone(), "http://127.0.0.1:1", FLOOR_MS);
    let restored = offline.load_overlay(&[OPENROUTER]).await;
    assert_eq!(restored.models_for(OPENROUTER).len(), 400);
    assert_eq!(restored.image_models_for(OPENROUTER).len(), 59);
    assert_eq!(restored.classifier_models_for(OPENROUTER).len(), 10);
}

/// The staleness guard (pi #7016) gates ALL THREE halves on one decision: a persisted entry that is
/// not strictly newer than the embedded catalogs contributes nothing of any type. Without this, the
/// image and classifier channels would be a way around the guard the chat channel enforces.
#[tokio::test]
async fn a_stale_entry_contributes_no_rows_of_any_type() {
    let origin = Origin::spawn(ALL_TYPES_BODY).await;
    let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
    catalog(store.clone(), &origin.base_url, FLOOR_MS)
        .refresh_provider(OPENROUTER, RefreshOptions::forced())
        .await
        .expect("200");

    // The fixture's `last-modified` is 2026-10-03; a floor after it makes the whole entry stale.
    let floor_after = 4_000_000_000_000;
    let overlay = catalog(store, &origin.base_url, floor_after)
        .load_overlay(&[OPENROUTER])
        .await;
    assert!(
        overlay.is_empty(),
        "a stale entry must contribute nothing: chat {}, images {}, classifiers {}",
        overlay.models_for(OPENROUTER).len(),
        overlay.image_models_for(OPENROUTER).len(),
        overlay.classifier_models_for(OPENROUTER).len()
    );
}
