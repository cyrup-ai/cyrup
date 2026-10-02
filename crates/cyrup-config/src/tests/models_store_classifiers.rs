//! `FileModelsStore` and classifier models (EXT-027).
//!
//! Pi persists a provider's chat models and its classifier models in the entry's ONE `models` array
//! (`ModelsStoreEntry.models: readonly AnyModel[]`, `packages/ai/src/models-store.ts:3-5`; the
//! llama.cpp provider writes `[...refreshed, ...refreshedClassifiers]`,
//! `extensions/llama/provider.ts:251-253` @v0.99.2-17). The trait's default
//! `write_classifier_models` refuses a non-empty list, so a refresh over the on-disk store used to
//! fail after writing its chat half; these tests pin the file backend that does not.
//!
//! **No network.** Everything here is a temp-dir file.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cyrup_provider::models_store::{ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use cyrup_provider::{ClassifierModel, Modality, Model, ModelCost};
use serde_json::{Value, json};

use crate::models_store::{FileModelsStore, MODELS_STORE_FILE_NAME};

const PROVIDER: &str = "llama.cpp";

fn chat(id: &str) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: "openai-completions".into(),
        provider: PROVIDER.into(),
        base_url: "http://127.0.0.1:1/v1".to_string(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 4096,
        max_tokens: 4096,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn classifier(id: &str) -> ClassifierModel {
    ClassifierModel {
        id: id.into(),
        name: id.to_string(),
        api: "llama-cpp-classify".into(),
        provider: PROVIDER.into(),
        base_url: "http://127.0.0.1:1".to_string(),
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        headers: None,
        context_window: 4096,
    }
}

fn entry(chat_ids: &[&str], checked_at: i64) -> ModelsStoreEntry {
    ModelsStoreEntry {
        models: chat_ids.iter().map(|id| chat(id)).collect(),
        last_modified: None,
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

fn classifier_ids(models: &[ClassifierModel]) -> Vec<&str> {
    models.iter().map(|m| m.id.as_str()).collect()
}

/// The refresh's publication, in the order the engine applies it (`publish_queued`): `write`, then
/// `write_classifier_models`. A fresh store (a restarted process) reads both halves back.
#[tokio::test]
async fn chat_and_classifier_models_survive_a_restart() {
    let (_dir, path) = path();
    {
        let store = FileModelsStore::new(&path);
        store
            .write(PROVIDER, entry(&["qwen3", "plain"], 7), None)
            .await
            .unwrap();
        store
            .write_classifier_models(
                PROVIDER,
                vec![classifier("qwen3"), classifier("plain")],
                None,
            )
            .await
            .unwrap();
    }

    let reopened = FileModelsStore::new(&path);
    let stored = reopened.read(PROVIDER, None).await.unwrap().unwrap();
    assert_eq!(ids(&stored.models), ["qwen3", "plain"]);
    assert_eq!(stored.checked_at, Some(7));
    let classifiers = reopened
        .read_classifier_models(PROVIDER, None)
        .await
        .unwrap();
    assert_eq!(classifier_ids(&classifiers), ["qwen3", "plain"]);
}

/// The file is pi's shape: ONE `models` array, chat members first, classifier members carrying
/// `type: "classifier"` (`provider.ts:251-253`).
#[tokio::test]
async fn the_file_holds_one_models_array_chat_first_then_classifiers() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write(PROVIDER, entry(&["a", "b"], 1), None)
        .await
        .unwrap();
    store
        .write_classifier_models(PROVIDER, vec![classifier("a"), classifier("b")], None)
        .await
        .unwrap();

    let stored = on_disk(&path);
    let models = stored[PROVIDER]["models"].as_array().unwrap();
    let kinds: Vec<(&str, Option<&str>)> = models
        .iter()
        .map(|m| (m["id"].as_str().unwrap(), m["type"].as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("a", None),
            ("b", None),
            ("a", Some("classifier")),
            ("b", Some("classifier"))
        ],
        "{stored}"
    );
    assert_eq!(stored[PROVIDER]["checkedAt"], 1);
}

/// `read` hands back the chat half only: a classifier member in the array must not take the whole
/// entry down (it is not a chat `Model`), which would read as "no cached catalog".
#[tokio::test]
async fn reading_the_entry_skips_the_classifier_members() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store.write(PROVIDER, entry(&["a"], 1), None).await.unwrap();
    store
        .write_classifier_models(PROVIDER, vec![classifier("a")], None)
        .await
        .unwrap();

    let stored = FileModelsStore::new(&path)
        .read(PROVIDER, None)
        .await
        .unwrap()
        .expect("the entry is still readable with a classifier member in it");
    assert_eq!(ids(&stored.models), ["a"]);
}

/// A later `write` replaces the chat half and leaves the classifier models (the two trait calls are
/// independent, `ModelsStore::write_classifier_models`).
#[tokio::test]
async fn writing_the_chat_half_keeps_the_classifier_models() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write(PROVIDER, entry(&["old"], 1), None)
        .await
        .unwrap();
    store
        .write_classifier_models(PROVIDER, vec![classifier("old")], None)
        .await
        .unwrap();

    store
        .write(PROVIDER, entry(&["new"], 2), None)
        .await
        .unwrap();

    let reopened = FileModelsStore::new(&path);
    let stored = reopened.read(PROVIDER, None).await.unwrap().unwrap();
    assert_eq!(ids(&stored.models), ["new"]);
    assert_eq!(stored.checked_at, Some(2));
    assert_eq!(
        classifier_ids(
            &reopened
                .read_classifier_models(PROVIDER, None)
                .await
                .unwrap()
        ),
        ["old"]
    );
}

