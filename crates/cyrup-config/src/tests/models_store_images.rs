//! `FileModelsStore` and image models (PROV-128).
//!
//! Pi persists every model type in the entry's ONE `models` array
//! (`ModelsStoreEntry.models: readonly AnyModel[]`, `packages/ai/src/models-store.ts:3-5`). cyrup's
//! typed [`ModelsStoreEntry`] carries chat rows only, so each other type travels through a sibling
//! method pair; `EXT-027` added the classifier pair and PROV-128 adds the image one, because the
//! pi.dev catalog now asks for `?types=chat,image,classifier` and a `?types=` body carries 59 image
//! rows for `openrouter` alone. Without a file backend for them, every restart would drop the image
//! half of the overlay — the overlay would be "present and tested" and dead in production.
//!
//! **No network.** Everything here is a temp-dir file.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cyrup_provider::models_store::{ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use cyrup_provider::{ClassifierModel, ImageModel, Modality, Model, ModelCost};
use serde_json::Value;

use crate::models_store::{FileModelsStore, MODELS_STORE_FILE_NAME};

const PROVIDER: &str = "openrouter";

fn chat(id: &str) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: "openai-completions".into(),
        provider: PROVIDER.into(),
        base_url: "https://openrouter.ai/api/v1".to_string(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 4096,
        max_tokens: 4096,
        sampling_params: None,
        prompt_cache: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// An image row in pi's v1.0.0 shape: a `BaseModel` plus `type: "image"` and `output`, and NO
/// `contextWindow`/`maxTokens`/`reasoning` — which is exactly why it cannot sit in the chat half.
fn image(id: &str) -> ImageModel {
    ImageModel {
        id: id.into(),
        name: id.to_string(),
        api: "openrouter-images".into(),
        provider: PROVIDER.into(),
        base_url: "https://openrouter.ai/api/v1".to_string(),
        input: vec![Modality::Text, Modality::Image],
        output: vec![Modality::Image],
        cost: ModelCost::default(),
        headers: None,
    }
}

fn classifier(id: &str) -> ClassifierModel {
    ClassifierModel {
        id: id.into(),
        name: id.to_string(),
        api: "typesafe-system-one".into(),
        provider: PROVIDER.into(),
        base_url: "https://openrouter.ai/api/v1".to_string(),
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        headers: None,
        context_window: 32_000,
    }
}

fn entry(chat_ids: &[&str], checked_at: i64) -> ModelsStoreEntry {
    ModelsStoreEntry {
        models: chat_ids.iter().map(|id| chat(id)).collect(),
        last_modified: Some(1),
        checked_at: Some(checked_at),
        etag: None,
    }
}

fn path() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(MODELS_STORE_FILE_NAME);
    (dir, path)
}

