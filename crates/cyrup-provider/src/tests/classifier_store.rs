//! Persisting classifier models beside a provider's chat catalog (pi
//! `ModelsStoreEntry.models: readonly AnyModel[]`, `packages/ai/src/models-store.ts:3-5`, written
//! and restored by `extensions/llama/provider.ts:203-221`, `:253` @v0.99.2-17).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use super::classifier_support::{LLAMA_API, chat_model, classifier_model};
use crate::error::ProviderError;
use crate::models_store::{
    InMemoryModelsStore, ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions,
    ProviderModelsStore,
};
use cyrup_core::CancelToken;

fn cancelled() -> ModelsStoreOperationOptions {
    let token = CancelToken::new();
    token.cancel();
    ModelsStoreOperationOptions {
        signal: Some(token),
    }
}

/// A store that implements only the three required methods, the shape of every store written
/// before classifier models existed.
struct RequiredOnlyStore(InMemoryModelsStore);

#[async_trait::async_trait]
impl ModelsStore for RequiredOnlyStore {
    async fn read(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        self.0.read(provider_id, options).await
    }
    async fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.0.write(provider_id, entry, options).await
    }
    async fn delete(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.0.delete(provider_id, options).await
    }
}

// ---------------------------------------------------------------------------------- round trip --