/// Replacing the classifier models leaves the chat models and the entry's validators alone, and an
/// empty list clears them.
#[tokio::test]
async fn writing_classifier_models_keeps_the_chat_half_and_an_empty_list_clears_them() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    let mut with_validators = entry(&["a"], 3);
    with_validators.etag = Some("\"v1\"".to_string());
    with_validators.last_modified = Some(9);
    store.write(PROVIDER, with_validators, None).await.unwrap();
    store
        .write_classifier_models(PROVIDER, vec![classifier("first")], None)
        .await
        .unwrap();
    store
        .write_classifier_models(PROVIDER, vec![classifier("second")], None)
        .await
        .unwrap();

    let reopened = FileModelsStore::new(&path);
    let stored = reopened.read(PROVIDER, None).await.unwrap().unwrap();
    assert_eq!(ids(&stored.models), ["a"]);
    assert_eq!(stored.etag.as_deref(), Some("\"v1\""));
    assert_eq!(stored.last_modified, Some(9));
    assert_eq!(stored.checked_at, Some(3));
    assert_eq!(
        classifier_ids(
            &reopened
                .read_classifier_models(PROVIDER, None)
                .await
                .unwrap()
        ),
        ["second"],
        "replaced, not appended"
    );

    reopened
        .write_classifier_models(PROVIDER, Vec::new(), None)
        .await
        .unwrap();
    let cleared = FileModelsStore::new(&path);
    assert!(
        cleared
            .read_classifier_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ids(&cleared.read(PROVIDER, None).await.unwrap().unwrap().models),
        ["a"]
    );
}

/// `delete` removes the whole entry, classifier models included (`models-store.ts:143`).
#[tokio::test]
async fn deleting_a_provider_removes_its_classifier_models() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store.write(PROVIDER, entry(&["a"], 1), None).await.unwrap();
    store
        .write_classifier_models(PROVIDER, vec![classifier("a")], None)
        .await
        .unwrap();

    store.delete(PROVIDER, None).await.unwrap();

    let reopened = FileModelsStore::new(&path);
    assert!(reopened.read(PROVIDER, None).await.unwrap().is_none());
    assert!(
        reopened
            .read_classifier_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty()
    );
}

/// An empty list for a provider with no entry creates nothing: no file, no `{}` entry.
#[tokio::test]
async fn an_empty_classifier_list_creates_no_entry() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);

    store
        .write_classifier_models(PROVIDER, Vec::new(), None)
        .await
        .unwrap();

    assert!(!path.exists(), "nothing to store, nothing written");
}

/// A file written before classifier models existed (a plain chat entry, no `type` member anywhere)
/// reads exactly as it did, with an empty classifier list.
#[tokio::test]
async fn a_file_without_classifier_members_reads_as_before() {
    let (_dir, path) = path();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            PROVIDER: { "models": [chat("legacy")], "checkedAt": 5 },
            "groq": { "models": [], "etag": "\"g\"" },
        }))
        .unwrap(),
    )
    .unwrap();

    let store = FileModelsStore::new(&path);
    let stored = store.read(PROVIDER, None).await.unwrap().unwrap();
    assert_eq!(ids(&stored.models), ["legacy"]);
    assert!(
        store
            .read_classifier_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .read("groq", None)
            .await
            .unwrap()
            .unwrap()
            .etag
            .as_deref(),
        Some("\"g\"")
    );
}