fn on_disk(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn ids(models: &[Model]) -> Vec<&str> {
    models.iter().map(|m| m.id.as_str()).collect()
}

fn image_ids(models: &[ImageModel]) -> Vec<&str> {
    models.iter().map(|m| m.id.as_str()).collect()
}

/// **The one that matters most for the file backend.** An image member in the `models` array must
/// not cost the provider its CHAT entry.
///
/// `FileModelsStore::read` rebuilds a typed [`ModelsStoreEntry`] from the entry's chat members, so
/// the chat/non-chat split has to be keyed on `type` generally. It used to be keyed on
/// `"classifier"` specifically: an image member therefore landed in the chat half, and
/// `serde_json::from_value::<ModelsStoreEntry>` then failed the WHOLE entry on
/// `missing field reasoning` — `read` answered `None` and the provider silently lost its entire
/// persisted overlay, chat rows included, the first time a `?types=` refresh wrote one.
#[tokio::test]
async fn an_image_member_does_not_poison_the_chat_entry() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write(PROVIDER, entry(&["chat-a", "chat-b"], 7), None)
        .await
        .unwrap();
    store
        .write_image_models(PROVIDER, vec![image("img-a")], None)
        .await
        .unwrap();

    // Both halves are in the one on-disk array, pi's shape.
    let disk = on_disk(&path);
    let members = disk[PROVIDER]["models"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(
        members.iter().filter(|m| m["type"] == "image").count(),
        1,
        "the image member carries its `type` discriminant on disk"
    );

    // And the chat entry still reads back — the assertion that was red.
    let stored = FileModelsStore::new(&path)
        .read(PROVIDER, None)
        .await
        .unwrap()
        .expect("the chat entry must survive an image member beside it");
    assert_eq!(ids(&stored.models), ["chat-a", "chat-b"]);
    assert_eq!(stored.checked_at, Some(7));
}

/// Chat + image + classifier all survive a restart, read back through their own accessors.
#[tokio::test]
async fn every_model_type_survives_a_restart() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write_all_types(
            PROVIDER,
            entry(&["chat-a"], 11),
            vec![image("img-a"), image("img-b")],
            vec![classifier("cls-a")],
            None,
        )
        .await
        .unwrap();

    let fresh = FileModelsStore::new(&path);
    let stored = fresh.read(PROVIDER, None).await.unwrap().expect("entry");
    assert_eq!(ids(&stored.models), ["chat-a"]);
    let images = fresh.read_image_models(PROVIDER, None).await.unwrap();
    assert_eq!(image_ids(&images), ["img-a", "img-b"]);
    assert_eq!(
        images[0].output,
        vec![Modality::Image],
        "an image row's `output` must round-trip"
    );
    let classifiers = fresh.read_classifier_models(PROVIDER, None).await.unwrap();
    assert_eq!(classifiers.len(), 1);
    assert_eq!(classifiers[0].context_window, 32_000);
    // Neither non-chat reader may pick up the other's members.
    assert!(images.iter().all(|m| m.api.as_str() == "openrouter-images"));
}

/// `write_all_types` replaces all three halves in ONE locked write — the previous refresh's image
/// rows are gone, not merged, because the body it came from is one catalog.
#[tokio::test]
async fn write_all_types_replaces_every_half_together() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write_all_types(
            PROVIDER,
            entry(&["old"], 1),
            vec![image("img-old")],
            vec![classifier("cls-old")],
            None,
        )
        .await
        .unwrap();
    store
        .write_all_types(
            PROVIDER,
            entry(&["new"], 2),
            vec![image("img-new")],
            Vec::new(),
            None,
        )
        .await
        .unwrap();

    let stored = store.read(PROVIDER, None).await.unwrap().expect("entry");
    assert_eq!(ids(&stored.models), ["new"]);
    assert_eq!(
        image_ids(&store.read_image_models(PROVIDER, None).await.unwrap()),
        ["img-new"]
    );
    assert!(
        store
            .read_classifier_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty(),
        "an empty classifier list clears the half"
    );
}

/// The three halves are INDEPENDENT the rest of the time: `write` owns the chat members,
/// `write_image_models` the image members, `write_classifier_models` the classifier members, and
/// none of them may delete another's.
///
/// `write_classifier_models` used to rebuild the array as `chat ++ classifiers`, which dropped
/// every other non-chat member — so once the pi.dev refresh persisted image rows, the next
/// classifier write would have deleted them.
#[tokio::test]
async fn the_three_halves_do_not_overwrite_each_other() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write_all_types(
            PROVIDER,
            entry(&["chat-a"], 1),
            vec![image("img-a")],
            vec![classifier("cls-a")],
            None,
        )
        .await
        .unwrap();

    // A classifier-only write keeps the chat and IMAGE halves.
    store
        .write_classifier_models(PROVIDER, vec![classifier("cls-b")], None)
        .await
        .unwrap();
    assert_eq!(
        image_ids(&store.read_image_models(PROVIDER, None).await.unwrap()),
        ["img-a"],
        "a classifier write must not delete the image members"
    );
    assert_eq!(
        ids(&store.read(PROVIDER, None).await.unwrap().unwrap().models),
        ["chat-a"]
    );

    // An image-only write keeps the chat and CLASSIFIER halves.
    store
        .write_image_models(PROVIDER, vec![image("img-b")], None)
        .await
        .unwrap();
    assert_eq!(
        store
            .read_classifier_models(PROVIDER, None)
            .await
            .unwrap()
            .len(),
        1,
        "an image write must not delete the classifier members"
    );

    // A chat-only write keeps both non-chat halves.
    store
        .write(PROVIDER, entry(&["chat-c"], 3), None)
        .await
        .unwrap();
    assert_eq!(
        image_ids(&store.read_image_models(PROVIDER, None).await.unwrap()),
        ["img-b"]
    );
    assert_eq!(
        store
            .read_classifier_models(PROVIDER, None)
            .await
            .unwrap()
            .len(),
        1
    );

    // `write_with_classifiers` (the llama refresh path) also keeps the image half.
    store
        .write_with_classifiers(
            PROVIDER,
            entry(&["chat-d"], 4),
            vec![classifier("cls-c")],
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        image_ids(&store.read_image_models(PROVIDER, None).await.unwrap()),
        ["img-b"],
        "write_with_classifiers must not delete the image members"
    );
}

