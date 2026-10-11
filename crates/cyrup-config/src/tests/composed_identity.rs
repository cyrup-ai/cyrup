//! A `models.json` block composes a provider's catalog and auth; it must not cost the provider its
//! identity. pi's `composeModelProvider` builds the composed provider as
//!
//! ```ts
//! name: extension?.name ?? config?.name ?? base?.name ?? extension?.oauth?.name ?? providerId,
//! baseUrl: extension?.baseUrl ?? config?.baseUrl ?? base?.baseUrl,
//! ```
//!
//! (`packages/coding-agent/src/core/provider-composer.ts:607-608` @f1b2e77f5). No extension layer
//! composes here, so the chain is the block's value, then the base's, then (for the name) the id.
//! Before CFG-113 the composed `WireProvider` carried neither from its base: any block renamed
//! `OpenAI` to `openai` and dropped the provider-level `baseUrl` (PROV-154).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use crate::{ModelFile, compose_provider_registry, load_models_file};
use cyrup_provider::{CreateModelsOptions, InMemoryCredentialStore};

fn model_file(json: &str) -> ModelFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, json).unwrap();
    load_models_file(&path).expect("models.json parses")
}

/// The composed provider's `(name, base_url)` for `id` under `json`.
fn composed(json: &str, id: &str) -> (String, Option<String>) {
    let file = model_file(json);
    let (models, errors) = compose_provider_registry(
        &file,
        CreateModelsOptions {
            credentials: Some(Arc::new(InMemoryCredentialStore::new())),
            auth_context: None,
            catalog_overlay: None,
        },
    );
    assert!(errors.is_empty(), "the block composes cleanly: {errors:?}");
    let provider = models.get_provider(id).expect("registered");
    (
        provider.name().to_string(),
        provider.base_url().map(str::to_string),
    )
}

/// `?? base?.name` and `?? base?.baseUrl`: a block that sets neither keeps the base's.
#[test]
fn a_composed_builtin_keeps_its_bases_name_and_base_url() {
    assert_eq!(
        composed(
            r#"{ "providers": { "meta": { "apiKey": "sk-test" } } }"#,
            "meta"
        ),
        (
            "Meta".to_string(),
            Some("https://api.meta.ai/v1".to_string())
        )
    );
    assert_eq!(
        composed(
            r#"{ "providers": { "openai": { "apiKey": "sk-test" } } }"#,
            "openai"
        ),
        (
            "OpenAI".to_string(),
            Some("https://api.openai.com/v1".to_string())
        )
    );
}

/// `config?.name ?? …` and `config?.baseUrl ?? …`: the block's own values win over the base's.
#[test]
fn the_blocks_name_and_base_url_win_over_the_bases() {
    assert_eq!(
        composed(
            r#"{ "providers": { "openai": { "name": "Gateway", "baseUrl": "https://gateway.example/v1" } } }"#,
            "openai"
        ),
        (
            "Gateway".to_string(),
            Some("https://gateway.example/v1".to_string())
        )
    );
}

/// The provider's own `baseUrl` takes the block's value even under `oauth` (`:608` reads
/// `config?.baseUrl` unconditionally), while the ROWS keep theirs there (`:326`: `config.oauth ===
/// "radius" ? model.baseUrl : …`). The two URLs differ by design: the block's is the auth gateway.
/// pi's expected value is the same whatever the base (it replaces the base of an `oauth: "radius"`
/// block with `radiusProvider`, which has no `baseUrl`, `model-runtime.ts:263-276`; cyrup keeps the
/// built-in base, `[CYRUP-DELTA]` in `model/schema.rs`), because the block's value comes first.
#[test]
fn under_oauth_the_provider_takes_the_blocks_base_url_and_the_rows_keep_theirs() {
    let json = r#"{ "providers": { "openrouter": { "baseUrl": "https://gateway.example/v1", "oauth": "radius" } } }"#;
    assert_eq!(
        composed(json, "openrouter").1.as_deref(),
        Some("https://gateway.example/v1")
    );
    let file = model_file(json);
    let (models, _) = compose_provider_registry(
        &file,
        CreateModelsOptions {
            credentials: Some(Arc::new(InMemoryCredentialStore::new())),
            auth_context: None,
            catalog_overlay: None,
        },
    );
    let provider = models.get_provider("openrouter").expect("registered");
    assert!(
        provider
            .models()
            .iter()
            .all(|m| m.base_url != "https://gateway.example/v1"),
        "rows keep their own baseUrl under oauth (provider-composer.ts:326)"
    );
}

/// A builtin whose pi module passes no `baseUrl` (`providers/opencode.ts:14-27` @f1b2e77f5)
/// still has none once composed — the composer must not invent one, for example from its rows
/// (opencode's carry two different URLs).
#[test]
fn a_composed_builtin_without_a_base_url_still_has_none() {
    assert_eq!(
        composed(
            r#"{ "providers": { "opencode": { "apiKey": "sk-test" } } }"#,
            "opencode"
        ),
        ("OpenCode Zen".to_string(), None)
    );
}

/// A provider that exists only in `models.json` has no base: its name and URL are the block's,
/// and without a `name` the id stands in (`?? providerId`).
#[test]
fn a_base_less_provider_takes_the_blocks_base_url_and_falls_back_to_its_id() {
    assert_eq!(
        composed(
            r#"{ "providers": { "acme": {
                "baseUrl": "https://acme.example/v1",
                "api": "openai-completions",
                "apiKey": "sk-test",
                "models": [{ "id": "acme-large" }]
            } } }"#,
            "acme"
        ),
        (
            "acme".to_string(),
            Some("https://acme.example/v1".to_string())
        )
    );
}