/// Another provider's entry is untouched by a classifier write.
#[tokio::test]
async fn a_classifier_write_leaves_other_providers_alone() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    let mut groq = entry(&[], 1);
    groq.etag = Some("\"g\"".to_string());
    store.write("groq", groq, None).await.unwrap();

    store
        .write_classifier_models(PROVIDER, vec![classifier("a")], None)
        .await
        .unwrap();

    let reopened = FileModelsStore::new(&path);
    assert_eq!(
        reopened
            .read("groq", None)
            .await
            .unwrap()
            .unwrap()
            .etag
            .as_deref(),
        Some("\"g\"")
    );
    assert_eq!(
        classifier_ids(
            &reopened
                .read_classifier_models(PROVIDER, None)
                .await
                .unwrap()
        ),
        ["a"]
    );
}

/// A cancelled operation neither takes the lock nor touches the file (`models-store.ts:127-137`).
#[tokio::test]
async fn a_cancelled_classifier_operation_reports_the_abort_and_writes_nothing() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    let token = cyrup_core::CancelToken::new();
    token.cancel();
    let options = ModelsStoreOperationOptions {
        signal: Some(token),
    };

    let written = store
        .write_classifier_models(PROVIDER, vec![classifier("a")], Some(&options))
        .await;
    let read = store.read_classifier_models(PROVIDER, Some(&options)).await;

    assert!(written.unwrap_err().is_aborted());
    assert!(read.unwrap_err().is_aborted());
    assert!(!path.exists());
}

/// pi's single `write({ models: [...refreshed, ...refreshedClassifiers] })`: ONE operation replaces
/// both halves of the array, so the file never holds the new chat models beside the old classifier
/// models, and nothing the old classifiers were is kept.
#[tokio::test]
async fn write_with_classifiers_replaces_both_halves_in_one_write() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write_with_classifiers(
            PROVIDER,
            entry(&["old-a", "old-b"], 1),
            vec![classifier("old-a"), classifier("old-b")],
            None,
        )
        .await
        .unwrap();
    store.write("other", entry(&["o"], 9), None).await.unwrap();

    store
        .write_with_classifiers(PROVIDER, entry(&["new"], 2), vec![classifier("new")], None)
        .await
        .unwrap();

    let members = on_disk(&path)[PROVIDER]["models"]
        .as_array()
        .unwrap()
        .clone();
    let shown: Vec<(&str, &str)> = members
        .iter()
        .map(|member| {
            (
                member["type"].as_str().unwrap_or("chat"),
                member["id"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(shown, [("chat", "new"), ("classifier", "new")]);
    assert_eq!(on_disk(&path)[PROVIDER]["checkedAt"], 2);

    // A fresh store (a restarted process) reads the new pair back; other providers are untouched.
    let reopened = FileModelsStore::new(&path);
    let stored = reopened.read(PROVIDER, None).await.unwrap().unwrap();
    assert_eq!(ids(&stored.models), ["new"]);
    assert_eq!(
        classifier_ids(
            &reopened
                .read_classifier_models(PROVIDER, None)
                .await
                .unwrap()
        ),
        ["new"]
    );
    assert_eq!(
        reopened
            .read("other", None)
            .await
            .unwrap()
            .unwrap()
            .checked_at,
        Some(9)
    );

    // An empty classifier list leaves a chat-only entry.
    reopened
        .write_with_classifiers(PROVIDER, entry(&["chat-only"], 3), Vec::new(), None)
        .await
        .unwrap();
    assert!(
        reopened
            .read_classifier_models(PROVIDER, None)
            .await
            .unwrap()
            .is_empty()
    );
}

/// An aborted call is refused before the lock and the file, like every other write.
#[tokio::test]
async fn write_with_classifiers_honours_an_aborted_signal() {
    let (_dir, path) = path();
    let store = FileModelsStore::new(&path);
    store
        .write_with_classifiers(
            PROVIDER,
            entry(&["kept"], 1),
            vec![classifier("kept")],
            None,
        )
        .await
        .unwrap();
    let token = cyrup_core::CancelToken::new();
    token.cancel();
    let cancelled = ModelsStoreOperationOptions {
        signal: Some(token),
    };

    let error = store
        .write_with_classifiers(PROVIDER, entry(&["lost"], 2), Vec::new(), Some(&cancelled))
        .await
        .unwrap_err();

    assert_eq!(error.code(), "aborted");
    let stored = store.read(PROVIDER, None).await.unwrap().unwrap();
    assert_eq!(ids(&stored.models), ["kept"]);
}