#[tokio::test]
async fn round_trip_without_classifier_models() {
    let store = InMemoryModelsStore::new();
    let entry = ModelsStoreEntry {
        models: vec![chat_model("llama.cpp", "m1")],
        checked_at: Some(7),
        ..ModelsStoreEntry::default()
    };

    store.write("llama.cpp", entry.clone(), None).await.unwrap();

    assert_eq!(store.read("llama.cpp", None).await.unwrap(), Some(entry));
    assert!(
        store
            .read_classifier_models("llama.cpp", None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn round_trip_with_classifier_models_in_order() {
    let store = InMemoryModelsStore::new();
    let models = vec![
        classifier_model("llama.cpp", "zeta", LLAMA_API),
        classifier_model("llama.cpp", "alpha", LLAMA_API),
    ];

    store
        .write_classifier_models("llama.cpp", models.clone(), None)
        .await
        .unwrap();

    assert_eq!(
        store
            .read_classifier_models("llama.cpp", None)
            .await
            .unwrap(),
        models
    );
    // Scoped by provider id.
    assert!(
        store
            .read_classifier_models("other", None)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Chat entry and classifier models are written independently: neither call disturbs the other,
/// and an empty list clears the classifier models (`persist: { models: [...] }` replaces the whole
/// array in pi, so a caller persisting both writes both).
#[tokio::test]
async fn chat_entry_and_classifier_models_are_independent() {
    let store = InMemoryModelsStore::new();
    let chat = ModelsStoreEntry {
        models: vec![chat_model("p", "m")],
        checked_at: Some(1),
        ..ModelsStoreEntry::default()
    };
    let classifiers = vec![classifier_model("p", "c", LLAMA_API)];
    store.write("p", chat.clone(), None).await.unwrap();
    store
        .write_classifier_models("p", classifiers.clone(), None)
        .await
        .unwrap();

    // Rewriting the chat entry keeps the classifier models.
    let newer = ModelsStoreEntry {
        checked_at: Some(2),
        ..chat.clone()
    };
    store.write("p", newer.clone(), None).await.unwrap();
    assert_eq!(
        store.read_classifier_models("p", None).await.unwrap(),
        classifiers
    );

    // Rewriting the classifier models keeps the chat entry.
    let replaced = vec![classifier_model("p", "c2", LLAMA_API)];
    store
        .write_classifier_models("p", replaced.clone(), None)
        .await
        .unwrap();
    assert_eq!(store.read("p", None).await.unwrap(), Some(newer));
    assert_eq!(
        store.read_classifier_models("p", None).await.unwrap(),
        replaced
    );

    // An empty list clears them.
    store
        .write_classifier_models("p", Vec::new(), None)
        .await
        .unwrap();
    assert!(
        store
            .read_classifier_models("p", None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.read("p", None).await.unwrap().is_some());
}

/// pi's `entries.delete(providerId)` removes the whole entry (`models-store.ts:42-44`).
#[tokio::test]
async fn delete_removes_the_classifier_models_too() {
    let store = InMemoryModelsStore::new();
    store
        .write_classifier_models("p", vec![classifier_model("p", "c", LLAMA_API)], None)
        .await
        .unwrap();
    store
        .write_classifier_models("q", vec![classifier_model("q", "c", LLAMA_API)], None)
        .await
        .unwrap();

    store.delete("p", None).await.unwrap();

    assert!(
        store
            .read_classifier_models("p", None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.read_classifier_models("q", None).await.unwrap().len(),
        1
    );
}

/// `options?.signal?.throwIfAborted()` is the first statement of every operation
/// (`models-store.ts:31`, `:37`, `:42`); the two new operations honour it and an aborted write does
/// not mutate.
#[tokio::test]
async fn an_aborted_signal_rejects_the_classifier_operations_without_mutating() {
    let store = InMemoryModelsStore::new();
    let kept = vec![classifier_model("p", "c", LLAMA_API)];
    store
        .write_classifier_models("p", kept.clone(), None)
        .await
        .unwrap();
    let options = cancelled();

    let read = store
        .read_classifier_models("p", Some(&options))
        .await
        .unwrap_err();
    let write = store
        .write_classifier_models("p", Vec::new(), Some(&options))
        .await
        .unwrap_err();

    assert!(matches!(read, ProviderError::Aborted), "got: {read:?}");
    assert!(matches!(write, ProviderError::Aborted), "got: {write:?}");
    assert_eq!(store.read_classifier_models("p", None).await.unwrap(), kept);
}

#[tokio::test]
async fn the_scoped_store_forwards_the_classifier_operations() {
    let backing: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
    let scoped = ProviderModelsStore::new(backing.clone(), "llama.cpp");
    let models = vec![classifier_model("llama.cpp", "c", LLAMA_API)];

    scoped
        .write_classifier_models(models.clone(), None)
        .await
        .unwrap();

    assert_eq!(scoped.read_classifier_models(None).await.unwrap(), models);
    assert_eq!(
        backing
            .read_classifier_models("llama.cpp", None)
            .await
            .unwrap(),
        models
    );
    let options = cancelled();
    assert!(matches!(
        scoped
            .read_classifier_models(Some(&options))
            .await
            .unwrap_err(),
        ProviderError::Aborted
    ));
    assert!(matches!(
        scoped
            .write_classifier_models(Vec::new(), Some(&options))
            .await
            .unwrap_err(),
        ProviderError::Aborted
    ));
}

// ------------------------------------------------------------------- stores without support --

/// A store that predates classifier models holds none, and reports it on a write that would lose
/// something rather than dropping it silently; an empty write has nothing to lose.
#[tokio::test]
async fn a_store_without_classifier_support_says_so() {
    let store = RequiredOnlyStore(InMemoryModelsStore::new());

    assert!(
        store
            .read_classifier_models("p", None)
            .await
            .unwrap()
            .is_empty()
    );
    store
        .write_classifier_models("p", Vec::new(), None)
        .await
        .unwrap();
    let err = store
        .write_classifier_models("p", vec![classifier_model("p", "c", LLAMA_API)], None)
        .await
        .unwrap_err();

    assert_eq!(err.code(), "model_source");
    assert!(
        err.to_string()
            .contains("does not persist classifier models"),
        "got: {err}"
    );
    assert!(
        store
            .read_classifier_models("p", None)
            .await
            .unwrap()
            .is_empty()
    );
}

/// The default methods of a store without classifier support honour a cancelled signal too, before
/// they answer (`throwIfAborted` first, as every store operation does).
#[tokio::test]
async fn a_store_without_classifier_support_honours_an_aborted_signal() {
    let store = RequiredOnlyStore(InMemoryModelsStore::new());
    let options = cancelled();

    let read = store
        .read_classifier_models("p", Some(&options))
        .await
        .unwrap_err();
    let empty_write = store
        .write_classifier_models("p", Vec::new(), Some(&options))
        .await
        .unwrap_err();
    let write = store
        .write_classifier_models(
            "p",
            vec![classifier_model("p", "c", LLAMA_API)],
            Some(&options),
        )
        .await
        .unwrap_err();

    assert!(matches!(read, ProviderError::Aborted), "got: {read:?}");
    assert!(
        matches!(empty_write, ProviderError::Aborted),
        "got: {empty_write:?}"
    );
    assert!(matches!(write, ProviderError::Aborted), "got: {write:?}");
}

// ----------------------------------------------------------------------- the existing file shape --

/// A store file written before classifier models existed still parses, and persisting classifier
/// models leaves the entry's own wire shape exactly as it was.
#[test]
fn an_old_store_file_still_parses_and_the_entry_shape_is_unchanged() {
    let old = r#"{
        "models": [{
            "id": "m", "name": "M", "api": "openai-completions", "provider": "p",
            "baseUrl": "http://x/v1", "reasoning": false, "input": ["text"],
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
            "contextWindow": 1000, "maxTokens": 100
        }],
        "lastModified": 3,
        "checkedAt": 5,
        "etag": "\"v1\""
    }"#;

    let entry: ModelsStoreEntry = serde_json::from_str(old).unwrap();

    assert_eq!(entry.models.len(), 1);
    assert_eq!(entry.checked_at, Some(5));
    assert_eq!(entry.etag.as_deref(), Some("\"v1\""));
    let out = serde_json::to_value(&entry).unwrap();
    assert!(out.get("classifierModels").is_none());
    assert!(out["models"][0].get("type").is_none());
}

// --------------------------------------------------------------------- one operation for both --

/// pi persists `{ models: [...refreshed, ...refreshedClassifiers] }` in ONE `write`
/// (`extensions/llama/provider.ts:251-253`): the in-memory store replaces both halves together, and
/// an empty classifier list clears the old ones, as the one array being replaced does.
#[tokio::test]
async fn write_with_classifiers_replaces_both_halves_together() {
    let store = InMemoryModelsStore::new();
    let old = ModelsStoreEntry {
        models: vec![chat_model("p", "old")],
        checked_at: Some(1),
        ..ModelsStoreEntry::default()
    };
    store.write("p", old, None).await.unwrap();
    store
        .write_classifier_models("p", vec![classifier_model("p", "old", LLAMA_API)], None)
        .await
        .unwrap();

    let new = ModelsStoreEntry {
        models: vec![chat_model("p", "new")],
        checked_at: Some(2),
        ..ModelsStoreEntry::default()
    };
    let classifiers = vec![classifier_model("p", "new", LLAMA_API)];
    store
        .write_with_classifiers("p", new.clone(), classifiers.clone(), None)
        .await
        .unwrap();

    assert_eq!(store.read("p", None).await.unwrap(), Some(new.clone()));
    assert_eq!(
        store.read_classifier_models("p", None).await.unwrap(),
        classifiers
    );

    store
        .write_with_classifiers("p", new, Vec::new(), None)
        .await
        .unwrap();
    assert!(
        store
            .read_classifier_models("p", None)
            .await
            .unwrap()
            .is_empty(),
        "an empty list clears the classifier models"
    );
}

/// The same operation through the provider-scoped view, and an aborted one changes neither half.
#[tokio::test]
async fn write_with_classifiers_through_the_scoped_store_and_when_aborted() {
    let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
    let scoped = ProviderModelsStore::new(store.clone(), "p");
    let kept = ModelsStoreEntry {
        models: vec![chat_model("p", "kept")],
        ..ModelsStoreEntry::default()
    };
    let kept_classifiers = vec![classifier_model("p", "kept", LLAMA_API)];
    scoped
        .write_with_classifiers(kept.clone(), kept_classifiers.clone(), None)
        .await
        .unwrap();

    let error = scoped
        .write_with_classifiers(ModelsStoreEntry::default(), Vec::new(), Some(&cancelled()))
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::Aborted), "{error:?}");
    assert_eq!(store.read("p", None).await.unwrap(), Some(kept));
    assert_eq!(
        store.read_classifier_models("p", None).await.unwrap(),
        kept_classifiers
    );
}

/// A store with no classifier support refuses a non-empty list BEFORE the chat half is written:
/// the default composes the halves classifier-first, so the refusal leaves the old catalog intact
/// rather than failing after replacing it.
#[tokio::test]
async fn the_default_write_with_classifiers_refuses_before_touching_the_chat_half() {
    let store = RequiredOnlyStore(InMemoryModelsStore::new());
    let old = ModelsStoreEntry {
        models: vec![chat_model("p", "old")],
        ..ModelsStoreEntry::default()
    };
    store.write("p", old.clone(), None).await.unwrap();

    let error = store
        .write_with_classifiers(
            "p",
            ModelsStoreEntry {
                models: vec![chat_model("p", "new")],
                ..ModelsStoreEntry::default()
            },
            vec![classifier_model("p", "c", LLAMA_API)],
            None,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::ModelSource(_)), "{error:?}");
    assert_eq!(
        store.read("p", None).await.unwrap(),
        Some(old),
        "the chat half was not replaced"
    );

    // Without classifier models the default is just the chat write.
    let new = ModelsStoreEntry {
        models: vec![chat_model("p", "new")],
        ..ModelsStoreEntry::default()
    };
    store
        .write_with_classifiers("p", new.clone(), Vec::new(), None)
        .await
        .unwrap();
    assert_eq!(store.read("p", None).await.unwrap(), Some(new));
}