/// An empty image list clears the half and creates nothing for a provider with no entry; deleting
/// the provider removes every half.
#[tokio::test]
async fn an_empty_image_list_clears_and_delete_removes_everything() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);

    store
        .write_image_models("nobody", Vec::new(), None)
        .await
        .unwrap();
    assert!(
        !path.exists() || on_disk(&path).get("nobody").is_none(),
        "an empty list must not create an entry"
    );

    store
        .write_all_types(
            PROVIDER,
            entry(&["chat-a"], 1),
            vec![image("img-a")],
            vec![classifier("cls-a")],
            None,
        )
        .await
        .unwrap();
    store
        .write_image_models(PROVIDER, Vec::new(), None)
        .await
        .unwrap();
    assert!(
        store
            .read_image_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ids(&store.read(PROVIDER, None).await.unwrap().unwrap().models),
        ["chat-a"],
        "clearing the image half keeps the chat half"
    );

    store.delete(PROVIDER, None).await.unwrap();
    assert!(store.read(PROVIDER, None).await.unwrap().is_none());
    assert!(
        store
            .read_image_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty()
    );
}

/// The abort is honoured in the same position as every other method: before the cross-process lock
/// and before any I/O (`models-store.ts:31`, `:37`).
#[tokio::test]
async fn image_reads_and_writes_honour_an_aborted_signal() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write_all_types(
            PROVIDER,
            entry(&["kept"], 1),
            vec![image("img-a")],
            Vec::new(),
            None,
        )
        .await
        .unwrap();

    let token = cyrup_core::CancelToken::new();
    token.cancel();
    let options = ModelsStoreOperationOptions {
        signal: Some(token),
    };
    assert!(
        store
            .read_image_models(PROVIDER, Some(&options))
            .await
            .is_err()
    );
    assert!(
        store
            .write_image_models(PROVIDER, Vec::new(), Some(&options))
            .await
            .is_err()
    );
    assert!(
        store
            .write_all_types(
                PROVIDER,
                ModelsStoreEntry::default(),
                Vec::new(),
                Vec::new(),
                Some(&options)
            )
            .await
            .is_err()
    );
    // Nothing was touched.
    assert_eq!(
        image_ids(&store.read_image_models(PROVIDER, None).await.unwrap()),
        ["img-a"]
    );
}

/// A row that came from pi.dev carries `"type":"chat"` EXPLICITLY (checked against the live endpoint
/// on 2026-10-05: every row of the model-id-keyed shard does). It must read as a chat member, not
/// be mistaken for a non-chat one and dropped out of the entry.
#[tokio::test]
async fn an_explicit_chat_type_member_reads_as_a_chat_model() {
    let (_dir, path) = path();
    let mut row = serde_json::to_value(chat("explicit")).unwrap();
    row.as_object_mut()
        .unwrap()
        .insert("type".to_string(), Value::String("chat".to_string()));
    std::fs::write(
        &path,
        serde_json::json!({ PROVIDER: { "models": [row], "checkedAt": 5 } }).to_string(),
    )
    .unwrap();

    let store = FileModelsStore::new(&path);
    let stored = store.read(PROVIDER, None).await.unwrap().expect("entry");
    assert_eq!(ids(&stored.models), ["explicit"]);
    assert!(
        store
            .read_image_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .read_classifier_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty()
    );
}
